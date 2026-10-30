//! From a run and a seed to an sd-server `img_gen` request, and from the
//! finished job to the image's [`Recipe`].

use base64::Engine;
use serde_json::{Value, json};

use crate::error::Result;
use crate::library::Library;
use crate::model::{LoraFile, Mode, Recipe, Run};

/// Qwen-Image 2.1 decides on alpha from the prompt (DESIGN.md, Transparency).
pub fn prompt_sent(prompt: &str, alpha: bool) -> String {
    if alpha {
        let p = prompt.trim().trim_end_matches('.');
        format!(
            "This is an RGBA image with transparency. {p}. The image has alpha channel and the background is transparent."
        )
    } else {
        prompt.to_string()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenOptions {
    /// sd-server preview mode: `none`, `proj`, `tae` or `vae`.
    pub preview: String,
    pub preview_interval: u32,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions { preview: "none".into(), preview_interval: 1 }
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The `img_gen` body for one seed. Refs and the edit init are read from the
/// library (after checking their pixel hashes) and sent as base64 PNG with
/// their native channels.
pub fn img_gen_body(lib: &Library, run: &Run, seed: u64, opts: &GenOptions) -> Result<Value> {
    let refs = run.refs.iter().map(|r| lib.read_verified(r)).collect::<Result<Vec<_>>>()?;
    let mut sample = json!({
        "sample_method": run.sample.method,
        "sample_steps": run.sample.steps,
        "guidance": { "txt_cfg": run.sample.cfg },
    });
    if !run.sample.scheduler.is_empty() {
        sample["scheduler"] = json!(run.sample.scheduler);
    }
    if let Some(shift) = run.sample.flow_shift {
        sample["flow_shift"] = json!(shift);
    }
    if !run.sample.custom_sigmas.is_empty() {
        sample["custom_sigmas"] = json!(run.sample.custom_sigmas);
    }
    let prompt = prompt_sent(&run.prompt, run.alpha);
    let mut body = img_gen_body_from(&prompt, &run.negative_prompt, run.width, run.height, seed, sample, &refs, opts);
    if !run.model.loras.is_empty() {
        body["lora"] = lora_field(&run.model.loras);
    }
    if let Mode::Edit { init, strength } = &run.mode {
        body["strength"] = json!(strength);
        if let Some(init) = init {
            body["init_image"] = json!(b64(&lib.read_verified(init)?));
        }
    }
    Ok(body)
}

/// The request's `lora` list (both servers take this): each LoRA's path
/// inside the LoRA folder, and its weight.
pub fn lora_field(loras: &[LoraFile]) -> Value {
    loras
        .iter()
        .map(|l| {
            json!({
                "path": crate::config::lora_request_path(&l.id),
                // as typed, not as f32 widened (0.8, not 0.800000011920929)
                "multiplier": (f64::from(l.weight) * 1e4).round() / 1e4,
            })
        })
        .collect()
}

/// The `img_gen` body from its parts: `sample` is `sample_params`, `refs`
/// the reference images' bytes (the benchmark builds requests with it).
#[allow(clippy::too_many_arguments)]
pub fn img_gen_body_from(
    prompt: &str,
    negative_prompt: &str,
    width: u32,
    height: u32,
    seed: u64,
    sample: Value,
    refs: &[Vec<u8>],
    opts: &GenOptions,
) -> Value {
    json!({
        "prompt": prompt,
        "negative_prompt": negative_prompt,
        "width": width,
        "height": height,
        "seed": seed,
        "batch_count": 1,
        "sample_params": sample,
        "ref_images": refs.iter().map(|r| b64(r)).collect::<Vec<_>>(),
        "output_format": "png",
        "embed_image_metadata": true,
        "preview": opts.preview,
        "preview_interval": opts.preview_interval,
    })
}

/// A small throwaway job after launch or load, so lazy weight loading
/// doesn't land on the first real seed.
pub fn warmup_body() -> Value {
    json!({
        "prompt": "warm-up",
        "width": 512,
        "height": 512,
        "seed": 0,
        "batch_count": 1,
        "sample_params": { "sample_steps": 1 },
        "output_format": "png",
        "preview": "none",
    })
}

/// Where and how an image was generated, for its recipe.
#[derive(Clone, Debug, Default)]
pub struct Provenance {
    pub device: String,
    pub elapsed_s: f32,
    pub queued_s: f32,
    pub step_s: f32,
    pub sdcpp: String,
}

pub fn recipe(run: &Run, seed: u64, p: &Provenance) -> Recipe {
    let parent = match &run.mode {
        Mode::Edit { init, .. } => init.clone(),
        Mode::Generate => None,
    };
    Recipe {
        prompt_original: run.prompt.clone(),
        prompt_sent: prompt_sent(&run.prompt, run.alpha),
        negative_prompt: run.negative_prompt.clone(),
        width: run.width,
        height: run.height,
        seed,
        alpha: run.alpha,
        sample: run.sample.clone(),
        mode: run.mode.clone(),
        refs: run.refs.clone(),
        parent,
        source_idea: run.source_idea.clone(),
        rewrite: None,
        model: run.model.clone(),
        sdcpp: p.sdcpp.clone(),
        increart: env!("CARGO_PKG_VERSION").into(),
        device: p.device.clone(),
        elapsed_s: p.elapsed_s,
        queued_s: p.queued_s,
        step_s: p.step_s,
        inputs: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_wrapper() {
        assert_eq!(prompt_sent("a fox.", false), "a fox.");
        assert_eq!(
            prompt_sent("cute robot sticker.", true),
            "This is an RGBA image with transparency. cute robot sticker. The image has alpha channel and the background is transparent."
        );
    }

    #[test]
    fn loras_are_sent_as_paths_inside_their_folder() {
        let l = |id: &str, weight| LoraFile { id: id.into(), sha256: String::new(), weight };
        assert_eq!(
            lora_field(&[l("loras/style.safetensors", 0.8), l("Lora/sub/x.gguf", 1.0)]),
            json!([{ "path": "style.safetensors", "multiplier": 0.8 }, { "path": "sub/x.gguf", "multiplier": 1.0 }])
        );
    }
}

//! From a run and a seed to an sd-server `img_gen` request, and from the
//! finished job to the image's [`Recipe`].

use base64::Engine;
use serde_json::{Value, json};

use crate::error::Result;
use crate::library::Library;
use crate::model::{LoraFile, Mode, Recipe, Run};

/// The prompt as the server gets it: Qwen-Image 2.1 decides on alpha from
/// the prompt (DESIGN.md, Transparency). References are in the model's own
/// syntax already, made so when the run was queued ([`native_refs`]).
pub fn prompt_sent(prompt: &str, alpha: bool) -> String {
    let prompt = prompt.to_string();
    if alpha {
        let p = prompt.trim().trim_end_matches('.');
        format!(
            "This is an RGBA image with transparency. {p}. The image has alpha channel and the background is transparent."
        )
    } else {
        prompt
    }
}

/// sd-server's recipes for MiniMax-H3-Image (MiniMax's own) and P-H3-Image
/// (Pruna AI's, pruned from the MiniMax-H3 video model): the same text
/// encoder family, references named `<Picture N>`.
pub const MINIMAX_H3_IMAGE: [&str; 2] = ["minimax-h3-image", "p-h3-image"];

/// `<imageN>` as `<Picture N>`: MiniMax-H3's text encoder sees each
/// reference as `<Picture N>: ` before the prompt (P-H3-Image also puts
/// `Edit <Picture 1>. ` before a prompt that doesn't start `Edit <Picture N>`).
pub fn picture_tags(prompt: &str) -> String {
    let mut out = String::with_capacity(prompt.len() + 8);
    let mut rest = prompt;
    while let Some(at) = rest.find("<image") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "<image".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with('>') {
            out.push_str(&format!("<Picture {}>", &after[..digits]));
            rest = &after[digits + 1..];
        } else {
            out.push_str("<image");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// The prompt's references in `recipe`'s own syntax, either syntax being
/// accepted: `<Picture N>` for MiniMax-H3-Image and P-H3-Image
/// ([`picture_tags`]), `<imageN>` for the others ([`as_image_tags`]). Done
/// when a run is queued, unless its model preset takes them as written
/// (`RecipePreset::refs_as_written`).
pub fn native_refs(recipe: &str, prompt: &str) -> String {
    let p = as_image_tags(prompt);
    if MINIMAX_H3_IMAGE.contains(&recipe) { picture_tags(&p) } else { p }
}

/// `<Picture N>` (MiniMax-H3's way) as `<imageN>`. Any case, the space
/// optional; anything else is left alone.
pub fn as_image_tags(prompt: &str) -> String {
    const WORD: &str = "<picture";
    if !prompt.to_ascii_lowercase().contains(WORD) {
        return prompt.to_string();
    }
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(at) = rest.to_ascii_lowercase().find(WORD) {
        out.push_str(&rest[..at]);
        let after = &rest[at + WORD.len()..];
        let n = after.trim_start_matches(' ');
        let digits = n.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && n[digits..].starts_with('>') {
            out.push_str(&format!("<image{}>", &n[..digits]));
            rest = &n[digits + 1..];
        } else {
            out.push_str(&rest[at..at + WORD.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
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
    let sample = sample_params(&run.sample);
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

/// The request's `sample_params`: the model's own sampler and schedule
/// (empty) aren't sent, so the server uses the recipe's.
pub fn sample_params(s: &crate::model::Sample) -> Value {
    let mut sample = json!({
        "sample_steps": s.steps,
        "guidance": { "txt_cfg": s.cfg },
    });
    if !s.method.is_empty() {
        sample["sample_method"] = json!(s.method);
    }
    if !s.scheduler.is_empty() {
        sample["scheduler"] = json!(s.scheduler);
    }
    if let Some(shift) = s.flow_shift {
        sample["flow_shift"] = json!(shift);
    }
    if !s.custom_sigmas.is_empty() {
        sample["custom_sigmas"] = json!(s.custom_sigmas);
    }
    sample
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

/// A warm-up body for sd-server: its schedule one step, sent rather than
/// left out (sd-server takes a recipe's own sigmas, MiniMax-H3's 7, over
/// `sample_steps` otherwise; KoboldCpp takes no sigmas, nor needs them).
pub fn one_step_sigmas(mut body: Value) -> Value {
    if let Some(sample) = body.get_mut("sample_params").and_then(Value::as_object_mut) {
        sample.insert("custom_sigmas".into(), json!([1.0, 0.0]));
    }
    body
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

/// A seed's `img_gen` body as a warm-up: one step (its own schedule
/// dropped), no previews; the prompt, size and references kept, as the
/// server's conditioning cache is keyed by them.
pub fn as_warm_up(mut body: Value) -> Value {
    if let Some(sample) = body.get_mut("sample_params").and_then(Value::as_object_mut) {
        sample.insert("sample_steps".into(), json!(1));
        sample.remove("custom_sigmas");
    }
    body["batch_count"] = json!(1);
    body["preview"] = json!("none");
    body["embed_image_metadata"] = json!(false);
    body
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

    /// A warm-up from a seed's request keeps what the conditioning cache is
    /// keyed by and samples one step.
    #[test]
    fn warm_up_from_a_seed() {
        let opts = GenOptions { preview: "proj".into(), ..Default::default() };
        let sample = json!({ "sample_steps": 20, "custom_sigmas": [1.0, 0.5, 0.0], "guidance": { "txt_cfg": 4.0 } });
        let body = img_gen_body_from("a fox", "", 800, 608, 3, sample, &[b"png".to_vec()], &opts);
        let w = as_warm_up(body.clone());
        for k in ["prompt", "width", "height", "ref_images"] {
            assert_eq!(w[k], body[k], "{k}");
        }
        assert_eq!(w["sample_params"]["sample_steps"], 1);
        assert!(w["sample_params"].get("custom_sigmas").is_none());
        let w = one_step_sigmas(w);
        assert_eq!(w["sample_params"]["custom_sigmas"], json!([1.0, 0.0]), "one step, not the recipe's");
        assert_eq!(one_step_sigmas(warmup_body())["sample_params"]["custom_sigmas"], json!([1.0, 0.0]));
        assert_eq!(w["sample_params"]["guidance"], body["sample_params"]["guidance"]);
        assert_eq!(w["preview"], "none");
    }

    #[test]
    fn rgba_wrapper() {
        assert_eq!(prompt_sent("a fox.", false), "a fox.");
        assert_eq!(
            prompt_sent("cute robot sticker.", true),
            "This is an RGBA image with transparency. cute robot sticker. The image has alpha channel and the background is transparent."
        );
    }

    /// Each model's references in its own syntax, written either way:
    /// MiniMax-H3-Image's `<Picture N>`, the others' `<imageN>`; a tag that
    /// isn't one is left alone.
    #[test]
    fn references_in_each_models_syntax() {
        let p = "Edit <image1>. Give the cat the scarf from <Picture 2>, as in <picture12>.";
        let h3 = "Edit <Picture 1>. Give the cat the scarf from <Picture 2>, as in <Picture 12>.";
        assert_eq!(native_refs("minimax-h3-image", p), h3);
        assert_eq!(native_refs("p-h3-image", p), h3);
        assert_eq!(
            native_refs("qwen-image-21", h3),
            "Edit <image1>. Give the cat the scarf from <image2>, as in <image12>."
        );
        assert_eq!(picture_tags("<image> <imageX> <image3 <image4>"), "<image> <imageX> <image3 <Picture 4>");
        assert_eq!(picture_tags("a café <image1>"), "a café <Picture 1>");
        for left in ["<Picture>", "<Picture x>", "<Picture 3", "Picture 1", "<pictures 1>", "a fox"] {
            assert_eq!(as_image_tags(left), left);
        }
        assert_eq!(as_image_tags("<PICTURE 12> café"), "<image12> café");
    }

    /// The model's own sampler: none sent (sd-server and KoboldCpp then
    /// use the recipe's).
    #[test]
    fn the_sampler_sent_unless_the_models_own() {
        use crate::model::Sample;
        let mut s = Sample {
            method: String::new(),
            scheduler: String::new(),
            steps: 8,
            cfg: 1.0,
            flow_shift: None,
            custom_sigmas: vec![],
        };
        let p = sample_params(&s);
        assert!(p.get("sample_method").is_none() && p.get("scheduler").is_none(), "{p}");
        (s.method, s.scheduler) = ("euler_a".into(), "karras".into());
        let p = sample_params(&s);
        assert_eq!((&p["sample_method"], &p["scheduler"]), (&json!("euler_a"), &json!("karras")));
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

# 2. Models: which files, where, how big

Sources: `crates/increart-core/kcppt/*.kcppt`, `crates/increart-core/src/tiers.rs`,
`crates/increart-core/src/config.rs` (`builtin_recipes`),
`crates/increart-core/src/kobold_image.rs`, [../vram-tiers.md](../vram-tiers.md),
[../DESIGN.md](../DESIGN.md). URLs and sizes were checked against the Hugging
Face API on 2026-10-29.

**Licence:** this project never redistributes weights. Each file comes from
its publisher's Hugging Face repository under that repository's licence; read
the model card before use.

The two backends need **different files**: KoboldCpp can't load sd-server's
int8 convrot safetensors (it expands them to ~29–31 GiB and fails; the app
hides them from KoboldCpp), and the app's built-in sd-server preset uses the
int8 files.

## 2.1 KoboldCpp set (GGUF)

The KoboldCpp wizard downloads these itself (step 3.5) into
`<models>/koboldcpp/`. Download by hand only if you want to pre-fetch or the
machine running the wizard has no internet.

| File | Slot in the app's model preset | Size | Needed by |
|---|---|---|---|
| `qwen_image_2.1-Q4_K.gguf` | `diffusion_model` | 3.91 GiB | 8 GB and 12 GB presets |
| `qwen_image_2.1-Q8_0.gguf` | `diffusion_model` | 7.16 GiB | 16, 24, 32 GB presets |
| `Qwen3VL-8B-Instruct-Q4_K_M.gguf` | `llm` (text encoder), and the PE chat model | 4.68 GiB | all |
| `mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf` | `clip_l` (vision for the encoder), and PE's `mmproj` | 0.70 GiB | all |
| `qwen_image_2.1_vae_bf16.safetensors` | `vae` | 0.63 GiB | all |

Total: **9.9 GiB** (Q4_K) or **13.2 GiB** (Q8_0). With GPUs of different sizes
every image server uses the smallest preset's quantization, so you only ever
need one image model.

**Goal:** the files in `<models>/koboldcpp/`, names unchanged.

**Preconditions:** ~15 GiB free disk; `curl`. Pick `QUANT=Q4_K` (8–12 GB
GPU) or `QUANT=Q8_0` (16 GB and up).

```sh
MODELS="$HOME/models"      # any folder; you give it to the wizard later
QUANT=Q8_0                 # or Q4_K
mkdir -p "$MODELS/koboldcpp" && cd "$MODELS/koboldcpp"
for url in \
  "https://huggingface.co/leejet/Qwen-Image-2.1-GGUF/resolve/main/qwen_image_2.1-$QUANT.gguf" \
  "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/main/Qwen3VL-8B-Instruct-Q4_K_M.gguf" \
  "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf" \
  "https://huggingface.co/Comfy-Org/Qwen-Image-2.1/resolve/main/vae/qwen_image_2.1_vae_bf16.safetensors"
do curl -L --fail -C - -O "$url"; done
```

**Expected result:**

```text
<models>/
  koboldcpp/
    qwen_image_2.1-Q8_0.gguf            (or -Q4_K.gguf)
    Qwen3VL-8B-Instruct-Q4_K_M.gguf
    mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf
    qwen_image_2.1_vae_bf16.safetensors
```

**Verify** (the size on disk must equal what Hugging Face reports):

```sh
cd "$MODELS/koboldcpp"
for f in *; do
  case $f in qwen_image_2.1-*) r=leejet/Qwen-Image-2.1-GGUF; p=$f;;
    qwen_image_2.1_vae*) r=Comfy-Org/Qwen-Image-2.1; p=vae/$f;;
    *) r=Qwen/Qwen3-VL-8B-Instruct-GGUF; p=$f;; esac
  want=$(curl -sIL "https://huggingface.co/$r/resolve/main/$p" | grep -i '^content-length' | tail -1 | tr -dc 0-9)
  echo "$f local=$(stat -c %s "$f") remote=$want"
done
```

Each line shows equal `local` and `remote`.

**If it fails:**
- Sizes differ / download cut off → re-run the download loop (`-C -`
  resumes).
- `curl: (22) … 401/403` → the repository may require accepting terms on
  huggingface.co; log in there, or use `huggingface-cli download` with a token.
- Already have the files elsewhere (another folder or the Hugging Face cache
  `$HF_HOME`, default `~/.cache/huggingface`)? The wizard finds files by name
  up to five folders deep and **links** them into `<models>/koboldcpp/` when
  the size matches, instead of downloading.

## 2.2 sd-server set (ComfyUI layout, int8 convrot)

This is what the app's built-in model preset "Qwen-Image 2.1" loads
(`builtin_recipes` in `config.rs`). int8 convrot needs sd.cpp's patched ggml
(the default build) and is fast on HIP/ROCm; on Vulkan the same files ran
4.7× slower (480 s vs 103 s). Speed on CUDA is **unmeasured**.

| File (path under the models root) | Slot | Size |
|---|---|---|
| `diffusion_models/qwen_image_2.1_int8_convrot.safetensors` | `diffusion_model` | 6.76 GiB |
| `text_encoders/qwen3vl_8b_int8_convrot.safetensors` | `llm` (includes the vision tower) | 8.71 GiB |
| `vae/qwen_image_2.1_vae_bf16.safetensors` | `vae` | 0.63 GiB |

Total: **16.1 GiB**.

**Goal:** the three files in ComfyUI-style category folders under one models
root.

**Preconditions:** ~17 GiB free disk.

```sh
MODELS="$HOME/models"
B=https://huggingface.co/Comfy-Org/Qwen-Image-2.1/resolve/main
for p in diffusion_models/qwen_image_2.1_int8_convrot.safetensors \
         text_encoders/qwen3vl_8b_int8_convrot.safetensors \
         vae/qwen_image_2.1_vae_bf16.safetensors; do
  mkdir -p "$MODELS/$(dirname "$p")"
  curl -L --fail -C - -o "$MODELS/$p" "$B/$p"
done
```

**Expected result:**

```text
<models>/
  diffusion_models/qwen_image_2.1_int8_convrot.safetensors
  text_encoders/qwen3vl_8b_int8_convrot.safetensors
  vae/qwen_image_2.1_vae_bf16.safetensors
```

**Verify:**

```sh
cd "$MODELS"
for p in diffusion_models/qwen_image_2.1_int8_convrot.safetensors \
         text_encoders/qwen3vl_8b_int8_convrot.safetensors \
         vae/qwen_image_2.1_vae_bf16.safetensors; do
  want=$(curl -sIL "https://huggingface.co/Comfy-Org/Qwen-Image-2.1/resolve/main/$p" | grep -i '^content-length' | tail -1 | tr -dc 0-9)
  echo "$p local=$(stat -c %s "$p") remote=$want"
done
```

After sd-server runs (guide 4), `GET /sdcpp/v1/model-files` must list these
exact IDs.

**If it fails:**
- Put this root **first** in `programs.sd_server.model_dirs`: sd-server qualifies file
  IDs of additional roots, and the built-in preset uses the bare relative
  paths above.
- Don't symlink these files from outside the root: sd-server rejects
  symlinks that resolve outside the model directory.
- Both sets can share one `<models>` folder: KoboldCpp's files live in its
  `koboldcpp/` subfolder.

## 2.3 Other files (not used by any built-in preset)

Listed so you don't download them by mistake:

- `Comfy-Org/Qwen-Image-2.1`: `*_bf16.safetensors` (13.25 GiB diffusion,
  16.33 GiB encoder), Turbo weights, `w4a8` files, ControlNet, the
  `qwen3.5_9b_qwen_image_2.1_pe_{t2i,i2i}` prompt-enhancer models (int8 /
  w4a8 safetensors only: KoboldCpp can't run them, which is why PE uses
  Qwen3-VL-8B-Instruct instead).
- `Qwen/Qwen3-VL-8B-Instruct-GGUF`: `Q8_0` / `F16` variants and
  `mmproj-…-F16.gguf` (1.08 GiB; works as PE's `mmproj` too, larger).
- `taeqi2_1.safetensors` for sd-server `tae` previews: mentioned in DESIGN.md,
  **not downloaded or tested** by this project.

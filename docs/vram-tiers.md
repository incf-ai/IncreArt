# KoboldCpp presets by VRAM

The setup wizard has presets for two image models: [MiniMax-H3-Image](#minimax-h3-image)
(the default, KoboldCpp 1.125 or later) and [Qwen-Image 2.1](#qwen-image-21).
Each preset table's **System RAM** is what the weights take beside the
GPU: offloaded weights in pinned RAM, a part run on the CPU in ordinary
RAM. KoboldCpp itself takes about 1.5 GiB more.

## MiniMax-H3-Image

MiniMax-H3-Image editing plus prompt enhance (PE) on one GPU of 12, 16, 24
or 32 GB, all in a single KoboldCpp (1.125 or later). The presets are
derived from KoboldCpp's official
[Minimax-H3-Image.kcppt](https://huggingface.co/koboldcpp/kcppt/blob/main/Minimax-H3-Image.kcppt),
its 12 GB template (a copy is kept in `crates/increart-core/kcppt/official/`),
and ship as `MiniMax-H3-Image-*GB.kcppt` beside the Qwen-Image ones. They
use unsloth's pruned GGUFs of the diffusion model, leejet's Q4_K_M GGUF of
the Qwen3-VL-32B text encoder (17 GiB) and Comfy-Org's fp16 VAE.

Measured 2026-11-22 on the same Radeon AI PRO R9700 (Vulkan, RADV), with
KoboldCpp 1.125.1: one 1024×1024 edit with one 1 MP reference, 6 steps,
CFG 1 (the recipe's), through the app's own launcher and client
(`crates/increart-core/examples/bench.rs`, its `recipe`/`steps`/`cfg`/`prompt`
case fields). The peak VRAM is the KoboldCpp process's own (fdinfo), so
other users of the card don't count. Raw results are in `tmp/bench-h3/`.

### The official template

| Settings | Per image | Peak VRAM | System RAM |
|---|---|---|---|
| Q4_K_S on the GPU, text encoder and VAE on the CPU (official) | 208 s (192–564 s) | 13.6 GiB | ~19 GiB |
| Q4_K_S offloaded, encoder and VAE on the GPU, 8 GiB cap | 57 s | 8.3 GiB | ~29 GiB |
| Q8_0 offloaded, encoder and VAE on the GPU, 8 GiB cap | 65 s | 8.6 GiB | ~39 GiB |
| Q8_0 offloaded, 12 GiB cap | 63 s | 12.4 GiB | ~39 GiB |
| Q8_0 offloaded, no cap | 62 s | 21.6 GiB | ~39 GiB |

- **The official template doesn't fit a 12 GB card on an edit.** It keeps
  the 10.75 GiB diffusion model on the GPU without a cap, and a 1 MP edit
  with a reference peaks at 13.6 GiB. It runs the 32B text encoder on the
  CPU, reference image included: 3–9 minutes an image, as the CPU is
  shared with other programs.
- **Offloading is 3.6× faster and fits.** With the weights in RAM
  (`sdoffloadcpu`) and the encoder and VAE on the GPU, an 8 GiB cap holds
  the card at 8.3 GiB. The price is RAM: all the weights are pinned, 29 GiB
  for Q4_K_S and 39 GiB for Q8_0.
- **The cap hardly matters.** 8 GiB, 12 GiB and none are within 4 s of
  each other (65, 63 and 62 s for Q8_0), so the presets differ in
  quantization and in whether PE stays loaded. Q8_0 costs 8 s an image
  over Q4_K_S, and 10 GiB more RAM.
- **Loading reads every weight from disk**: 29–39 GiB, 4–5 minutes from
  this machine's hard disk (~165 MB/s); 2¼ minutes (136 s) when the files are still in the page
  cache. A swap back to
  images after a PE ask reloads it, so on the swapping presets (12 and 16
  GB) keep the models on an SSD, or give PE a GPU of its own.

### The presets

| VRAM | Image model | Text encoder, VAE | Image VRAM cap | PE | Per image | Peak VRAM | System RAM |
|---|---|---|---|---|---|---|---|
| 12 GB | Q4_K_S | GPU | 8 GiB | swapped | 57 s | 8.3 GiB | ~29 GiB |
| 16 GB | Q8_0 | GPU | 8 GiB | swapped | 65 s | 8.6 GiB | ~39 GiB |
| 24 GB | Q8_0 | GPU | 8 GiB | resident | 65 s | 15.6 GiB | ~39 GiB |
| 32 GB | Q8_0 | GPU | 12 GiB | resident | 63 s | 19.4 GiB | ~39 GiB |

Each has the official template's settings plus prompt enhance (as the
Qwen-Image presets below: Qwen3-VL-8B Q4_K_M and its Q8_0 projector,
`"jinja": true`, `"gpulayers": 999`, an 8192-token context, autoswap on
the swapping ones) and `"sdoffloadcpu": true, "sdflashattention": true,
"sdclipdevice": -1, "sdvaedevice": -1` with the cap as `sdvramlimit`.
Templates: `cargo run -p increart-core --example tier_kcppt -- crates/increart-core/kcppt/official/Minimax-H3-Image.kcppt`.

### sd-server

The built-in MiniMax-H3-Image model has a **12 GB: Q4_K_S, Q4 encoder**
size: the same diffusion model GGUF as the KoboldCpp presets (linked, not
fetched twice) and the Q4_K_M text encoder.

sd-server's auto-fit keeps all the weights in RAM (about 30 GiB) when the
diffusion model and its 2 GiB working memory don't fit the GPU, and
computes on the GPU; what it caches there grows to its budget, which is
the free VRAM unless `--max-vram` says otherwise. Same edit as above
(1024², 1 MP reference, 6 steps, CFG 1), sd.cpp 32d516c, ROCm:

| sd-server arguments | Per image | Peak VRAM | System RAM |
|---|---|---|---|
| `--fa --max-vram 8 --vae-tiling` | 34 s | 11.7 GiB | 30 GiB |
| `--fa --max-vram 11` | 28 s | 13.0 GiB | 30 GiB |
| `--fa` (no budget, 32 GB card: all on the GPU) | 28 s | 29.9 GiB | 4 GiB |

So on a 12 GB card give the sd-server `--max-vram 8 --vae-tiling`: it fits
(just) and is still faster than KoboldCpp's 12 GB preset (57 s). The
first image after a load encodes the prompt with the 32B text encoder from
RAM: 3½–5 minutes, then the conditioning cache has it. The size's
note says this; the Models window's GPU column, which counts the whole
diffusion model, shows ✗ for it on a 12 GB card.

## Qwen-Image 2.1

Qwen-Image 2.1 editing plus prompt enhance (PE) on one GPU of 8, 12, 16,
24 or 32 GB, all in a single KoboldCpp. Each preset is a standalone
`.kcppt` template that downloads its models from Hugging Face on first
launch. They're derived from KoboldCpp's official
[Qwen-Image-2.1.kcppt](https://huggingface.co/koboldcpp/kcppt/blob/main/Qwen-Image-2.1.kcppt)
(the 12 GB tier without PE), and ship in `crates/increart-core/kcppt/`.
The app's server setup wizard reads them to plan its KoboldCpp servers
for each GPU (see DESIGN.md, Startup: servers).

Measured 2026-10-28/29 on an AMD Radeon AI PRO R9700 32 GB (Vulkan, RADV,
PCIe 5.0 x16), i9-14900K, KoboldCpp 1.123.1. Each tier was emulated on the
32 GB card with KoboldCpp's own limits, and the card's peak VRAM rise was
measured. The workload was one 1024×1024 edit with one 1 MP reference,
euler, 20 steps, CFG 2 (the app's "CFG 2, refs" recipe), plus PE rewrites
with a 768 px image, with `scripts/bench-tiers.sh` and, for the presets
below, a one-off harness (since removed; it is in git history as
`scripts/bench-autoswap.py`): one KoboldCpp per template, sampling the
card's `mem_info_vram_used` every 0.2 s, timing each request swaps
included. The results are recorded here. Templates:
`cargo run -p increart-core --example tier_kcppt -- Qwen-Image-2.1.kcppt`.
Raw results are in `tmp/bench-tiers/`.

### The presets

Each row is the template itself, launched with `--config` and measured with
the sequence the app uses: a PE ask, an image, two asks, then two images.

| VRAM | Image model | Text encoder | Image VRAM cap | PE | Per image | PE ask | Peak VRAM | System RAM (pinned) |
|---|---|---|---|---|---|---|---|---|
| 8 GB | Q4_K | CPU | 4 GiB | swapped | 364 s (443 s after a swap) | 1.0 s (6.7–7.3 s after a swap) | 6.3 GiB | ~4.5 GiB + 7.5 GiB |
| 12 GB | Q4_K | GPU | 8 GiB | swapped | 208 s (217 s after a swap) | 1.7 s (6.5–7.2 s after a swap) | 8.7 GiB | ~10 GiB |
| 16 GB | Q8_0 | GPU | 12 GiB | swapped | 210 s (218 s after a swap) | 1.3 s (6.1–6.3 s after a swap) | 12.3 GiB | ~13.5 GiB |
| 24 GB | Q8_0 | GPU | 10 GiB | resident | 214–217 s | 0.9–2.2 s | 18.1 GiB | ~13.5 GiB |
| 32 GB | Q8_0 | GPU | none | resident | 207–211 s | 1.3–2.3 s | 22.7 GiB | ~13.5 GiB |

- **Swapped**: KoboldCpp's autoswap (`admin`, `routermode`, `autoswapmode`)
  keeps only one model family on the card. A chat request unloads the image
  model and loads the chat model, and an image request does the reverse.
- **Resident**: both stay loaded, and nothing is reloaded.
- **Peak VRAM** is the card's rise over idle. Leave room for the desktop and
  other apps: every preset keeps at least 1.7 GiB free (8 GB) and up to
  ~9 GiB (32 GB).
- **Pinned RAM**: offloaded weights live in pinned system RAM (GTT on
  Linux), so the machine needs this much free RAM beyond KoboldCpp's own
  ~1–1.5 GiB. The 8 GB preset also runs its text encoder in ordinary RAM.

### What the measurements showed

- **The VRAM cap is the lever.** Offloading the weights to RAM
  (`sdoffloadcpu`) frees the weights. It costs no speed in KoboldCpp
  (Q8_0: 206 s offloaded vs 205 s on the GPU), but it still leaves 13–18 GiB
  of working buffers for a 1 MP edit with a reference. `sdvramlimit` (sd.cpp's
  `--max-vram`) caps those. Speed holds down to a 7 GiB cap (Q8_0: 224 s,
  +9%), then falls off a cliff: at 6 GiB it's 359 s (+75%), and at 4 GiB 363 s.
- **Q8_0 costs no speed against Q4_K** (206 s vs 201–204 s), only VRAM or
  pinned RAM. From 16 GB up, the presets use Q8_0 for quality.
- **PE beside the image model costs ~7 GiB** (Qwen3-VL-8B Q4_K_M, its
  vision projector and an 8192-token context; 6.2–6.8 GiB with the
  templates' Q8_0 projector). It takes nothing off image
  speed, and each ask takes 1–2 s at ~90 tok/s.
- **Swapping costs ~5 s toward PE and ~9 s toward images.** In the app's
  rhythm (enhance, then generate), that's about 15 s on a 3½-minute image. At 8 GB the
  swap toward images costs ~75 s, because the CPU text encoder encodes the
  prompt again after every reload.
- **The text encoder on the CPU** saves only ~1.6 GiB, and costs ~70 s
  whenever the prompt changes. Only the 8 GB preset uses it, because there
  the 4 GiB cap plus a GPU encoder wouldn't fit.
- The PE chat model and the image model's text encoder are the same GGUF.
  KoboldCpp downloads it once, but loads it twice when both are resident.

### Using a template

```sh
koboldcpp --config Qwen-Image-2.1-12GB.kcppt
```

Command-line flags override the template (port, `--downloaddir`, ...),
with two things to know:

- **On a machine with more than one GPU, pick the GPU with an environment
  variable**, for example `GGML_VK_VISIBLE_DEVICES=1 koboldcpp --config …`
  (Vulkan), or `CUDA_VISIBLE_DEVICES` or `HIP_VISIBLE_DEVICES`. Every
  autoswap reload of a `.kcppt` re-picks the backend
  (`auto_set_backend_cli`) and drops `usevulkan`, whether it came from the
  command line or the file. The reloaded chat model was then split across
  both cards. Reloads inherit the environment, so the variable holds. A
  single-GPU machine is unaffected.
- The templates set `"launch": false`, because the official one opens a
  browser at launch, which a server shouldn't.

### Template settings

Every template has the official template's settings plus:

```json
"model_param": "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/main/Qwen3VL-8B-Instruct-Q4_K_M.gguf",
"mmproj": "https://huggingface.co/Qwen/Qwen3-VL-8B-Instruct-GGUF/resolve/main/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf",
"launch": false, "jinja": true, "gpulayers": 999,
"sdoffloadcpu": true, "sdflashattention": true, "sdtiledvae": 512,
"sdvaedevice": -1, "sdmaingpu": -1
```

(`sdllm`, `sdclip1` and `sdvae` are the official template's URLs.) Per tier:

| Tier | `sdmodel` | `sdclipdevice` | `sdvramlimit` | `contextsize` | Autoswap |
|---|---|---|---|---|---|
| 8GB | `…/leejet/Qwen-Image-2.1-GGUF/resolve/main/qwen_image_2.1-Q4_K.gguf` | -2 (CPU) | 4096 | 4096 | yes |
| 12GB | `…/qwen_image_2.1-Q4_K.gguf` | -1 (GPU) | 8192 | 8192 | yes |
| 16GB | `…/qwen_image_2.1-Q8_0.gguf` | -1 | 12288 | 8192 | yes |
| 24GB | `…/qwen_image_2.1-Q8_0.gguf` | -1 | 10240 | 8192 | no |
| 32GB | `…/qwen_image_2.1-Q8_0.gguf` | -1 | 0 (none) | 8192 | no |

"Autoswap yes" sets `"admin": true, "routermode": true, "autoswapmode": true`.

### Caveats

- Measured with one reference at 1 MP. More references or a larger output
  need bigger working buffers: the cap holds VRAM, but each step slows down
  as it nears the cliff.
- On AMD with Vulkan (RADV). CUDA and ROCm builds will place buffers
  differently, so their peaks may differ by a GiB or so.
- The emulated tiers ran on a 32 GB card at PCIe 5.0 x16. Offloaded weights
  stream over PCIe every step, so a slower link (PCIe 4.0 x8, say) will be
  slower. The cliff below a 7 GiB cap is that streaming taking over.
- Standalone, the templates swap with KoboldCpp's autoswap, which reloads
  on the request path: a chat request during an image kills the image.
  The app doesn't use autoswap. Its wizard takes the same settings and the
  app does the swapping itself, letting an image finish before a chat ask
  (DESIGN.md, KoboldCpp as an image server, Swapping).

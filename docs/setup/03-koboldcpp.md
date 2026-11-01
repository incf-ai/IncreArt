# 3. KoboldCpp (recommended image backend)

Sources: `crates/increart-core/src/kobold.rs`, `kobold_image.rs`,
`launcher.rs`, `process.rs`, `tiers.rs`, `pe.rs`,
`crates/increart-core/kcppt/`, `crates/increart-gui/src/startup.rs`,
[../vram-tiers.md](../vram-tiers.md), [../todo-kcpp-support.md](../todo-kcpp-support.md).

IncreArt starts one KoboldCpp per image GPU, loads Qwen-Image 2.1 into it
through KoboldCpp's admin mode, and (by default) uses the same process as the
prompt-enhance chat server. You only download the binary and run the app's
wizard.

Tested version: **KoboldCpp 1.123.1** (Qwen-Image 2.1 support first appeared
in 1.123). Older versions won't load the model.

## 3.1 Pick the build

**Goal:** the right release file for your GPU.

Release: <https://github.com/LostRuins/koboldcpp/releases> (assets of v1.123.1
checked 2026-10-29).

| OS | GPU | Asset | App runtime | Status |
|---|---|---|---|---|
| Linux x64 | NVIDIA | `koboldcpp-linux-x64` (618 MiB, CUDA) | CUDA | untested here |
| Linux x64 | AMD / Intel / other with a Vulkan driver | `koboldcpp-linux-x64-nocuda` (131 MiB, Vulkan + CPU) | Vulkan | measured (AMD) |
| Windows x64 | NVIDIA | `koboldcpp.exe` | CUDA | untested |
| Windows x64 | other | `koboldcpp-nocuda.exe` | Vulkan | untested |
| macOS | Apple silicon | `koboldcpp-mac-arm64` | Metal | untested |

- There is **no official Linux ROCm build**; the ROCm fork
  (YellowRoseCx/koboldcpp-rocm) last released v1.104, Windows only, too old
  for Qwen-Image 2.1. On AMD use the Vulkan build, or sd-server with ROCm
  (guide 4).
- The `-oldpc` builds are not covered.
- A `nocuda` file name with runtime CUDA or ROCm makes the wizard warn "This
  KoboldCpp build has Vulkan only".

## 3.2 Download

**Goal:** an executable KoboldCpp where IncreArt finds it by itself.

**Preconditions:** 3.1 chosen.

IncreArt looks for KoboldCpp (when no config names one) on `PATH`, then in
`~`, `~/Downloads`, `~/.local/bin` and any `~/koboldcpp*` folder; a file named
`koboldcpp` or `koboldcpp-…` / `koboldcpp_…` that is executable
(`kobold::find_binary`).

```sh
V=v1.123.1
ASSET=koboldcpp-linux-x64-nocuda       # NVIDIA: koboldcpp-linux-x64
mkdir -p "$HOME/koboldcpp"
curl -L --fail -o "$HOME/koboldcpp/$ASSET" \
  "https://github.com/LostRuins/koboldcpp/releases/download/$V/$ASSET"
chmod +x "$HOME/koboldcpp/$ASSET"
```

macOS: same with `ASSET=koboldcpp-mac-arm64`; Gatekeeper may block an
unsigned download (`xattr -d com.apple.quarantine <file>`; **untested**).

**Expected result:** a ~131 MiB (nocuda) or ~618 MiB (CUDA) executable.

**Verify:**

```sh
"$HOME/koboldcpp/$ASSET" --version
```

Last line printed: `1.123.1` (this is what the app reads; Server settings…
shows it as "KoboldCpp 1.123.1").

**If it fails:**
- `Permission denied` → `chmod +x` again; check the filesystem isn't mounted
  `noexec` (`mount | grep noexec`).
- The Linux build is a self-extracting launcher; the first run unpacks to a
  temp folder and can take several seconds.
- Running from Python sources instead: the app accepts a command line as the
  binary, e.g. `python3 /opt/koboldcpp/koboldcpp.py` (split on spaces).

## 3.3 Check KoboldCpp answers (no model, no GPU work)

**Goal:** prove the binary runs a server and the endpoint the app checks
works.

**Preconditions:** 3.2; port 5001 free.

```sh
"$HOME/koboldcpp/$ASSET" --nomodel --skiplauncher --quiet \
  --host 127.0.0.1 --port 5001 & KPID=$!
sleep 20
curl -s http://127.0.0.1:5001/api/extra/version; echo
kill $KPID
```

**Expected result:** JSON with `"result": "KoboldCpp"` and `"version":
"1.123.1"` (the app refuses a server whose `result` isn't `KoboldCpp`).

**If it fails:** `Connection refused` → wait longer (first unpack) or check
another program holds the port: `ss -ltnp | grep 5001`.

## 3.4 Check the GPU listing tool

**Goal:** the wizard can list your GPUs and their memory.

The wizard's runtime defaults to CUDA when `nvidia-smi -L` succeeds, else
Vulkan (Metal on a Mac). It runs:

| Runtime | Command the app runs | Package |
|---|---|---|
| Vulkan | `vulkaninfo` | `vulkan-tools` |
| CUDA | `nvidia-smi --query-gpu=index,name,memory.total --format=csv,noheader,nounits` | NVIDIA driver |
| ROCm | `rocm-smi --showproductname --showmeminfo vram --json` | ROCm |
| Metal | none (a share of RAM: ~2/3 up to 36 GB, 3/4 above) | – |

**Verify** (Vulkan):

```sh
vulkaninfo --summary | grep -E '^GPU[0-9]+:|deviceType|deviceName'
```

Each real GPU is listed; `PHYSICAL_DEVICE_TYPE_CPU` entries (llvmpipe) are
ignored by the app; integrated GPUs are listed but not ticked. The `GPUn`
numbers are the numbers KoboldCpp's `--usevulkan n` takes.

**If it fails:** wizard shows `vulkaninfo: … (install it, or set the GPUs by
hand)` → `sudo apt-get install vulkan-tools`, or use **Add a GPU by hand** and
type the memory. `vulkaninfo listed no GPU` → no Vulkan driver for the GPU
(AMD/Intel: `mesa-vulkan-drivers`).

## 3.5 Run the wizard (downloads models, writes the preset)

**Goal:** a server preset with one KoboldCpp per GPU, opened.

**Preconditions:** guide 1 done; a library open (guide 6.1); 3.2 done; enough
RAM (see the plan's RAM line); ~15 GiB disk under the models folder.

Steps in the app:

1. On the servers screen with no presets: **Set up servers…** (with presets:
   **Set up KoboldCpp for your GPUs…**).
2. Keep **KoboldCpp, Qwen-Image set up for your GPUs (recommended)**.
3. **1. KoboldCpp**: Binary (prefilled if found), **Models**: a folder, e.g.
   `~/models`. Files go in `<Models>/koboldcpp/`.
4. **2. GPUs** **through** a runtime (Vulkan / CUDA / ROCm, or Metal on a
   Mac). Tick the GPUs to use. Each row shows its memory and role.
5. With 2+ GPUs, **3. How to use N GPUs**: "Images on every GPU" (default,
   more images at once, PE beside the image model) or "Prompt enhance on a
   GPU of its own". **Prompt enhance on** picks which GPUs run the chat model.
6. Read the plan: a bold summary, one line per GPU ("GPU 1 (24 GB preset):
   …"), why ("23.98 GB is at least 90% of 24 GB (21.6) …"), and the RAM line
   ("Needs about N GiB of RAM for the offloaded weights; M GiB is available
   now").
   Optional: **Choose presets myself** to force a smaller/larger preset.
7. **Download, save as preset and open**.

**Expected result:**

- Files in `<Models>/koboldcpp/` (guide 2.1), `<Models>` added to the model
  folders.
- A model preset "Qwen-Image 2.1 GGUF Q8_0 (CFG 2, refs)" (or `Q4_K`), 20
  steps, CFG 2.
- A server preset named after the GPUs, e.g. "KoboldCpp Vulkan1", saved and
  opened; servers named `kcpp-gpu<N>` on ports 5001, 5002, …; a PE-only GPU
  gets `pe-gpu<N>` on the next port.

What the app runs per image GPU (`KoboldServer::start_args` + the tier's
`image_args`; example: Vulkan GPU 1, 12 GB preset, chat beside):

```text
koboldcpp --host 127.0.0.1 --port 5001 --skiplauncher --quiet \
  --usevulkan 1 --gpulayers 999 \
  --model <Models>/koboldcpp/Qwen3VL-8B-Instruct-Q4_K_M.gguf --jinja \
  --mmproj <Models>/koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf --contextsize 8192 \
  --admin --admindir <state>/koboldcpp/kcpp-gpu1 --adminpassword <random hex> \
  --sdflashattention --sdoffloadcpu --sdtiledvae 512 --sdvaedevice main \
  --sdclipdevice main --sdvramlimit 8192
```

- No chat model on that GPU → `--nomodel` instead of `--model … --mmproj …`.
- CUDA / ROCm: `--usecuda --gpulayers 999` plus `CUDA_VISIBLE_DEVICES` (with
  `CUDA_DEVICE_ORDER=PCI_BUS_ID`) or `HIP_VISIBLE_DEVICES`. Metal:
  `--gpulayers 999` only.
- To load the image model, the app runs the binary with `--exportconfig`
  into the admin folder, then `POST /api/admin/reload_config`. KoboldCpp
  restarts its worker and comes back with the model.
- Presets 8–16 GB **swap**: the app (not KoboldCpp's autoswap) switches
  between the chat model and the image model, ~5 s toward chat, ~9 s toward
  images.
- Started servers log to `<state>/logs/koboldcpp-<name>.log` and stop with
  the app, unless **Leave started servers running** is on. A KoboldCpp
  already answering on the port is used as is.

**Verify** (after the preset opens and a first image was queued, so the
model is loaded):

```sh
curl -s http://127.0.0.1:5001/api/extra/version; echo   # "txt2img": true once the image model is loaded
curl -s http://127.0.0.1:5001/sdapi/v1/sd-models; echo  # the loaded diffusion model file
curl -s http://127.0.0.1:5001/v1/models; echo           # answers: usable as the PE chat server
tail -n 20 ~/.local/state/increart/logs/koboldcpp-kcpp-gpu*.log
ls -l ~/models/koboldcpp/
```

In the app: the top bar's server item is "ready"; **Servers…** shows each
server's state and model.

**If it fails:** see [07-troubleshooting.md](07-troubleshooting.md#koboldcpp).
Most common: the wizard refuses a GPU under 7.2 GiB ("has less than the 8 GB
the smallest preset needs"); a RAM warning (close other programs, or choose a
smaller preset); the image fails with "KoboldCpp didn't load it (see its
log)" → open **Logs…** or the log file above.

## 3.6 Multi-GPU on Vulkan

**Servers the app starts are not affected.** The app picks the card with
`--usevulkan <n>`. Each model switch is an admin reload of a `.kcpps` that
KoboldCpp itself exports from the launch arguments (`--exportconfig`, with
`istemplate: false`), and KoboldCpp only re-picks the backend when it
reloads a *template* (`reload_from_new_args` calls `auto_set_backend_cli`
only when `istemplate` is set). So `usevulkan <n>` holds across reloads.

**A `.kcppt` template you run yourself is affected** (section 3.7):
KoboldCpp's own autoswap reloads the template, re-picks the backend and
drops `usevulkan`, splitting the reloaded model across all cards
([../vram-tiers.md](../vram-tiers.md)). Pick the card with the environment
instead, which reloads inherit:

```sh
GGML_VK_VISIBLE_DEVICES=1 "$HOME/koboldcpp/$ASSET" --config Qwen-Image-2.1-24GB.kcppt
```

**Verify:** `radeontop` / `nvtop` shows memory rising on that card only, also
after the first image (the first reload).

## 3.7 Connecting to a KoboldCpp you start yourself

A preset entry `KoboldCpp { name, run: Connect { url, password, gpu } }`
uses a running KoboldCpp as is. Without admin mode it only runs
the image model it was started with; a run needing another model fails with
"it runs X and can't switch to Y". Example standalone start with a shipped
template (downloads its own models; KoboldCpp's autoswap, unlike the app,
kills an image in progress when a chat request arrives):

```sh
"$HOME/koboldcpp/$ASSET" --config crates/increart-core/kcppt/Qwen-Image-2.1-24GB.kcppt --port 5001
```

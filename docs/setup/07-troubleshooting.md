# 7. Troubleshooting

Each entry: the message as the app shows it (quoted from the source, `{…}`
filled in), the cause, the fix. Find a message with
`grep -rn "<part of the text>" crates/`.

First look at the logs of servers the app started:

```sh
ls -lt ~/.local/state/increart/logs/       # Linux; Windows %LOCALAPPDATA%\increart\logs
# sd-server-<name>.log, koboldcpp-<name>.log, pe-<name>.log
```

In the app: **Servers…** → **Logs…**. It opens a log on its latest launch
(earlier ones from a menu), with each line's time, long runs of one kind
folded, warnings amber and errors red; **errors only** keeps those and the
lines around them, and Ctrl+F finds a word
([screenshot](../gui/98h5-server-logs-folded.webp)). A run's own log, what
the engine did with it (which server loaded its model and why not the
others, the load, each image), is `runs/<run-id>/generation.log` in its
project, and in **Projects → Jobs** when you click the run.

To find what a server does wrong:

- **Validate servers…** on the Servers screen runs the calls the app makes,
  case by case, and says what failed, often with a **Try:** line on what to
  check ([screenshot](../gui/60e4-validate-servers-failed.webp)). Copy report
  gives every request and answer.
- **Test** beside a connected-to server in the preset editor runs the
  engine's calls to it one by one.
- **Log server traffic** in Server settings (or `INCREART_TRAFFIC_LOG=1`)
  writes every request to the image servers and their answers to
  `server-traffic.log`.

To report a bug, **Save for a bug report…** in the Logs window zips the
config and the end of each log, passwords masked.

## Build and launch

| Symptom | Cause | Fix |
|---|---|---|
| build-script error from `yeslogic-fontconfig-sys` / `libwebp-sys` | missing dev packages / C compiler | guide 1.1 |
| `requires rustc 1.95` or newer | old toolchain | `rustup update stable` |
| `increart: unknown argument: …` then usage, exit 2 | bad flag | flags are `--library`, `--servers`, `--no-servers`, `--help` only |
| `increart: --library needs a value` | flag without value | `--library last` or a folder |
| window doesn't open, error about EGL/GL/xkbcommon/display | runtime libs or no display | guide 1.1 run packages; check `$DISPLAY` / `$WAYLAND_DISPLAY` |

## Config

| Symptom | Cause | Fix |
|---|---|---|
| `settings not loaded ({error}); using defaults`, then `the unreadable settings are kept in {file}` | `config.rsn` doesn't parse | fix the syntax in the kept copy (`config.rsn.unreadable`) and put it back while the app is closed; check which file with `echo $INCREART_CONFIG` (guide 1.5). The app saves the defaults over `config.rsn` |
| `resolution "{name}": {w} × {h} is not a multiple of 32, using …` | size not a multiple of 32 | informational; snapped on load |
| `Prompt enhance servers moved into the server presets: …` | old-style `pe: Pe { servers }` config | automatic; the old file is kept as `config.rsn.before-chat-presets` |
| hand-written comments vanished | the GUI rewrites the whole file | expected |

## Library

| Message | Fix |
|---|---|
| `This folder isn't empty: choose another one.` | new libraries need an empty or missing folder |
| `Not an IncreArt library (no library.rsn).` | pick the folder containing `library.rsn`, or **Create a library here instead** |
| `No such folder.` | check the path |

## Servers dialog (any backend)

| Message | Cause | Fix |
|---|---|---|
| `No sd-server binary set` / `sd-server not found at {path}` / `Models folder not found: {dir}` | preset has `Start` sd-servers and settings are incomplete | **Fix…** → Server settings…; paths must exist |
| `{name}: no server answers on {url}` | a `Connect` entry's server isn't running | start it; check `curl {url}/sdcpp/v1/capabilities` (sd-server) or `{url}/api/extra/version` (KoboldCpp) |
| `{name}: can't start {binary}: {error}` | binary missing / not executable | `ls -l` and `chmod +x` the binary; fix the path |
| `{name}: the server exited ({status}) during start-up; log {file}:` + tail | crash at start: wrong `--backend` device, missing libraries, port in use by another program | read the tail; `sd-server --list-devices` for valid names; `ss -ltnp \| grep <port>` |
| `{name}: the server didn't answer, its log quiet for {secs} s (the server start timeout); log {file}` | its log stopped growing before it answered | read the log; slow disk or stuck driver; raise the timeout in Settings → Timeouts if it was only slow |
| servers still running after the app was killed | **Leave started servers running** on, or a leftover from a crash | `pgrep -af 'sd-server\|koboldcpp'`; stop them, or let the next session reuse them (a server already answering on its port is used as is) |
| "No open server can run {preset}: …" when queueing | no open server has the model preset's files | open a preset whose servers have the files (guide 6.4), or fix the preset's file IDs |

## KoboldCpp

| Message | Cause | Fix |
|---|---|---|
| `vulkaninfo: {error} (install it, or set the GPUs by hand)` | `vulkan-tools` missing (or `nvidia-smi` / `rocm-smi` for those runtimes) | install it, or **Add a GPU by hand** |
| `vulkaninfo listed no GPU` / `nvidia-smi listed no GPU` | no driver for that runtime | install the GPU's Vulkan driver (`mesa-vulkan-drivers`) or NVIDIA driver; or pick another runtime |
| `GPU {n} ({x} GB) has less than the 8 GB the smallest preset needs` | card reports < 7.2 GiB | not supported by the presets; use another GPU |
| `GPU {n} ({x} GB) has less than the 8 GB prompt enhance needs` | PE-only card too small | choose a bigger card for PE, or "Images on every GPU" |
| `Leave a GPU for images, or make images on every GPU.` | every GPU ticked for PE in "PE on its own GPU" layout | untick one under **Prompt enhance on** |
| `This KoboldCpp build has Vulkan only: get its CUDA build, or use Vulkan.` | `nocuda` binary with runtime CUDA/ROCm | download `koboldcpp-linux-x64` (NVIDIA) or switch the runtime to Vulkan |
| `Needs about {n} GiB of RAM for the offloaded weights; {m} GiB is available now.` (warning) | pinned weights need RAM | free RAM, use fewer GPUs, or **Choose presets myself** with a smaller preset |
| `GPU {n}: the {x} GB preset peaks at {y} GiB, more than its …` | chosen preset too big | expect out-of-memory; pick Auto |
| `{url}: got {done} of {total} bytes` | download interrupted | run the wizard again (finished files are kept; `.part` files restart) |
| `{url} isn't KoboldCpp` | something else answers on that port | change the port or stop that program |
| `{id} isn't in the model folders` | model preset file missing under `sdserver.model_dirs` | check `ls <models>/koboldcpp/`; re-run the wizard |
| `is in sd-server's int8 format, which KoboldCpp can't load` | int8 convrot file given to KoboldCpp | use the GGUF preset (guide 2.1) |
| `KoboldCpp didn't load it (see its log)`, or the log line, e.g. `Cannot find image model file: …` | reload with the model failed; KoboldCpp went back to launch settings | read `koboldcpp-<name>.log`; usually a path or out of memory |
| `KoboldCpp didn't write its settings: …` | `--exportconfig` failed (old KoboldCpp, bad args) | check `--version` ≥ 1.123; check entry `args` |
| `KoboldCpp didn't restart` / `KoboldCpp didn't come back after loading` | admin reload hung or crashed | read the log; restart the preset |
| `KoboldCpp was started without admin` / `it runs {x} and can't switch to {y} (…)` | `Connect` entry to a KoboldCpp without admin mode | start it with the model the run needs, or let the app start it (`run: Start`) |
| `KoboldCpp returned no image (see its log)` | KoboldCpp answers failures as HTTP 200 with an empty image | read the log (out of memory, no image model) |
| `KoboldCpp takes at most 4 reference images ({n} given)` | KoboldCpp limit | use ≤ 4 refs, or sd-server |
| `KoboldCpp can't take custom sigmas` / `KoboldCpp has no sampler {name}` | recipe uses features KoboldCpp lacks | change the model preset / sampler, or use sd-server |
| `KoboldCpp over https isn't supported` | `https://` URL in a Connect entry | use `http://` (e.g. through an SSH tunnel) |
| second GPU's memory rises with one GPU configured (Vulkan) | reported for KoboldCpp reloads in [../vram-tiers.md](../vram-tiers.md) | guide 3.6 |
| an image is killed when you rewrite a prompt | you run a `.kcppt` with KoboldCpp's autoswap yourself | let the app start KoboldCpp (it swaps without killing images) |
| Long prompts look cut | KoboldCpp truncates prompts to 3000 UTF-8 bytes ([../todo-kcpp-support.md](../todo-kcpp-support.md)) | shorten, or use sd-server |

## sd-server

| Message | Cause | Fix |
|---|---|---|
| `no model is loaded` / `the server is loading or unloading a model` | load still running | wait; `curl …/sdcpp/v1/models/status` |
| `model busy: …` | load requested while jobs pending | wait for jobs |
| `invalid model config: …` | bad file IDs / recipe | compare the preset's files with `GET /sdcpp/v1/model-files` |
| `model load failed: …` | out of memory, corrupt or wrong file | add `--offload-to-cpu` to the entry's `args`; re-check file sizes (guide 2.2) |
| `the server's job queue is full` | > 64 pending jobs | automatic back-off; ignore |
| `job not found (unknown or expired)` | finished jobs are purged after ≥ 60 s | transient |
| model files missing from `model-files` | symlink resolving outside the model dir (rejected by sd-server); wrong folder names; not the first root | copy or move the files into `diffusion_models/`, `text_encoders/`, `vae/` under the first `model_dirs` entry |
| int8 files are very slow | Vulkan build (4.7× slower than HIP measured) | build with HIP on AMD; or use KoboldCpp + GGUF |
| int8 files rejected at load | built with `SD_USE_UPSTREAM_GGML=ON` | rebuild with the default (patched) ggml, guide 4.2 |

## Prompt enhance

| Message | Cause | Fix |
|---|---|---|
| `No chat server in use: add one to a server preset (Servers…), or pick one to continue without sd-servers` | preset has no chat server | guide 5 |
| `{name}: no chat server answers on {url}` | Connect chat server down | start it; `curl {url}/v1/models` |
| `{name}: KoboldCpp exited ({status}) while starting; log {file}` | bad model path, out of memory, bad device flags | read `pe-<name>.log` |
| `{name}: KoboldCpp didn't answer, its log quiet for {secs} s (the server start timeout); log {file}` | the model load stopped printing | read the log; raise the timeout in Settings → Timeouts if it was only slow |
| `{url}: HTTP {status}: …` | server error | read the server's log |
| PE can't describe images | KoboldCpp chat entry without `mmproj` (text only) | set the vision file (guide 5.2) |
| asks are slow while images run | PE shares a GPU with an image server | give PE its own GPU or the CPU |

Still stuck? Ask in [#increart on the KoboldAI Discord](https://discord.gg/7D4jgg6J6Z).

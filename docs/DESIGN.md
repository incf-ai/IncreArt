# IncreArt — Qwen-Image 2.1 batch generation & review

Status: **draft**. Architecture deliberately mirrors IncreMusic-Yue2, a
sibling project for music generation; sections marked "same as Yue2" follow
its design.

Licence: GUI source is MIT. sd.cpp is MIT; model weights carry their own
licences and are never redistributed by this project.

## Proofs of concept

Throwaway prototypes that settle parts of this design. Their decisions are
folded into this document; each has a writeup.

| POC | Scope | Status | Writeup | Code |
| --- | --- | --- | --- | --- |
| POC1 | Whole GUI with mock data and a mock backend: screen structure, both review queues, folder trees and Thunar-like folder views, collections and smart folders, library model (stores, links, IDs, pixel hash), plus an on-disk sample library and verifier | complete (October 2026) | [POC1.md](POC1.md) | `tmp/gui-prototype/` (git-ignored; see the writeup) |

## Goals

- An **input review queue**: ideas (text, images, or both) collected into an
  inbox, triaged with the same keyboard flow, and made into projects.
- Projects containing runs of many seeds, generated across both GPUs.
- Text-to-image **and** edit with reference images (Qwen-Image 2.1 `ref_images`).
- **Transparent (RGBA) output** as a first-class option, preserved end to end.
- **Prompt rewriting** with the official Qwen-Image 2.1 prompt-enhancer model,
  keeping both the original and the rewritten prompt.
- An **output review queue**: fast keyboard review of generated images
  (good / neutral / bad), grid and side-by-side compare.
- Two libraries:
  - **input library**: kept ideas and reference images, reusable as prompts,
    refs or edit sources in any project;
  - **output library**: reviewed generations across all projects.
- Full lineage both ways: idea → project → run → image, and image → refs/parent.
- Any output can be fed back into the input library (as a ref or a new idea).
- Every output is reproducible from the recipe embedded in the file.

Non-goals for v1: mask painting / inpainting UI, LoRA training, upscaling
pipeline (sd-server's `upscale` endpoint can be wired later).

## The two review queues

The app is built around two symmetric queues, each feeding a library:

```
            ┌──────────────── R / I (output → input) ◄───────────────┐
            ▼                                                        │
ideas ──► INPUT QUEUE ──review──► INPUT LIBRARY ──new project (P)──► PROJECT
(text,    (Idea inbox)            (kept ideas,                    │ runs × seeds
 images,                           starred shortlist,             ▼ (both GPUs)
 drop                              ref images)              OUTPUT QUEUE ──review──► OUTPUT LIBRARY
 folder)                                                    (Review tab)            (all projects)
```

- Both queues use the same mechanism: an item's folder is its verdict. Review
  rates with `1`/`2`/`3` (or `G`/`N`/`B`); the Idea inbox has two verdicts,
  save to input library (`1`/`2`/`S`) and discard (`3`/`B`); both take arrows
  and `Ctrl+Z`. (`Space`
  differs: it makes a project from an idea in the Idea inbox and toggles grid / single in
  Review.) (On the
  input side the folder records keep vs. discard; the star is a flag in
  `idea.rsn`, so kept ideas can live in user folders — see Input library
  folders.)
- The output queue (Review) is worked oldest-first by default (the Idea
  inbox grid shows the newest first); the output queue can also be
  filtered to one run. **newest ›** jumps to its last image once;
  **Follow** (in the bar, for the session) moves on through the unreviewed
  images by itself, one at a time, up to the newest, then waits for the
  next (Single, Grid, and Compare's B). A ring beside it counts down to the
  next image; clicking the ring pauses. Its pace (⚙, config
  `review_follow`): **Auto** (default) gives each image as long as one
  takes to make (over the last 10 min) times (lag + 1) / (ahead + 1), so
  it holds `lag` (4) images behind the newest, quicker when further behind
  and slower when closer, kept to 3–60 s; with nothing made lately, or in
  **Fixed**, 8 s an image. Also: start at the newest (else the image
  shown), and hold while the pointer is over the image. The clock stops
  while paused, held, off the Review tab or on the contact sheet. A rating
  within 0.4 s of a move on rates the image it moved from; rating the image
  shown starts the next one's time. → (and End) keep following and restart
  the time; going back or picking another image (←, Home, a click, Swap,
  Go to) turns it off.
- Review's bar has **Generate ›** (Ctrl+G): the project of the image shown
  (the cursor's; on the contact sheet, the selected row's), else the
  scope's project, opened in Generate as it is. Its ▾ opens it with that
  run's settings or that image's recipe & seed instead. The counterpart of
  **Review N new** on the Generate strip.
- Review's scope tree has its own **Sort** (config `review_order`, A–Z by
  default), independent of the Projects tab: Created, Last run and A–Z as
  the project tree orders projects (folders follow the same choice, by the
  projects anywhere in them), or Oldest to review (the oldest projects with
  images to review first, a folder by the oldest such project in it, then
  those with none). Runs and sweeps under a project stay newest first.
- The input queue is global (one inbox). The output queue merges the
  `unreviewed/` folders of Review's scope: all projects (the default), a
  folder of projects, or one project.
- Unreviewed counts show as badges on the Idea inbox and Review tabs; the
  Input library and Output library tabs show how many items they hold (kept
  ideas; kept, i.e. good + neutral, images).
- "Queue" in this document always means a review queue. Pending generation
  work is called **jobs** (see Scheduling and the Jobs panel).

## Backend: stable-diffusion.cpp `sd-server`

One `sd-server` process per GPU, each on its own port, either started by the
app or already running elsewhere and connected to; which one is chosen at
start-up (see Startup: servers). Device names come from `sd-server
--list-devices` and are matched case-insensitively; this project writes them as
`ROCm0`/`ROCm1` (HIP) or `Vulkan1`/`Vulkan2` (Vulkan, where the Intel iGPU is
`Vulkan0`):

```
sd-server --listen-port 1234 --backend ROCm0 --model-dir /path/to/ComfyUI/models --fa --conditioning-cache-size 32
sd-server --listen-port 1235 --backend ROCm1 --model-dir ...
```

### Startup: library

The app always starts by asking which library to open; it never reopens one
by itself, unless the command line says so (see Startup: command line). The dialog lists the recent libraries (`recent_libraries` in
`config.rsn`, most recent first, at most 10; the last used one tagged "last used",
the tag in a muted accent colour): a click opens one, × removes it from the list
(the folder is left alone). With none yet it says what a library is. The
list takes at least the height of three rows, so a short one doesn't leave a
wide, flat dialog.
Opening a library moves it to the top of the list. Release zips ship an
example library in an `Example library` folder beside the executable; it
is put in the list, tagged "example" (on first run, in place of "last
used", so it is the main button), and stays there until removed with ×:
opening other libraries never pushes it out, and a new release's takes
the place of the one before (config `example_library`). Removed, it isn't
offered again (`example_library_removed`). While it opens, a
progress bar shows under the buttons (reading every project, run and idea:
under a second, about 1 s on a cold disk).
The bottom row: on the right the main button, naming the last used one,
**Open increart_library ⏎** (the full path in its tooltip; Enter does the
same; a name past 24 characters is cut with "…"), or with no library yet **Create a new library…**; on the left the
others, **New library…**, **Open existing…** and **Unpack archive…**:

- **New library…** (**Create a new library…**): where should it live? `~/IncreArt`
  (preselected), `IncreArt` in the current folder (when that is elsewhere),
  or somewhere else (typed or **Browse…**). **Create library** makes it;
  the folder is created if missing. A non-empty folder is refused before
  trying; one that is already a library offers **Open it instead**.
- **Open existing…**: libraries found in those places (and not
  in the recent list) to pick, or a folder typed or browsed to. It says
  whether the folder is a library (has `library.rsn`); an empty or missing
  folder offers **Create a library here instead**.
- **Unpack archive…**: as **Unpack a library archive…** from the library menu (see Library
  archives), its progress and Stop under the buttons, a failure said there.

### Startup: command line

Each start-up screen can be skipped from the command line (`increart
--help`):

- `--library <folder>` opens that library; `--library last` the most recent
  one.
- `--servers <preset>` opens that preset (name in any case) and goes on to
  the main window while the servers start, as **Continue in background**
  does; `--servers last` the preset opened last. `--no-servers` continues
  without servers.

A screen is only skipped when its argument is valid: a folder without
`library.rsn`, an unknown or empty preset, or a preset with Start servers
while sd-server or a model directory is missing leaves that screen up, with a
line saying why. `--servers` is used once, when the first library opens.
Unknown arguments print the usage and exit with status 2.

### Switching libraries

The top bar's right end names the open library (**increart_library ⌄**, the
full path in its tooltip). Its menu lists the recent libraries (the open one
ticked), **Open an existing library…** and **Create a new library…** (the
startup wizards), **Show in the file manager**, and the library archives
(below): **Export the library to a zip…**, **… to a tar.gz…** and **Unpack
a library archive…**. Switching keeps the
servers connected and skips the servers screen: drafts are saved, the engine
stops (runs with seeds left continue when their library is opened again; while
runs are generating it asks first), the new library opens, and the engine
restarts on it. Everything else about the old library is dropped (selection,
back / forward history, searches, folder views, prompt enhance's asks, which
are stopped); settings, servers (the chat servers in use and their state
too) and prompt presets stay.

Picking from the wizards, or a library that fails to open, shows the library
screen titled **Switch library**, with **Back to <library>** (`Esc`) to return
to the open one (the engine restarts if it was stopped). There the open
library is tagged "open now" (and can't be removed from the list) and the main
button names the last used other one.

### Startup: servers

When the app starts (after the library is opened) a modal dialog asks for
the session's servers: the sd-servers to generate with and, optionally, the
chat server prompt enhance uses (see Prompt rewriting). One choice covers
both: **presets** saved in `config.rsn` (see Config file → Servers), each a
list of servers that may include chat servers (prompt enhance shares its
asks among them):

- The preset list (the dialog is one column; editing and settings open
  modals). The one used last is tagged "last used",
  the tag in a muted accent colour. Each row shows its servers, e.g. "Start ROCm0 on
  :1234 · Start ROCm1 on :1235 · Chat: Qwen3-VL 8B (GPU)" or "Connect to
  http://10.0.0.2:1234".
  **A click opens a preset**. The bottom row's main button names the last
  used one, **Open ROCm0 only ⏎** (a name past 24 characters cut with "…",
  whole on hover), and Enter does the same; not when the
  dialog was reopened from the main window while servers run (**Close**
  keeps them). Each row has
  ✎ **Edit** and 🗑 **Delete** (which asks first).
- Opening a preset only starts and stops what changed. sd-servers in use
  that it also has (the same entry, and for Start servers the same
  sd-server settings) are kept as they are, model loaded; the others stop
  and the new ones start. When its sd-servers are exactly those in use,
  nothing is opened or restarted at all. Otherwise the engine restarts with
  the new set, dropping the images being made. The chat server likewise:
  one running as the preset's (the same `run`) goes on, else it stops and
  the preset's takes over (started at once if it starts with its preset).
  Mid-session, opening a preset asks first when it would stop sd-servers
  ("Stop ROCm0 and open Other? ROCm1 stay as they are.") or drop images
  being made (and says how many runs are generating); otherwise it opens
  at once. A preset with only a chat server stops the sd-servers in use.
- With no presets, **Set up servers…** opens a wizard. It starts on
  **KoboldCpp, Qwen-Image set up for your GPUs (recommended)**: the
  KoboldCpp binary and a model directory, then the GPUs **through** a GPU
  runtime (Vulkan, CUDA or ROCm; CUDA when `nvidia-smi` runs, else Vulkan;
  on a Mac only Metal)
  as that runtime's tool lists them (`tiers::detect_gpus`: `vulkaninfo`,
  each one's largest device-local heap; `nvidia-smi`; `rocm-smi`, in HIP's
  node order; Metal: one GPU, the share of the RAM macOS lets it keep,
  about 2/3 up to 36 GB and 3/4 above, with no offload in its presets and
  no RAM note, the weights already being in RAM; another runtime lists
  them again, numbered its own way;
  CPU devices left out, integrated ones listed but not ticked, **Add a
  GPU by hand**, its memory typed in). Each ticked GPU takes the biggest
  of the shipped VRAM presets ([vram-tiers.md](vram-tiers.md),
  `crates/increart-core/kcppt/`, read by `tiers.rs`) it holds at 90% (a
  "32 GB" card reports 29.9–31.9 GiB); under 8 GB it can't be used. The
  **Presets** table lists them all with their measured speed, peak VRAM
  and pinned RAM, the ones in use highlighted and tagged with their GPUs;
  below the plan, a line per GPU says why ("23.98 GB is at least 90% of
  24 GB (21.6); the 32 GB preset needs 28.8 GB") and the RAM the pinned
  weights need against what's available. **Choose presets myself** gives
  each image GPU a menu (Auto or a preset; a row's click sets them all);
  a preset bigger than its memory takes, or any under 8 GB, is warned
  about (its peak VRAM against the card's), not refused, and unticking
  goes back to Auto. Prompt enhance's own card takes no preset (it needs
  8 GB) and doesn't set the quantization. One GPU runs images and prompt
  enhance in one KoboldCpp (swapping below 24 GB, see **Swapping**
  below). With two or more, **How to use N GPUs** offers each layout by
  what it gives, naming the GPUs: "2 images at a time — GPU 1: images +
  prompt enhance, GPU 2: images" (the default: prompt enhance beside the
  image model) or "1 image at a time — GPU 1: images, GPU 2: prompt
  enhance only" (a prompt-enhance KoboldCpp started at launch, any other
  GPUs making images too). **Prompt enhance on** ticks its GPUs, each running
  the chat model, the asks shared among them. By default, with images on
  every GPU, it's ticked on each whose preset has room to keep the chat
  model loaded beside the image model (24 GB and up; smaller ones would
  swap); if none has, the biggest. With a GPU of its own, the second
  biggest (ties to the lower number). Ticking by hand sticks until the
  layout changes; the last one can't be unticked. With a runtime other than Vulkan the
  notes say the times were measured on Linux with Vulkan, and warn when the binary
  is a `nocuda` build (Vulkan only). Each GPU row says what it does ("Images + prompt enhance
  (stays loaded)", "… (swaps, ~15 s per image)", "Images", "Prompt
  enhance only, always loaded", "not used"), and a bold summary heads
  the plan ("2 images at a time; prompt enhance always loaded on GPU
  1."). Every image
  server uses one quantization, the smallest preset's, so one
  recipe runs on all. **Download, save as preset and open** fetches the
  files into `<models>/koboldcpp/` (Hugging Face, through `.part` files,
  a broken or cancelled one carried on next time, see Timeouts; one already there kept; one found by name in the model folders or
  Hugging Face's cache (`$HF_HOME`, else `~/.cache/huggingface`, five
  folders down, not `blobs`/`xet`), the same size as Hugging Face's HEAD
  says, linked instead; a progress bar and **Cancel download**), adds
  the folder to the model folders, adds a recipe preset ("Qwen-Image 2.1
  GGUF Q8_0 (CFG 2, refs)", 20 steps, CFG 2), and saves and opens the
  server preset ("KoboldCpp Vulkan1", with " (2)" when taken). The
  binary is checked first (`--version`, when the wizard opens, the field is
  left or a file picked), its version or why it can't run under it
  ("koboldcpp not found: it isn't a file in the working folder, and isn't
  on PATH"); until it runs, that button is greyed out (why on hover) and,
  once it's known not to, **Save anyway** downloads and saves the preset
  without opening it, with a note to download KoboldCpp and pick it or set
  it later in Server settings. Setting the shared binary there moves the
  presets' entries that had the one before along with it. The other
  choice, **sd-server, with your own models**, is the sd-server
  binary and model directory, then the devices from `--list-devices` with
  every GPU ticked (not the CPU) on ports 1234, 1235, …, or a server
  already running to connect to, then optionally **Add a chat server…**
  for prompt enhance (the chat server settings below). **Save as preset
  and open** names the preset after its devices ("ROCm0 + ROCm1", or
  "Running server") and opens it.
- KoboldCpp comes first throughout: the screen's intro names it beside
  sd-server, **Set up KoboldCpp for your GPUs…** (the wizard) sits beside
  **New preset** once there are presets, and preset summaries say which
  server each entry is ("Start sd-server on ROCm0 :1234", "Start KoboldCpp
  on :5001 (and chat)").
- **Server settings…** (bottom right) opens a modal with what every preset
  shares, in three sections. **KoboldCpp**: the binary new KoboldCpp
  entries and the wizard start (`Config::koboldcpp`; empty: the one in use,
  `Config::known_koboldcpp`: prompt enhance's, else a preset entry's; with
  none, found at startup by `kobold::find_binary` on `PATH`, in `~`,
  `~/Downloads`, `~/.local/bin` or a `~/koboldcpp*` folder (`~` is
  `USERPROFILE` on Windows), `koboldcpp` or a release build
  (`koboldcpp-…`/`koboldcpp_…`, with `.exe` on Windows and no extension
  elsewhere), and kept; else `koboldcpp`; the wizard sets it), with its version from
  `--version` (checked when the settings open, the field is left or a file
  picked) or why it can't run. **sd-server**: the binary and the devices
  (`sd-server --list-devices`, **List again**). **Both**: the **Models**
  folders (`sdserver.model_dirs`), a row each, in order: typed or Browse…,
  Show, ↑ / ↓ to search it earlier or later, × to drop it from the list (not
  from disk), and **Add folder…** (browses to another; one already listed
  isn't added twice). The wizard adds its download folder when it isn't
  listed. Both servers search all of them, the first that has a path
  winning (see Model directories); and leave started servers running. **Timeouts** (also a tab in Settings): see Timeouts below. When a preset starts sd-servers and the binary or
  model directory is missing, the dialog says so with **Fix…**, which opens
  it.
- ✎ or **New preset** opens the editor modal: the name, then the server
  rows. **+ KoboldCpp** (first, in the accent colour) adds a KoboldCpp
  image server (port 5001 up, the shared binary); its row has its settings
  under it: started by IncreArt (the shared binary again when switched
  back) or already running, binary, settings file, address, runs on (its
  choice / CPU / GPUs: a runtime, Vulkan / CUDA / ROCm, and the numbers its
  tool lists), more args, or address / password / device,
  and **Chat** to make it one of prompt enhance's chat servers too, with the chat
  model for a started one, labelled **Model**, **mmproj** and **Context** (ticked: the wizard's Qwen3-VL and projector if
  they're in a model folder's `koboldcpp/`, context 8192; see KoboldCpp as
  an image server). Every path (binary, settings, chat model, mmproj, and
  the wizard's binary and model directory) has a Browse… picker starting in
  its folder, else the first model folder. **+ sd-server** adds a device and
  port (1234, 1235, … by default); the app launches it and stops it when it
  exits. **+ Connect to sd-server** adds a URL; connected servers are left
  running when the app exits. With sd-server rows, the listed devices are
  buttons too: a click adds an sd-server on that one (devices already in
  the preset are greyed). A preset can mix all kinds.
  **+ Chat server** opens the chat server settings, starting from the
  presets' first chat server (else KoboldCpp on the GPUs; another beside
  ones the preset has gets a numbered name and the next free port); a
  preset may have several, and prompt enhance shares its asks among them
  and its KoboldCpp entries with Chat. Each row shows the name with **Settings…** and ×. The
  settings window's **Add** / **Save** puts it in the preset being edited;
  it's stored when the preset is saved. A preset may have only a chat
  server. Under the rows, **What each server does** is a grid of the
  preset's servers by role: **Images** (every server but a chat entry;
  each run goes to one whose model list has its preset's files, see
  Scheduling), **Text (prompt enhance)** (the servers its asks are shared
  among; a KoboldCpp that swaps its chat model with images says *not
  asked* when another chat server is there) and **Vision** (whether those
  see the reference images: ✓ with an mmproj, *text only* started without
  one, *assumed* when it can't be told, as for a server connected to; the
  images are sent then). The presets list shows the same in a line under
  each preset ("images: kobold, proxy · text: PE · vision: PE", or "·
  no prompt enhance"), and the top bar's server tooltips say which model
  presets each image server can generate (from its model list) and
  whether prompt enhance's asks carry the images. Esc cancels; a click
  outside doesn't. The editor's buttons:
  - **Save** stores the preset, renaming it in place.
  - **Save and open** stores it and opens it.
  - **Open without saving** opens the edited servers once and leaves the
    preset as it was.
  - **Cancel** drops the edits.
- **Continue without servers** opens a small dialog first: browsing,
  review and the libraries work; Generate queues runs that wait for a
  server. It lists the presets' chat servers (one per process) with
  **None**, the one in use ticked, so prompt enhance can go on without any
  sd-server; **Continue** or **Cancel**. Continuing without from the main
  window by other means (the command line's `--no-servers` aside, which
  has none in use yet) keeps the chat server in use. The first
  Queue of a session with no server (and none starting) warns that the run
  will only wait, with **Set up servers…**, **Queue anyway** and **Cancel**;
  it doesn't ask again until the app restarts.
- While the servers come up the dialog is **locked**: presets, server rows
  and settings are disabled and dimmed ("locked while the servers open"),
  and per-sd-server progress stays readable ("starting — 12 s" followed by the
  server log's latest line, e.g. "294/396 · 4.38GB/s" while tensors load;
  up; or the error with the tail of a started server's log), with the
  chat server's state on a row of its own: it never holds the sd-servers
  up, and its failure doesn't stop the preset opening. sd-servers in use
  that the preset doesn't keep are stopped first ("stopping the servers in
  use first"). Two buttons:
  - **Continue in background**: the main window opens at once; the top bar
    shows each server as "starting… <latest log line>" / "failed", failures are reported in
    the status bar, and the engine starts with the servers that came up when
    the last one has answered.
  - **Cancel**: stops opening, and the servers already started.
- Stopping servers (Cancel, Continue without servers, opening another
  preset) shows each as "◌ stopping…" in the top bar until its process has
  exited and freed its GPU memory; then the status bar says "Stopped
  sd-server gpu0, gpu1: its GPU memory is free".
  Each opening is numbered; reports from a cancelled or replaced attempt
  are ignored (and a server it started after all is stopped).
- If one fails (and you stayed in the dialog) it stays open: retry, change
  the preset, or continue with the servers that did come up.
- **Leave started servers running when IncreArt exits** (`sdserver:
  leave_running`, off by default): started servers outlive the app with
  the model loaded, and the next session's Start finds them on their ports,
  so it is ready at once; the GPU memory stays in use meanwhile.
- Using a preset records it as `last_preset`.
- The top bar's **Servers…** opens a status window: each server's state and
  model, servers still starting, how many runs are queued or generating, and
  **Change servers…** (the dialog), **Jobs** and **Close**.
- Reopened from the main window, the dialog can be left again: **Close**
  (or Esc) goes back keeping the servers in use, and **Continue without
  servers** becomes **Stop the servers, continue without** while servers
  are in use.
- Started servers (sd-server, KoboldCpp) stop with the app even when it
  dies without stopping them (killed, crashed), unless **Leave started
  servers running** is on (`increart-core/src/process.rs`). Linux: the
  kernel sends them SIGTERM when it goes (`PR_SET_PDEATHSIG`, asked for by
  a spawning thread that lasts as long as the app); that reaches only the
  server itself, so for a server in its own process group (KoboldCpp) a
  small shell watcher with the same signal stops the whole group (TERM,
  then KILL after 5 s). Windows: they join a
  job object that kills them when the app's handle to it closes. macOS: a
  small shell watcher per server sends SIGTERM once the app is gone
  (checked every second). On Windows no console window opens for a server
  or a helper command.
- A **Start** server finds an sd-server already answering on its port (e.g.
  one left running by an earlier session) and uses it instead of failing on
  a port in use.

Only the native async API (`/sdcpp/v1/...`) is used:

| Use | Endpoint |
| --- | --- |
| Health / features | `GET capabilities` |
| Model catalog | `GET recipes`, `GET model-files`, `POST models/refresh` |
| Load / status | `POST models/load {recipe, models:{slot: file_id}}`, `GET models/status` |
| Submit | `POST img_gen` → 202 `{id}` |
| Poll + preview | `GET jobs/{id}` (status, `preview.step/total_steps/b64_json`, `result`, `error`) |
| Cancel | `POST jobs/{id}/cancel` |

Errors to handle explicitly:

- `img_gen`: 503 `model_not_loaded` / `model_loading` (wait or trigger load),
  400/422 (request bug → fail the seed, keep the run), 429 (queue full →
  back off).
- `models/load`: 409 `model_busy` (jobs still pending on that server), 400
  `invalid_model_config` (previous model stays loaded), 422 `model_load_failed`
  (server is left with no model). The load request blocks until done; poll
  `models/status` from elsewhere for progress.
- Jobs: a cancelled generating job stays `generating` with
  `cancel_requested: true` until it reaches a checkpoint; keep polling until
  `cancelled`. Finished jobs are purged after a TTL (≥ 60 s), so results must
  be fetched as soon as a job completes. At most 64 jobs may be pending per
  server.

Model file IDs are opaque; the GUI resolves them from `model-files` and stores
both the ID and the file's content hash in the recipe.

Default Qwen-Image 2.1 selection (existing ComfyUI tree):

- `diffusion_model`: `diffusion_models/qwen_image_2.1_int8_convrot.safetensors`
- `llm`: `text_encoders/qwen3vl_8b_int8_convrot.safetensors`
- `vae`: `vae/qwen_image_2.1_vae_bf16.safetensors`
- `tae` (optional, for previews): `taeqi2_1.safetensors` from
  `madebyollin/taeqi2_1`, not downloaded yet. It also supports RGBA.

`llm_vision` is only needed with a separate GGUF text encoder. The safetensors
encoder includes the vision tower: confirmed 2026-10-24 with a three-ref
`ref_images` job (see Measurements).

Referring to reference images in prompts: for Qwen-Image 2.1, sd.cpp puts
each ref into the encoder prompt as `<image1><|vision_start|>…<|vision_end|>
<image2>…` (in `ref_images` order) before the user's text, so prompts name
them **`<image1>`, `<image2>`, …** ("Picture 1" is the Qwen-Image-Edit
2509/2511 convention, "Image 1" Mage-Flow's). The Generate panel's ref
chips show these tags; a chip's **T** button puts its tag in the prompt at the
cursor (replacing a selection; at the end if the prompt was never focused),
spaced from the words around it, and returns to the prompt after it. Any
prompt template or PE rewrite must keep them.
For the text encoder a transparent ref is composited onto white; the
diffusion side gets the VAE-encoded ref (alpha handling there unverified).

Built-in recipes:

| Recipe | Diffusion weights | Steps | Sigmas | CFG |
| --- | --- | --- | --- | --- |
| `qwen-image-21` | base (above) | 20 | resolution-dependent flow schedule | recipe default 1; sd.cpp example uses **6.0**. Pick the preset value by A/B test. |
| `qwen-image-21-turbo` | **separate Turbo weights** (`Qwen/Qwen-Image-2.1-Turbo`, not downloaded yet) | 8 | fixed `qwen-image-2.1-turbo` sigmas | 1.0 |

The GUI's presets set CFG explicitly rather than inheriting the recipe default.
Switching between base and Turbo changes the model selection and therefore
triggers a load (see Scheduling). Dimensions are snapped to multiples of 32;
the recipes default to 2048², but the GUI's default is 1024² (see measurements
below).

### KoboldCpp as an image server

A server preset can have **KoboldCpp** entries in place of (or beside)
sd-servers (**+ KoboldCpp** in the preset editor). The engine drives one as
it drives an sd-server: a client (`kobold_image`) puts KoboldCpp's A1111-style
API behind sd-server's job calls, so scheduling, previews, cancel and recipes
work the same.

```rsn
Kobold(KoboldServer {
    name: "kcpp0",
    run: Start {
        binary: "/opt/koboldcpp/koboldcpp",  // or "python3 …/koboldcpp.py"
        // config: "…/qwen-image.kcpps",  // from KoboldCpp's launcher; what follows goes over it
        port: 5001,
        device: Gpu { devices: [1] },     // as for prompt enhance (runtime: Vulkan unless set); missing: KoboldCpp's choice
        // args: ["--sdflashattention"],
    },
    // chat: Some(KoboldChat { model: "…/Qwen3VL-8B-Instruct-Q4_K_M.gguf", mmproj: "…", context: 8192 }),
}),
// Kobold(KoboldServer { name: "box", run: Connect { url: "http://10.0.0.2:5001", password: "", device: "Vulkan1" } }),
```

- **Started** (`Start`): launched with no image model (`--nomodel`, or the
  chat model) and admin mode on: `--admin --admindir
  <state>/koboldcpp/<name> --adminpassword <made once, kept there>`, so a
  server left running from an earlier session still takes the app's
  reloads. It logs to `logs/koboldcpp-<name>.log` (**Logs…**), runs in its
  own process group, and stopping it stops the group (its admin mode runs
  the server as a child process it restarts).
- **Loading a model**: the run's model files are looked up under the
  server settings' model folders (the same recipe presets serve both)
  and given as `--sdmodel` (`diffusion_model`), `--sdllm` (`llm`), `--sdvae`
  (`vae`), `--sdclip1` (`clip_l`), `--sdclip2` (`clip_g`, `t5xxl`); other
  slots can't be loaded. KoboldCpp writes the settings itself
  (`--exportconfig`, the launch arguments plus the files, about 1.5 s) into
  the admin folder, and `POST /api/admin/reload_config` restarts its worker
  with them (it answers for a moment, goes, comes back when loaded). Loaded
  means it then reports `txt2img` and that diffusion model. If the load
  fails KoboldCpp goes back to its launch settings; the error says why,
  from its log ("Cannot find image model file: …"). A chat model running
  beside it restarts too (asks in flight fail).
- **Connected** (`Connect`): used as it runs. A run whose diffusion model
  is the one it runs generates there; any other can't load ("it runs X and
  can't switch to Y"), and the scheduler leaves those runs to other
  servers.
- **Generating**: `POST /sdapi/v1/txt2img` (`img2img` with an init image and
  `denoising_strength`), which answers with the image; jobs wait in the
  client and go one at a time. Prompt, negative, size, seed, steps,
  `cfg_scale`, `sampler_name`, `scheduler`, `flow_shift`, and the refs as
  `extra_images` (at most 4) are sent, with `remove_limits` and step / CFG
  limits above the run's, so nothing is clamped. What it can't do fails the
  seed with the reason instead of being dropped: custom sigmas, more than 4
  refs, a sampler the model doesn't list (KoboldCpp would quietly use its
  default). Seeds are taken modulo 2³² (as KoboldCpp does).
- **Progress**: `GET /sdapi/v1/progress?genkey=<job>` while generating, with
  the preview image when previews are on (its kind is KoboldCpp's launch
  setting).
- **Cancel**: a waiting job goes at once; a generating one is stopped by
  closing its connection (KoboldCpp aborts an image whose client goes; it
  has no abort by key for images).
- **Recipes** record `koboldcpp <version>` (from `/api/extra/version`) where
  an sd-server's record its sd.cpp commit, and the entry's device
  (`Vulkan1`, `CUDA0`, `ROCm0,1`, `CPU`, or as configured).
- **Chat too**: with `chat`, it is prompt enhance's chat server when its
  preset opens (unless the preset has a chat entry of its own), asked at its
  address as a Connect chat server. A started one runs the chat model
  beside the image model (`--model --mmproj --contextsize --jinja`).
- **Swapping** (`swap`, set by the wizard for 8–16 GB cards): the chat
  model and the image model take turns on the card, one loaded at a time.
  The app does the swapping, not KoboldCpp's autoswap: autoswap reloads
  on the request path and kills an image in progress when a chat request
  comes. Each chat ask (`pe::Client` through `kobold_image::chat_turn`)
  waits for the image being made, reloads the launch settings (the chat
  model, no image model), and holds its turn until answered; images wait
  while asks are waiting (asks first: they're interactive), then reload
  the image settings (exported without the chat model). A swap costs ~5 s
  toward chat and ~9 s toward images.
- Checked against KoboldCpp 1.123.1's source and, on the CPU, a real
  KoboldCpp (`INCREART_KOBOLDCPP=… cargo test --test real_koboldcpp`:
  launch, settings written, a failing reload recovering with its reason,
  stopping). Not yet checked on a GPU: Qwen-Image's refs and quality,
  previews, cancel by closing, and that a seed gives the same image again.

## Transparency (RGBA)

Qwen-Image 2.1 decides whether to output alpha from the **prompt**; there is no
API switch. The documented format is:

> `This is an RGBA image with transparency. <description>. The image has alpha channel and the background is transparent.`

- **No Transparent toggle** (removed 2026-10-25). Ask for transparency in the
  prompt itself, e.g. "… remove the background and make it transparent". The
  prompt is sent exactly as typed; older runs keep their recorded `alpha: true`
  and the prompt that was sent.
- Why: the wrapper above breaks edits. With a reference image (`<image1>`) it
  made the model return an empty, fully transparent canvas (alpha ≈ 1/255
  everywhere, a flat purple underneath) for every edit prompt tried, while the
  same edit asked for in plain words ("make it transparent") worked. It does
  work for text-to-image, but one way of asking is simpler than two.
- Output format is always PNG (alpha survives; never JPEG). `ref_images` and
  `init_image` are sent with their native channels, so RGBA inputs keep their
  alpha.
- Review, library thumbnails and compare draw images over a checkerboard,
  toggleable to a solid colour. Thumbnails are WebP with alpha.
- Library filter and smart folder: `transparent`. It must test the pixels,
  not the channel count: Qwen-Image 2.1 returns RGBA PNGs for every prompt,
  and ordinary images come back with alpha 253–255 throughout (measured, see
  below). An image counts as transparent when at least 5% of its pixels have
  alpha below 200. It is worked out once per store image, when it is
  written, imported or first indexed, and kept in `cache/pixel-index.rsn`
  (not with the thumbnail: thumbnails are made lazily, so counts would be
  wrong until every image had been shown). The run's recorded `alpha` is
  history only; nothing filters on it.

## Prompt rewriting

Prompt enhance (PE) runs **Qwen3-VL-8B-Instruct** (GGUF: `Q4_K_M` plus the
`Q8_0` mmproj for vision) in **KoboldCpp**, through its OpenAI-compatible
`/v1/chat/completions`. It's the same model the image presets use as their
text encoder, so one download serves both. Any other OpenAI-compatible chat
server can be connected instead (`Connect`).

- sd-server cannot run chat models, so PE is a **separate, optional
  service**: a chat server entry in a server preset (see PE server and
  requests below). The app starts it on demand (or when the preset opens),
  or connects to one already running.
- Qwen also ships fine-tuned Qwen3.5-9B prompt-enhancer models for
  Qwen-Image 2.1 (text-to-image and edit). They aren't used yet; PE runs
  the general Qwen3-VL model with IncreArt's own system prompts (see Open
  questions).
- The system prompts are built in (`pe.rs`) and can be overridden in
  config, per job.
- GPU placement: the chat server runs on a GPU of its own, shares one with
  an image server (KoboldCpp's swap mode takes turns on cards too small for
  both, docs/vram-tiers.md), or runs on CPU. Rewriting is a short,
  interactive step, so CPU is acceptable.
- Where it's used:
  - Idea inbox / input library: `W` (or Rewrite in a card's menu) queues the
    idea for the PE server; rewriting is asynchronous (seconds to a minute,
    possibly on CPU), so nothing blocks. The idea's card shows the state
    ("queued for rewrite", "rewriting… 40%", "rewrite ready — open to
    review"). Opening the idea (Edit idea window) shows the
    original-vs-rewritten diff with the added words highlighted: **Accept**
    stores the result as a variant alongside the original (both are kept),
    **Reject** drops it. Card right-click menus offer **Accept all ready
    rewrites (N)**. The top bar's PE status shows the server state, with the
    queue in its tooltip; clicking it opens the server status (the
    **Servers** modal), which has a Prompt enhance section.
  - New idea form: **Rewrite with PE** suggests a rewrite of the draft, to
    use as the text, keep as a variant, or discard.
  - Generate panel: a **Rewrite** button (T2I or i2i model, depending on
    whether refs are attached; the i2i model also receives the ref images)
    with a diff view of original vs rewritten before accepting.
- Generation always records `prompt_original` and `prompt_sent`. Rewriting is
  never automatic at generation time, so a run's prompt is fixed and
  reproducible.

### PE server and requests

Built so far: **KoboldCpp** (or any OpenAI-compatible chat server), with
vision, suggesting values for a sweep's blanks, rewriting the Generate
prompt and ideas' text, and describing images. Still to come from the plan
above: queued rewrites from idea cards (`W`, card states, Accept all ready
rewrites).

- Config (`config.rsn`): a chat server is an entry of a server preset
  (Config file → Servers), at most one per preset; the system prompts and
  sampling are shared by every chat server:
  ```rsn
  ServerPreset { name: "Both GPUs", servers: [
      Start { name: "gpu0", port: 1234, device: "ROCm0" },
      Chat(PeServer {
          name: "Qwen3-VL 8B (GPU)",
          run: KoboldCpp {
              binary: "/opt/koboldcpp/koboldcpp",  // or a command line: "python3 /opt/koboldcpp/koboldcpp.py"
              // config: "…/qwen3vl.kcpps",  // settings saved from KoboldCpp's launcher; what's below goes over it
              model: "…/Qwen3VL-8B-Instruct-Q4_K_M.gguf",  // may be left out with a config
              mmproj: "…/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf",  // vision; empty: text only
              host: "127.0.0.1",
              port: 1240,
              device: Gpu { devices: [1] },  // or Cpu { threads: 16 }; missing: Auto (KoboldCpp's choice)
              context: 8192,                 // --contextsize; missing or 0: KoboldCpp's default
              // args: ["--blasthreads", "24"],  // more arguments, last
          },
          start: AtLaunch,  // missing: OnDemand (when first asked)
      }),
      // Chat(PeServer { name: "…", run: Connect { url: "http://127.0.0.1:5001" } }),
  ] },
  pe: Pe {
      // system_prompts: { "rewrite": "…" },  // fill_blanks / style_prompt / rewrite / describe; empty or missing: built in
      // sampling: { "fill_blanks": Sampling { temperature: 1.1, min_p: 0.05 } },
  },
  ```
- Older configs listed PE servers under `pe: Pe { server, servers }`, each
  with its own system prompts and sampling. Loading one moves them: the one
  used (`server`, else the first) into every preset without a chat server,
  its system prompts and sampling becoming everyone's, and each other one
  into a new preset of its own (named after it); the file is saved so at
  once, the old one kept beside it as `config.rsn.before-chat-presets`, and
  the status bar says what moved.
- The chat server in use is the open preset's, or the one picked to
  continue without sd-servers; none until then.
- `device`: `Cpu { threads }` adds `--usecpu [--threads N]`; `Gpu { devices,
  runtime }` puts every layer on the GPUs (no ids: every GPU, the model split
  across them) through `runtime` (`Vulkan` when missing, so older configs
  read as before): `Vulkan` adds `--usevulkan [ids…] --gpulayers 999`;
  `Metal` (KoboldCpp's Mac build, which has no Vulkan) only `--gpulayers 999`;
  `Cuda` and `Rocm` add `--usecuda --gpulayers 999` and pick the GPUs with
  `CUDA_VISIBLE_DEVICES` / `HIP_VISIBLE_DEVICES` (`--usecuda` takes one id
  or all), CUDA with `CUDA_DEVICE_ORDER=PCI_BUS_ID` so its numbers are
  `nvidia-smi`'s. The ids are the runtime's own numbering, listed by
  `vulkaninfo --summary` (software renderers left out), `nvidia-smi`, or
  `rocm-smi` (by node, as HIP numbers them). CUDA and ROCm need KoboldCpp's
  CUDA build or a ROCm build (koboldcpp-rocm); its `nocuda` build has
  Vulkan only. On this machine Vulkan's 0 is the Intel iGPU and 1, 2 the
  R9700s (03:00.0, 06:00.0), ROCm's 0, 1 the same R9700s, and one
  GPU is quicker than a split. Measured on 13 blanks: about 0.5–1 s an ask on
  one R9700, against 3–7 s on 16 CPU threads; it takes about 7.2 GB of the
  card beside sd-server's (the 8B Q4 model 4.5, its 8192-token cache 1.2,
  the vision projector 1.1, working memory 0.5).
- `start`: `OnDemand` starts it on the first request; `AtLaunch` when its
  preset opens (or it's picked to continue without sd-servers), so the
  first ask doesn't wait for the model to load. A preset's chat servers in
  use change together: ones no longer used stop first (they hold memory,
  and may have the port a new one wants), ones that run the same keep
  running.
- **Chat server** settings (a preset's chat row **Settings…**, or **+ Chat
  server**, in the preset editor and the setup wizard): name, KoboldCpp or
  Connect, binary / settings / model / vision files (Browse…), address and port,
  **Runs on** GPU (a **Runtime**, Vulkan / CUDA / ROCm, and the GPUs it
  lists ticked; none: every GPU) / CPU (threads)
  / KoboldCpp's choice, context, more args, and **Starts** when first asked
  / when the preset opens. **Add** / **Save** checks the name is set and
  the files needed are named (the binary, and a model or a settings file),
  and puts it in the preset being edited; it's
  stored when the preset is saved, and a change to how the one in use runs
  takes effect when the preset is opened again (it restarts then). The
  server status window's PE row has the chat server in use, its state,
  **Start now** / **Connect** when off or failed (loads the model now) and
  **Stop** while running (frees its memory; asks waiting are dropped; one
  stopped while starting is Off, not Failed). Asking with no chat server
  in use says to add one to a preset (Servers…); the Generate panel's
  prompt-enhance hints have a **Servers…** button for it.
- `KoboldCpp` is started as `start` says (`--config` first, so the rest
  overrides it, then `--model --mmproj` when set, `--host --port
  --skiplauncher --quiet --jinja`, the `device` and `context` flags, then
  `args`). `binary` is a file, or else a command line split on spaces (a
  Python install: `python3 …/koboldcpp.py`). It logs to
  `logs/pe-<name>.log`; one already answering on that port is used as is. It
  gets up to 5 minutes to load, and is stopped with the app. Its Linux build
  is a launcher running the server as its child, so it runs in its own
  process group, and stopping it stops the group (TERM, then KILL after 5 s).
  It is also started with the parent-death signal (`PR_SET_PDEATHSIG`,
  SIGTERM) plus the group watcher above, since the launcher doesn't pass the
  signal on to everything it started (in admin mode the model process
  outlived it), so a killed or crashed app doesn't leave it running. The app itself closes as its close button does on
  SIGTERM, SIGINT or SIGHUP (drafts saved, servers it started stopped).
  Changing how it runs (`run`) starts it afresh on the next request; its
  system prompts and sampling don't (they go with each request). Starts
  and stops go through the same queue as the requests, in order, so a
  restarted server's port is free before the new one starts.
  `Connect` only checks the server answers (`/v1/models`).
- Requests wait in one queue shared by the chat servers in use: each has a
  worker that opens it (all at once) and then takes the next ask when free,
  so N servers answer N asks at a time; `PeOn` says which took an ask
  (shown as "… on <name>" with more than one). The status is Ready once any
  is (the PE row reads "A + 1 more: 1 of 2 ready" until all are), Starting
  while all are, and Failed (the asks dropped) once all failed; one failing
  beside others that work is reported as a problem and the rest go on. A
  stop drops asks still queued (or on their way there) and cuts short the
  one being answered. Each ask is one chat
  completion each (`/v1/chat/completions`, at most 300 tokens for blanks,
  50 per value asked when more, and 700 for text; a fresh connection each,
  as KoboldCpp closes one after answering). Each carries the job's
  **sampling**, per job like the system prompts (temperature
  and `min_p`, a slider each under the job's system prompt, with Default):
  by default 1.1 / 0.05 for blanks, for variety kept sensible, and 0.9 / 0
  for rewriting and describing. Each also carries a fresh random `seed`:
  without one KoboldCpp can answer the same ask word for word. Images are scaled to at most 512 px for blanks (their
  gist is enough; Qwen3-VL reads one as ~180 tokens rather than ~400) and
  768 px for rewriting and describing, and sent as PNG `image_url` data
  parts *before* the text, and a blank's message has what every blank's
  ask shares first (the images' line, the template) and its own part last
  (its styles, values offered, instructions): KoboldCpp doesn't read again
  the start a request shares with the one before, so the asks of a sweep
  read its images once (on CPU, ~8.5 s an ask saved; 13 blanks went from
  200 s to about 80 s); a transparent one is
  flattened over a checkerboard first (models read the colour of fully
  transparent pixels as if it were there). A KoboldCpp without `mmproj`
  gets no images, and can't describe.
- **System prompts** are per job (`fill_blanks`, `rewrite`, `describe`),
  shared by every chat server, with a built-in default for each. The
  server status window's Prompt enhance section shows the chat server in
  use, how it runs, its state, and each job's system prompt in a
  collapsed section to edit (the default until changed, "(changed)"
  otherwise; **Default** puts it back; an edit equal to the default is
  stored as none).
- Filling blanks: the system prompt asks for only a JSON array of short,
  concrete, varied values that read in place of the blank, looking at the
  images. The message gives the template, the style prompt(s) using the
  blank ("one value is used in all N of these styles" for a shared one),
  the blanks already filled, which images are attached (`<image1>`…, then
  the start image) and asks for 6. The answer's JSON array is used, else
  its lines (bullets and numbers dropped); repeats and empties are dropped.
- **Rewriting** (Generate's **Rewrite**, **Rewrite with PE** in Edit idea
  and the New idea form): the system prompt asks for a clearer, more
  specific and visual prompt keeping the user's intent, `<imageN>` tags,
  `{blanks}` and `{preset}`, and only the prompt back. The message gives
  the text (a sweep template says what `{preset}` is), which images are
  attached (Generate: the references, then the start image; an idea: its
  images) and the instructions, if any. The answer is cleaned (code fences,
  a "Rewritten prompt:" label and quotes around it all are dropped).
- **Describing** (**Describe with PE…** in any image's right-click menu,
  next to Copy image): the image alone, asking for a one-paragraph
  description as a prompt (subject, composition, setting, lighting,
  colours, materials, style; text in it quoted).
- Both open the **rewrite window** (not modal; the button shows "…" while
  it waits): **Instructions** ("more cinematic, shorter") with **Rewrite
  again** / **Describe again**; for a rewrite, **Changes**, a word diff of
  the original against the answer (removed words struck through in red,
  added ones green); the answer to edit (**As answered** undoes edits),
  warning about `<imageN>` tags or `{blanks}` it lost. **Use as the
  prompt** / **Use as the text** / **Use as the Generate prompt** replaces
  the text, its old version kept in its history ("before rewrite");
  **Keep as a variant** (Edit idea) adds it to the idea's variants with
  origin `Rewritten { model: <server name>, system_prompt_hash }` (the
  first 12 hex digits of the system prompt's SHA-256), leaving its text;
  using it on an idea closed meanwhile also keeps it as a variant.
  **Copy**, **Discard**. A newer ask replaces an older one: its answer, or
  one coming after the window closed, is dropped.
- **Progress**: answers are streamed (`"stream": true`; Server-Sent Events,
  about one token a piece; a server answering all at once works too). Each
  ask goes through queued ("queued, 2 ahead", or the server's start with
  its time), preparing the images, reading the prompt and images (the time
  so far and how long it took last time with that many images), and
  writing ("~84 tokens, 6.2/s"), reported at most 4 times a second (the
  rate from the first second on, worked out at most twice a second). The
  rewrite window shows the step with **Stop**, and the answer and its diff
  as they're written. The top bar's PE item shows the number of asks and
  the first one's step, its tooltip every ask in order; the servers list's
  PE row (its device: "GPU 1", "CPU", or "chat" for one connected to; on a
  GPU an sd-server also uses, its asks slow the images down) and the
  status window show the same state ("no chat server", "starts
  when asked", "starting… N s", "ready", "working (N): …", "failed"); a
  failure also goes to the messages.
- **In Jobs**: Projects → Jobs has a **Prompt enhance** section between the
  servers and the runs (while a chat server is set up; "Nothing asked."
  when idle), and the status bar's jobs popover the same while asks wait. A
  block per batch, not per ask: **Suggest values**, **Enhance style
  prompts** (both with their project), **Rewrite the prompt** / **Rewrite
  the idea** (its title) / **Describe the image**; Stop first (it stays put
  as the text changes), "4 of 41", the step of the ask being worked on, a
  bar over the batch, and every ask in the tooltip. Clicking the name goes
  where it was asked: the Generate form (blanks, styles, the prompt's
  rewrite) or the rewrite window brought to the front. The status bar's
  indicator adds "PE 4 / 41" over every batch ("· PE 4 / 41" after the runs;
  alone, instead of "no jobs"), its bar staying the images'. Asks aren't
  runs: not saved, not in the engine's queue or its order, no Job details.
- **Stopping** an ask (Stop, Discard or closing the rewrite window, a new
  ask from the same window, a blank's spinner): queued, it's dropped;
  being answered, the app stops reading it and asks KoboldCpp to stop
  (`/api/extra/abort`), so the next ask doesn't wait behind it. Asks are
  numbered; an answer to one stopped is dropped.
- Measured with Qwen3-VL-8B-Instruct Q4_K_M on CPU (16 threads): loading
  takes about 60 s; a request with one image about 18 s (the image takes
  ~13 s to encode), without one a few seconds.

## Caching

Three separate caches are involved; only the first is worth tuning:

| Cache | What | Where / lifetime | Cost if missed |
| --- | --- | --- | --- |
| Conditioning (sd.cpp) | Text-encoder output per prompt + refs | LRU in host RAM, per server, `--conditioning-cache-size N` (default 4); lost on restart or model reload | Full Qwen3-VL encode (the first encode in the log took 50 s, including lazy weight load) |
| Prefix (sd.cpp, Qwen-Image 2.1) | Per-layer K/V of text + ref tokens | VRAM, **one sampling run only** (released at the end) | About one extra step of work per image (~1.5 s of ~31 s) |
| Thumbnails (IncreArt) | `thumbs/{input,output}/<image-id>.webp` | Disk, deletable | Re-decode |

Decisions:

- **Raise the conditioning cache** to 32 entries per server. Each entry is
  only the encoder output (a few MB), so the RAM cost is small. Seeds of one
  run share an entry. Interleaving several runs and A/B variants needs more
  than the default 4.
- **Disk persistence** is not supported by sd.cpp today. The useful target is
  the conditioning cache (small, expensive to recompute, lost on restart).
  Plan: an upstream patch adding `--conditioning-cache-dir`. Key = hash of
  model file hashes + prompt + ref image hashes + encoder params; value =
  serialized `SDCondition`. Until that lands, the scheduler keeps runs sticky
  to a server (see below) to get the most out of the in-RAM cache.
- Persisting the **prefix cache** to disk is not planned. It is 0.5–4 GiB per
  condition, and reading it back would cost about as much as the single step
  it saves. `qwen_image_2_1_prefix_cache_type=q8_0` is an option if VRAM is
  tight.

## Crates (same split as Yue2)

- `increart-core` — config (RSN), server launcher/attacher, sd-server client,
  PE (chat) client, scheduler, image I/O + metadata, library/filesystem
  layout, history.
- `increart-gui-core` — `AppState` + pure `update(state, msg) -> effects`
  reducer; all behaviour tested headlessly.
- `increart-gui` — eframe/egui view; thin, no logic.

## File format: RSN

Every structured file IncreArt writes (`config.rsn`, `library.rsn`,
`idea.rsn`, `project.rsn`, `run.rsn`, `cache/pixel-index.rsn`) and the
`increart:recipe` PNG chunk use [RSN](https://github.com/khonsulabs/rsn)
(the `rsn` crate, a Rust-like object notation) through serde. It reads like Rust
literals: `Name { field: value }` structs (the name is optional when read,
and checked when present), unit and struct enum variants (`Generate`,
`Edit { init: ImageRef { … }, strength: 0.75 }`), `[…]` lists, `{ "key": value }`
maps, `//` comments. IncreArt writes 4-space indentation and `Option`
values bare (`source_idea: "…"` or `None`); `Some(…)` is also accepted on
read. Unknown fields are ignored on read, so older builds can open files
written by newer ones. The one exception is
`increart:source`, which is JSON (it describes a foreign file and may be
read by other tools).

## Config file

User settings live in one RSN file, `config.rsn`, in the app's config
folder: `$XDG_CONFIG_HOME/increart/` when that is set, else
`~/.config/increart/` on Linux, `%APPDATA%\increart\` on Windows and
`~/Library/Application Support/increart/` on macOS. It
holds machine and user preferences, not library data, plus the recent
libraries (see Startup: library). Older configs' single `library` entry is
read into that list, and their PE servers into the server presets (see
Prompt rewriting → PE server and requests). The GUI rewrites the whole file when a setting is saved, so hand
comments are not preserved.

### Servers

```rsn
Config {
    server_presets: [
        ServerPreset { name: "Both GPUs", servers: [
            Start { name: "gpu0", port: 1234, device: "ROCm0" },
            Start { name: "gpu1", port: 1235, device: "ROCm1" },
        ] },
        ServerPreset { name: "Remote box", servers: [
            Connect { name: "box", url: "http://10.0.0.2:1234", device: "ROCm0" },
            Chat(PeServer { name: "Box chat", run: Connect { url: "http://10.0.0.2:5001" } }),
        ] },
    ],
    last_preset: "Both GPUs",
    sdserver: SdServer {
        binary: "/opt/sdcpp/build-hip/bin/sd-server",
        model_dirs: ["/path/to/ComfyUI/models"],
        args: ["--fa", "--conditioning-cache-size", "32"],
        preview: "none",
        commit: "fd21ecf",
    },
    resolutions: [ … ],
}
```

- `Start` servers take `host` (default `127.0.0.1`), `port`, `device` and
  optional per-server `args` (e.g. `--offload-to-cpu`). The command line is
  `--listen-ip host --listen-port port --backend device`, then a
  `--model-dir` per `model_dirs` entry, then `sdserver.args`, then the
  server's own `args`. Output goes to `sd-server-<name>.log` in the app's log
  directory.
- **Model directories** (`model_dirs`, edited in Server settings and Model
  presets): a model
  preset names each file by its path inside the folder that holds it
  (`vae/x.safetensors`), the first folder that has the path winning, as
  KoboldCpp searches them. sd-server names a file outside its first
  `--model-dir` `@<folder hash>/<path>`: loading a model looks each path up
  in its `model-files` (the first folder's file, else another's at that
  path) and sends its name; what it reports loaded, and what it lists as
  loadable, are read back without the `@<hash>/`. Runs record each file's
  SHA-256 from whichever folder holds it.
- `Connect` servers take a `url`; `device` is optional and only recorded in
  recipes.
- `Chat` is the preset's chat server for prompt enhance, at most one (see
  Prompt rewriting → PE server and requests); it isn't an sd-server and
  generates nothing.
- `sdserver.preview` is the preview mode sent with every job (`none`,
  `proj`, `tae`, `vae`); `commit` is recorded in recipes as `sdcpp`.

### Resolutions

The Generate panel's size list comes from `resolutions` in `config.rsn`. One
list is shared by every recipe, base and Turbo alike.

```rsn
Config {
    resolutions: [
        Resolution { name: "Square",        width: 1024, height: 1024, default: true },
        Resolution { name: "Square L",      width: 1344, height: 1344 },
        Resolution { name: "Square XL",     width: 2048, height: 2048 },
        Resolution { name: "Portrait 2:3",  width:  896, height: 1344 },
        Resolution { name: "Landscape 3:2", width: 1344, height:  896 },
        Resolution { name: "Wide 16:9",     width: 1664, height:  928 },
    ],
}
```

- Next to the size: **Scale…** (to megapixels or a side, keeping the
  aspect) and **Aspect** (1:1, 5:4, 4:3, 3:2, 16:10, 16:9, 2:1, 21:9, each
  landscape or portrait; swap width and height; or the aspect of a reference,
  "Like <image1>"), keeping about the same megapixels, rounded to 32.
- If `resolutions` is missing or empty, the built-in list above is used (and
  written back the first time the list is edited).
- Width and height must be multiples of 32 (sd.cpp rounds output sizes up to
  32 and rejects ref images that aren't aligned). Values that aren't are
  snapped to the nearest multiple on load, with a warning in the log.
- Exactly one entry is the default. If none is marked, the first one is; if
  several are, the first marked one wins.
- `name` is optional. The dropdown shows `W × H` followed by the name.
- Duplicate sizes are allowed in the file but flagged in the editor.

**Resolution editor** (a modal opened from an **Edit…** button next to the
size dropdown in the Generate panel; Escape or Cancel closes it, a click
outside doesn't, as that would lose the edits):

- One row per entry: name, width, height, then derived aspect ratio (reduced,
  e.g. `3:2`, or `≈16:9` when close), megapixels, and an estimated time per
  image.
- The estimate scales the measured warm time (34 s for 1024², 20 steps, one
  R9700) by pixel count and by the Generate panel's current step count. It is
  shown as `≈` and is only a rough guide; replace the constant with measured
  per-size timings once the 2048² benchmark exists.
- Width/height fields snap to multiples of 32 when editing finishes and show a
  note when snapping changed the value. Sides outside 256–2048 get a warning
  (not an error).
- Row actions: set default, duplicate, swap W/H, move up/down, delete (not the
  last entry). Footer: **Save**, **Cancel** on the left; **Reset to
  built-ins**, **+ Add** on the right.
- Saving writes `config.rsn` immediately. Existing runs are unaffected: a run
  stores its exact width and height, never a reference to a list entry.

The Generate panel's dropdown also has a **Custom W × H** entry for one-off
sizes (same snapping and warnings), with a **Save to list** button that adds
it as a new entry.

## Data model

```
Inbox ── Idea (unreviewed) ──review──► Input library (kept ideas + ref images)
                                            │ new project (one or many ideas)
                                            ▼
Project ─┬─ Run (prompt template, params, refs, N seeds)
         │    └─ Revision (edited params; new seeds go here)
         │         └─ Item (one seed → one image) ──review──► Output library
         └─ sources: the ideas it was made from
```

### Seeds

A run's seeds are consecutive: a first seed plus a count, always sent to
sd-server explicitly (never `-1`/random).

- **The first seed auto-advances per project**: the Generate panel's
  **First seed** starts at one past the highest seed any run of the project
  has used (generated, pending, cancelled or failed), so a new run gets fresh
  seeds. It is derived from the project's `run.rsn` files, so it survives
  restarts; a new project starts at 0. It is set when a project is selected
  and again after each run is queued.
- For A/B comparison, the **0** button next to the field restarts at seed
  0, so runs line up seed for seed: seed 3 of `r01` and seed 3 of `r02 cfg6`
  then differ only in the run's settings. (Until 2026-10-24 this was the
  default.)
- Adding seeds to an existing run, including seeds for a new revision,
  continues after that run's highest seed, so a run never generates the same
  seed twice.
- A seed is pending until its output link exists in the project, or it has
  a timing (it was generated: a link deleted, or lost by an unzip tool
  that skips symlinks, doesn't queue it again). Deleting an output link
  records its seed in `run.rsn` (`deleted: [Seeds { first, count }]`); a
  generated seed with neither a link nor that record has lost its link,
  which opening the library warns about (a message per project) and
  `--check-library` reports as a problem. Cancel
  records the dropped seeds in `run.rsn` (`cancelled: [Seeds { first, count
  }]`), and a seed that fails for good (bad request, generation error) is
  recorded with its error (`failed: [SeedFailure { rev, seed, error }]`), so
  neither is queued again after a restart.
- Each stored image's timing is recorded in `run.rsn` too (`timings:
  [SeedTiming { rev, seed, device, done, elapsed_s, queued_s, step_s }]`,
  see Timing), and in its recipe (`elapsed_s`, `queued_s`, `step_s`).

### IDs

Ideas, projects, runs, input images and output images all have IDs of the
form `YYYY.MM.DD-HH.MM-NNNN`, e.g. `2025.01.30-23.45-0000`:

- The timestamp is the creation time in **UTC**, to the minute: when an idea
  or project was created, a run queued, an input image imported, an output
  image written. UTC keeps names in true chronological order (no DST jumps)
  and identical across machines.
- `NNNN` counts from `0000` within that minute, **separately for each kind**.
  So `2025.01.30-23.45-0000` can be an idea, a run and an output image at the
  same time; a reference always knows which kind it means from where it
  appears (`source_idea`, `projects`, an `ImageRef`'s `store`).
- Sorting by ID is sorting by creation time.
- IDs are never reused or changed. Names people see (project directory names,
  idea titles, folder names) are separate and can change freely.

Allocation: the app is the only writer. It holds `library.lock` while the
library is open, and a second instance refuses to open it. On opening, it
scans the existing IDs of each kind; after that it hands out the next number
for the current minute from memory. Store images are additionally created
with an exclusive create, so even a wrong count can't overwrite a file; it
just moves on to the next number. Past 9999 in one minute the counter grows a
fifth digit (names then stop sorting exactly; not expected in practice).

### Ideas (input side)

An `Idea` is a small folder: `idea.rsn` plus any attached images.

- `idea.rsn`: id (see IDs), title, text (prompt draft, notes, or both),
  `variants: Vec<PromptVariant>` (rewritten or hand-edited alternatives, each
  with its origin: `Manual` or `Rewritten { model, system_prompt_hash }`),
  tags, source (`Manual`, `Clipboard`, `DroppedFile { original_path }`,
  `InboxFolder`, `FromOutput { image: ImageRef }`), created time,
  `projects: Vec<ProjectId>` made from it, `starred: bool`.
- Attached images live in the input image store
  (`input_images/<image-id>.png`, see Image stores), and the same picture
  used by several ideas and as a ref is stored once. `idea.rsn` lists the
  image IDs (authoritative); the idea folder also holds links to them named
  `<n>-<original name>.png`, so it can be browsed in a file manager.
- An image with no text is still a valid idea ("make something like this").
  A reference image in the input library is stored as such an image-only
  idea, so every library entry is an idea folder that can be filed.

Ways in: a text box / "new idea" hotkey, paste from clipboard (text or image),
drag and drop files, and a watched drop folder (`input/inbox_drop/`) so ideas
can be added from outside the app (phone sync, browser saves, scripts).
The inbox grid says so in a line at its top, with the folder's full path
and an Open button. Its cards have their own size zoom (like the Input
library's, saved separately).

Whatever the way in, images are imported into `input_images/` straight away
(re-encoded as PNG, see Image stores) and the new idea in the Idea inbox links
to them; the inbox has no image storage of its own. A file taken from
`inbox_drop/` is then moved to `inbox_drop/imported/`, so the watcher doesn't
import it twice and the original is still there until you clear it out.

Input review uses the same keys as output review, but the verdicts mean
something different. Good and neutral both move the idea into the input
library (top level of `input/library/`), differing only in `starred`; bad
moves it to `input/discarded/`:

| Key | Output queue | Input queue |
| --- | --- | --- |
| good | keep, best | **keep**, starred: shortlist for new projects |
| neutral | keep | **keep** in the input library |
| bad | reject (out of the way, still recoverable) | **discard** (`input/discarded/`, still recoverable) |
| `P` / `Space` | — | **new project** from the idea (or add to an open one) seeded with the idea's text as prompt and its images as refs |
| `W` | — | **rewrite** the idea's text with the PE model (adds a variant) |

**New project from idea** always asks for the new project's name first (an
idea's title needn't suit a project): the first idea title that is a valid,
free name is the guess (numbered when taken), else the field is empty; an
invalid or taken name is refused. **Add & new project…** in the New idea form
asks the same way and keeps the draft until the name is given. The dialog has
the save dialog's layout. Left, the **Folder**: a tree of the project folders
(with a filter) and, under it, the new folder to make in the one chosen. By
default the project goes in a new folder of its own name (it follows the name
as it is typed; renamed there, it no longer does); ✕ takes it away (the
project goes in the folder itself) and **New folder** brings it back, named
after the project. It is only made on Create, and an existing folder of that
name, in any case, is used. Right, the **Project name** (it has the focus as
the dialog opens; a field clicked keeps it) and where the project will be made
(`projects/clients/acme/Fox cafe/Fox cafe`); **Create** (Enter) makes it
there. The first folder is where the last project from ideas went (this
session), if that folder still exists, else the top level; after a project in
its own folder, that is the folder's parent (so projects don't nest).

A new project from several selected ideas at once is one project with all of them
as sources (e.g. a style image from one idea + a subject from another). An
idea with several variants can become one run per variant.

An idea's images become the project's `ref_images`, never an implicit `init_image`;
`init_image` is only sent for an explicit Edit run.

### Output → input

An output image can feed new work, pointed at in place (an `ImageRef` into
`output_images/`, no copy), so recipes' `refs` / `parent` lead back to it and
its project. Clean up keeps any output another run refers to.

- **How a project uses such images** is a project setting (`project.rsn`
  `edit_with`), shown in the Generate panel: **references** (the default:
  `ref_images`, Qwen-Image 2.1's native editing; the model both reads the
  image through the vision encoder and gets its VAE latents) or **img2img
  start** with a strength (`init_image`: sampling starts from a noised copy,
  keeping the layout; the recipe's `parent`). A copied project keeps it.
- Actions on an image (Review: keys, right-click in Single and Grid; Output
  library: right-click; Compare: right-click either side), each following
  the target project's setting:
  - **Use as a reference in ‹project›**: adds it to the selected project's
    form as a reference (disabled with no project selected).
  - `E` **Edit this image**: a fresh form in the image's own project with the
    image; with references the prompt starts as `<image1> ` for the change.
  - `N` **New project from this image**: a project next to the image's,
    named "<project> from s0003" (opened for rename), references, the image's
    prompt prefilled.
  - `D` **Duplicate project with this image**: a copy of the image's project
    (settings, sources, draft; no images), the form started from the image's
    run (prompt, size, steps, CFG, transparency, refs) plus the image.
  - `R` to the input library as a ref (an image-only idea, pixels imported
    into the input store), `I` a new idea in the Idea inbox with its prompt.
    Lineage is kept via `source: FromOutput`.
- The right-click menus of project tiles and the project tree have
  **Duplicate** (the same copy, no image). It first asks for the copy's name
  in a dialog ("<name> copy", flagged if taken or invalid), then copies next
  to the project and selects the copy.
- Up to 9 references (Qwen-Image 2.1's stated limit; sd.cpp enforces none):
  the Generate panel counts them ("3 / 9") and warns past 9. Each ref costs
  about as much attention as the output (refs are resized to the output's
  pixel count), which the time estimate includes.
- Each image in the Generate panel has **use size** next to its dimensions:
  it sets the output size to that image's aspect, in multiples of 32 within
  256–2048 (greyed out when the size already matches).

### Recipe

`Recipe` (embedded in each PNG as an `iTXt` chunk `increart:recipe`, RSN):

- `prompt_original`, `prompt_sent` (after rewrite and/or the RGBA wrapper),
  negative_prompt, width, height, seed, `alpha: bool`
- sample params: method, scheduler, steps, cfg, flow_shift, custom_sigmas.
  An empty scheduler and `flow_shift: None` are left out of the request, so
  the recipe's own (resolution-dependent) flow schedule applies
- `mode`: `Generate` | `Edit { init: ImageRef?, strength }`
- `refs: Vec<ImageRef>` — `ImageRef { id, store: Input | Output }`.
  The image is always at `<store>_images/<id>.png`, so a reference
  never goes stale, however the folders it is linked from are reorganised.
- `parent: Option<ImageRef>` — the image this was edited from (lineage)
- `source_idea: Option<IdeaId>`, and the PE model + system-prompt hash if
  rewritten
- model selection: recipe id + slot → (file id, sha256)
- sd.cpp commit, IncreArt version, GPU/backend, elapsed seconds
- `inputs: Vec<InputImage>` — the images it was made from, filled in by the
  library when the output is stored: `role` (`Ref(n)` for `<imageN>`, or
  `Start` for the img2img start image), `image` (`ImageRef`), `file` (the
  store file, e.g. `input_images/<id>.png`), `original_name` (the file an
  input was imported from, if known) and `blake3` (its BLAKE3 pixel hash).
  The output's own BLAKE3 is its `increart:blake3` chunk.

**Metadata upgrade.** `library.rsn` records `metadata: 1` once every store
image has `increart:blake3` and every output recipe its `inputs`. A library
opened without it is upgraded first (inputs, then outputs): only those text
chunks are added, the pixels aren't re-encoded. A store file is replaced
atomically, except in hard-link libraries, where it is rewritten in place so
the links keep sharing it. Images that can't be upgraded are listed as
problems. `metadata: 2`: runs record their deleted output links (`deleted`,
below); upgrading to it records every generated seed whose link is already
gone as deleted.

**Use counts.** How often an image was used is derived from the run records:
the runs naming it as a ref, the runs starting from it, and the images those
runs made. A run counts once it has generated (has a revision), so a project
duplicated without seeds doesn't count. It shows on idea cards (their images
added up; the input library can sort by it, **order: most used**), in
Generate's image list and in the Output library's lineage panel.

The two run counts are also kept in every store image (inputs and outputs), as
`increart:usage` (`tEXt`): `ref=000000003 start=000000001`. They are "ever
used" counts: one more ref per run naming the image (however often), one more
start per run starting from it, added when the run first generates (its first
revision) and never lowered, so they survive the run being deleted. The text
is always 29 bytes (nine digits each, at most 999999999), so a new count is
written over the old chunk in place: the chunk type, data and CRC in one
write, then a sync; the file keeps its size and isn't replaced. Copies in
copy-mode projects aren't updated. A write cut short leaves a wrong CRC.
Each time a library opens, every store image's counts are checked against the
runs: a missing, torn or unreadable chunk is rewritten from them, and a count
lower than the runs show is raised. The check reads every store image (about
6 s for 500 images on a cold spinning disk), so `Library::open` leaves it to
the caller: the GUI runs it in the background after opening, one image at a
time (library operations go on meanwhile), with a progress bar in the status
bar ("checking images 120 / 950") from its start, kept full ("checked 950
images") for 5 s once done; opening another library stops it. A failure to update the counts is listed
as a problem; it doesn't stop the run.

sd-server also embeds its own `parameters` tEXt chunk (A1111-style text plus
full `SDCPP` JSON). It is kept as-is. `increart:recipe` is authoritative
because it carries hashes and lineage.

Requests send refs as base64 PNG in `ref_images`; the image bytes are read from
the library at submit time and checked against their `increart:pixels`.

### Pixel hash

Besides its ID, every image has a content fingerprint, `pixel_sha256`: the
SHA-256 of its decoded pixels, not of the file bytes. Rewriting metadata (the
recipe chunk, sd-server's `parameters` chunk) or re-encoding at another
compression level doesn't change it. It is used to spot duplicates on import,
to verify files, and in cache keys; it is not how images are referenced.

Canonical encoding, version 1 (hashed in this order):

1. the ASCII tag `increart-pixels-v1`
2. width and height, each as a big-endian `u32`
3. channel count as one byte: 3 (RGB) or 4 (RGBA)
4. the rows, top to bottom, 8 bits per channel, no filter bytes or padding

Decoding rules: grey and palette images are expanded to RGB; an image has 4
channels exactly when the file has alpha (an alpha channel or `tRNS`), even
if every pixel is opaque; 16-bit channels keep the high byte. The version tag
lets the encoding change later without colliding with old hashes.

Every image also carries a second fingerprint, `increart:blake3`: BLAKE3
of the same canonical encoding (hex, 64 characters). The SHA-256 stays the
key of the pixel index and of verification; the BLAKE3 is what an output's
recipe records for each image it was made from (Recipe, `inputs`).

Model weights are the exception: they are identified by the SHA-256 of the
file bytes.

### Image stores

All images live in two flat stores, named by image ID. Each one is a PNG or
a lossless WebP (see Store format below):

- `input_images/<image-id>.png`: everything that came in from outside
  (idea attachments, reference images, edit sources).
- `output_images/<image-id>.png`: everything sd-server generated.

Store files are never renamed or moved. Idea folders and project folders hold
**links** to them (see Links), so the folder structure is still browsable and
review still works by moving files, but moving or deleting a link never
touches the image.

**Inputs are re-encoded losslessly on import** (PNG, or WebP; see Store format). Lossy formats (JPEG, WebP, AVIF with the `avif` feature) decode
slightly differently across decoder versions, so their pixel hash wouldn't be
stable, and one format keeps everything downstream simple. On import:

1. Decode; apply the EXIF orientation; convert to sRGB if an ICC profile says
   otherwise; reduce to 8-bit RGB or RGBA (keeping alpha if the file has it).
2. Compute `pixel_sha256`. If the input store already has an image with that
   hash (pixel index, below), reuse it.
3. Otherwise allocate an image ID and write the file with these chunks:
   - `increart:id` (`tEXt`): the image ID.
   - `increart:pixels` (`tEXt`): the pixel hash.
   - `increart:blake3` (`tEXt`): the BLAKE3 pixel hash.
   - `increart:usage` (`tEXt`): the use counts, `ref=000000000 start=000000000`
     (see Use counts).
   - `increart:source` (`iTXt`, JSON): what the original was, e.g.
     ```json
     {
       "version": 1,
       "original_name": "glaze.jpg",
       "original_path": "/home/user/Downloads/glaze.jpg",
       "original_format": "jpeg",
       "original_size": 482113,
       "original_sha256": "<sha256 of the original file bytes>",
       "original_mtime": "2026-10-19T08:12:44Z",
       "original_ctime": "2026-10-19T08:12:44Z",
       "original_atime": "2026-10-20T17:03:10Z",
       "original_created": "2026-10-19T08:12:43Z",
       "original_mode": 420, "original_uid": 1000, "original_gid": 1000,
       "original_width": 3024, "original_height": 4032,
       "imported": "2026-10-21T09:00:00Z",
       "conversions": ["exif-orientation: rotate 90", "icc: Display P3 -> sRGB"]
     }
     ```
   - `eXIf`: the original EXIF block, unchanged.

The original file's times (modified, status changed, last accessed before
the import read it, and birth time where the filesystem keeps one),
permission bits and owner are read before its bytes; those not known are
left out, and images imported before they were recorded don't have them. The
original file's bytes are not kept. When an identical picture is imported
again, the store file keeps the first import's `increart:source`; each idea's
`source` still records where its copy came from.

**Outputs arrive as PNG.** Requests set `output_format: "png"` (also
sd-server's default). IncreArt then adds `increart:id`, `increart:pixels`,
`increart:blake3`, `increart:usage` and `increart:recipe` as extra chunks, keeps
sd-server's `parameters` chunk, and writes the result to
`output_images/<image-id>.png`. In a PNG library the pixels are not
re-encoded; in a WebP library they are encoded as WebP with the same chunks.

`increart:id` lets the app identify a file reached through a hard link or a
copy (see Links), whose own name is not the ID. `increart:pixels` is checked
against a real hash when an image is first indexed. The **pixel index**
(`pixel_sha256 → image ID`, per store) is kept in `cache/`, and rebuilt from
the `increart:pixels` chunks by a scan when missing.

**Store format.** Settings → **Library** (this library only) picks the
format for new store files: **PNG** (the default) or **WebP, lossless**,
about half the size of PNG (48.9 % on 20 real outputs; libwebp, method 4,
`exact`, so the colour under transparent pixels is kept too). It is kept in
`library.rsn` (`store_format`, left out while PNG). WebP can't hold a side
over 16383 pixels; such an image stays PNG whatever is chosen. A library can
therefore be mixed: an image is `<image-id>.png` or `<image-id>.webp`, never
both, and every link to it has the same extension as its file.

A WebP store file is a lossless `VP8L` image in an extended (`VP8X`)
container: `RIFF/WEBP`, `VP8X` (its alpha flag set only when the image has
an alpha channel, so the pixel hash's channel count survives), then one
`PNGt` chunk per PNG text chunk (the PNG chunk's type, data and CRC,
big-endian, exactly as in a PNG), then `VP8L`, then `EXIF` if any. Decoders
skip the unknown `PNGt` chunks; IncreArt reads them like a PNG's, and since
they come before the image data, `increart:id` and `increart:usage` are read
and patched in place the same way (the CRC catches a torn write). sd-server
can't read WebP: refs and init images from a WebP store are sent as PNG,
encoded quickly without metadata.

**Convert N images to X…** (asks first) re-encodes every store file not
already in the chosen format. Files are encoded in parallel outside the
library lock (all cores but one, at most 8), each checked by decoding it
again and comparing its pixel hash with `increart:pixels`; then each batch
is installed under the lock: the new file is written atomically with the old
file's modification time, every link to the image (project outputs,
collections, idea folders) is remade with the new extension, and the old
file is deleted. Stop ends it after the current batch; running it again
carries on, and an image found in both formats (an interruption between
writing and deleting) is reported when the library opens, and fixed by
converting again. Files that can't be converted (unreadable, or too large
for WebP) are skipped and counted.

`library.rsn` has format 1 while the library is all PNG and format 2 once
WebP is chosen or WebP files exist, so an older IncreArt, which can't read
them, refuses to open it instead of seeing missing images. Converting back
to PNG returns it to format 1.

**Clean up.** Deleting a discarded project or idea for good only removes
links, so store images can become unreferenced. A **Clean up library**
command (never automatic) marks every image still referenced (links, those
of discarded projects too, `idea.rsn` images, `run.rsn` refs (discarded
projects' too, so a restore finds them), recipes' `refs` / `init` /
`parent`, `FromOutput` sources) and moves the rest to the system trash. An input image used as a ref by any
kept output therefore survives even after its idea is gone.

## Scheduling (same as Yue2, plus model switching)

Runs are queued; each free server takes the next pending seed of the
unfinished run that has had the fewest seeds dispatched since it was queued
(round-robin), so all unfinished runs share the GPUs. A newly queued run
therefore starts producing images right away instead of waiting behind a long
batch (at ~34 s per seed, a 256-seed run would otherwise block everything for
over an hour). A server that errors is backed off and retried; its seed goes
back to the pool. Stopping a run comes in two kinds, on each run in Jobs, in
its details window, and for every run at once in the Jobs header:

- **Finish & stop** drops the pending seeds and cancels in-flight jobs that
  are still waiting on the server, but lets the job already generating finish
  and keeps its image (the run shows "finishing N images…" until then). The
  two are told apart by the job's polled status (the worker reports a job
  once it is seen generating).
- **Stop now** = `jobs/{id}/cancel` on every in-flight job + drop pending
  seeds; images being generated are thrown away. **Stop all now** asks first.
- Either way, the seeds cancelled are let go of at once, not when their
  server says the job ended: a job a server never ends (a hung proxy, a
  KoboldCpp restarting) or a worker busy with a long load would otherwise
  keep the run "finishing…" for good. The cancels are still sent; a job
  that ends later is ignored. Stop now also ends a Finish & stop whose
  generating image never comes.

- **Shallow submission**: at most 2 jobs in flight per server (one generating,
  one queued). Everything else stays in IncreArt's own pool. This keeps
  cancel cheap, stays well under the 64-job limit, and means results are
  fetched promptly (before the job TTL expires).
- **Ties**: among runs that are equally due, the one whose last seed went
  out longest ago goes first, so every round visits the runs in queue order
  (a style sweep on one GPU fills its contact sheet style by style, column
  by column). There is no per-server affinity: with sd-server's conditioning
  cache at 32 entries, alternating between runs rarely costs a re-encode.
- **Order**: the toggle in the jobs popover and Projects → Jobs picks
  **Spread evenly** (the round-robin above; the default) or **One at a
  time** (every free slot goes to the earliest queued run with seeds left,
  so runs finish one after another in queue order). It is saved in the
  config (`job_order`) and applies to the running queue at once. Queueing a
  style sweep while it is One at a time asks first: **Spread evenly &
  queue**, **Keep one at a time & queue** or Cancel (one at a time would
  fill the contact sheet a style at a time).
- **Model switching**: a run whose model selection differs from a server's
  loaded model marks that server *draining*. It gets no new submissions; once
  its in-flight jobs finish, it calls `models/load` (409 means "not drained
  yet, retry") and then resumes. The other server keeps working on whatever
  matches its model. Avoid switching both servers back and forth: prefer the
  server that is already idle or loaded with the target model.
  Concretely: work for a model no server has makes one server drain for it
  (an idle one first, else the one with fewest jobs in flight), but never
  the last server of a model that still has pending work; a server with
  nothing to do for its own model switches to the most due one that no
  server is loading (until that load is done it stays free for another
  model's work queued meanwhile: runs queued together arrive one by one,
  and loads on this machine go one at a time anyway). With a
  single server, runs for a second model therefore wait until the first
  model's work is done (no reload ping-pong). A model whose load fails
  on a server isn't tried there again (no retry loop) but may still go to
  another; its runs wait (shown as stuck) once no server is left.
- **Which servers**: a model goes only to servers whose model list has all
  of its files: sd-server's `model-files`, a started KoboldCpp's model
  folders less the files it can't load (sd-server's int8 safetensors), a
  KoboldCpp started elsewhere only its running model. A server's list is
  read with its probe and again whenever runs are queued; a server that
  can't give one is tried with anything, as before. So a run of a model
  only one server has goes there, not to the first idle server (KoboldCpp
  first in line used to take an sd-server-only int8 model and fail, which
  stuck the run for every server). A model no server can take is said
  once in the messages and in its runs' logs, with each server's reason
  ("no server can load krea2, so its runs wait (kobold: …; proxy: …)").
- **Generation log**: each run's `generation.log` (next to its `run.rsn`)
  has a timestamped line, tagged with the server, for everything the
  engine does for it: queued, which server loads its model (and why not
  the others), how the load went and how long it took, the warm-up, the
  first seed's `img_gen` request (base64 data shown by its length), each
  seed sent, its job ID, generating, done or failed with the server's
  answer, cancels, and a server's failures. The job details window shows
  it (**Generation log**, live while open, with **Copy** and **Show
  file**; wrapped, the time dimmed, errors red and warnings amber, paths
  as file names unless **full paths** is ticked), and its progress says "waiting for a server" until a server
  has one of the run's seeds, "generating on <server>" then.
- **One load at a time on this machine**: servers on `localhost` /
  `127.0.0.1` load models one after another (a server waiting shows
  "waiting for another server's load"). A KoboldCpp that offloads its
  weights (`--sdoffloadcpu`) pins about its files' size in RAM, less the
  model it drops; its load waits up to 2 s for that much RAM plus 8 GiB to
  be free ("waiting for RAM: 20.6 of 21.0 GiB free (13.0 GiB to load + 8
  to spare)"), then loads anyway and says so in the messages ("loading
  without enough free RAM after 2 s: …"; RAM held outside the app, e.g.
  by increbot, may never come free), without holding up the other
  servers' loads meanwhile, and fails at once if the machine hasn't that
  much at all (RAM from `/proc/meminfo` on Linux,
  `GlobalMemoryStatusEx` on Windows, `hw.memsize` and `vm_stat`'s free,
  inactive and speculative pages on macOS). Two loads at once, with pinned weights,
  had starved the desktop (no swap; increbot shares the RAM). Server
  settings' **Wait for free RAM before loading a model** unticked skips
  the wait and the message. Remote servers load freely.
- **Warm-up**: after launch or load, each server runs one small throwaway job
  (e.g. 512², 1 step) so lazy weight loading doesn't land on the first real
  seed.

32 GB per GPU fits the int8 diffusion model + int8 text encoder;
`--offload-to-cpu` is a per-server config option if it doesn't (on ROCm it
costs ~4% speed and frees ~21 GiB: see Measurements so far).

## Filesystem layout

```
<library_root>/                       e.g. ~/IncreArt
  library.rsn                         format version, link mode (see Links), store format
  library.lock                        held while the app has the library open
  input_images/<image-id>.png         input image store (PNG or .webp, see Image stores)
  output_images/<image-id>.png        output image store (PNG or .webp)
  input/                              input queue + input library
    inbox_drop/                       watched; anything dropped here becomes an idea
      imported/                       originals already imported
    unreviewed/<idea-id>/             Idea inbox
      idea.rsn
      1-glaze.png                     link → ../../../input_images/<image-id>.png
    library/<folder>/…/<idea-id>/     input library: folder tree of kept ideas
    discarded/<idea-id>/              discarded ideas (recoverable)
    edited/<idea-id>/                 cropped and marked copies (Crop / mark…), hidden
  projects/                           folder tree (see Project folders)
    <folder>/…/<project>/             output queue + output library (per project)
      project.rsn                     project id + source idea ids (+ cover image, image titles)
      runs/<run-id>/run.rsn
      runs/<run-id>/generation.log    what the engine did for it and its servers' answers (Job window ▸ Generation log)
      unreviewed/<run-id>_r<rev>_s<seed>.png  link → output_images/<image-id>.png
      reviewed/{good,neutral,bad}/...         links
  collections/<folder>/…/<image-id>.png  output-library collections: links into output_images/
  thumbs/{input,output}/<image-id>.webp   shared cache, deletable
  cache/pixel-index.rsn               pixel hash → image ID, transparent images; deletable
  cache/conditioning/                 future: disk conditioning cache (needs sd.cpp patch)
```

The output library is the union of every project's `reviewed/` folders; the
input library is every idea under `input/library/`. Both are made of links;
the images themselves are in the stores.

Output links are named `<run-id>_r<rev>_s<seed>.png`, e.g.
`2026.10.19-09.00-0000_r0_s0003.png`: run `2026.10.19-09.00-0000`, revision
0, seed 3. The seed is zero-padded to at least 4 digits (`s0042`). The
`r`/`s` markers keep the name from reading like an image ID; within a project
the links sort by run, then revision, then seed (in numeric order as long as
the seeds in a run have the same number of digits, which padding guarantees
below 10000). The image's own ID is the link's target. A link's extension is its
target's (`.webp` for an image stored as WebP, see Image stores).

### Links

Links are **relative** (e.g. `../../../../output_images/<image-id>.png`), so the
library can be moved or mounted elsewhere. Their kind is set per library in
`library.rsn` (`link_mode`), chosen when the library is created (or first
opened, when `library.rsn` names none: the example library) by trying, in
order:

1. **Symlinks**: the default on Linux and macOS. Clearly show which file is
   the real one.
2. **Hard links**: no special permission needed on Windows, but only on NTFS
   and within one volume (the library is one directory tree, so that holds).
   They look like ordinary files to every tool; `increart:id` identifies them.
3. **Copies**: last resort, e.g. on FAT32/exFAT drives, which support no
   links at all. Works, but uses the space twice.

`INCREART_LINK_MODE=symlink|hardlink|copy` skips the trial and makes new
libraries with that kind (an unknown value refuses to create one); CI runs
the tests in all three on Windows.

Windows: symlinks are supported on NTFS, but **creating** one needs
Developer Mode or administrator rights (following them doesn't), so an
ordinary Windows install usually ends up on hard links. A library on a Linux
machine shared to Windows over SMB keeps its symlinks: Samba follows links
that stay inside the share by default, and these do.

The app treats all three the same: it reads a link's image ID from the
target path (symlink) or from `increart:id` (hard link, copy), so a library
can mix them. The example library (`example-library/` in the repository)
has no `link_mode`: opened, it probes one, makes its links that kind and
saves it. Git keeps its links as symlinks, or where it can't make them
(Windows without Developer Mode) as **stand-ins**, small files holding just
the target path; release zips hold stand-ins, which every unzip tool keeps
(some skip symlinks). A stand-in counts only if it is under 512 bytes, its
path starts with `../`, and it reaches a store image from where it is.
Copies are taken as they are. Opening a library also makes any of the
standard folders that are missing (git keeps no empty ones).

### Library archives

A whole library as one file, from the library menu (`archive.rs`):

- **Export the library to a zip…** / **… to a tar.gz…**: a save dialog
  beside the library, named after it. The archive holds one folder, the
  library, without `cache/`, `thumbs/` or the lock; `library.rsn` without
  `link_mode`. Each link goes in as a link to its store image, whatever its
  kind in the library, so each image is stored once (a hard-link or copy
  library too): in a tar.gz a **symlink** (`tar` on Linux and macOS makes
  them; tools on Windows may not), in a zip its **stand-in** (every unzip
  tool keeps it). PNG and WebP are stored, not compressed again, so the two
  come out about the same size. Written beside the destination first,
  then renamed. The library stays usable: the export only reads it, records
  first, then links, then store images, so a seed stored meanwhile is
  either missing from the archive or whole.
- **Unpack a library archive…** (also on the start-up library screen): a
  `.zip` or `.tar.gz` (any: these, a release zip, a `tar czf` of a
  library), then a folder to unpack it into (the picker starts beside the
  archive); the library becomes a folder of its own there, named as in the
  archive (or after the archive), and is opened (as switching does). A
  real library: the archive is left as it is. Symlinks come out as stand-ins and `library.rsn` loses
  its `link_mode`, so opening makes the links the kind this disk takes.
  Refused: an entry outside the archive's folder (an absolute path, `..`, a
  drive), an archive without a library, a folder of that name there already.
  Unpacked into a hidden folder first, so a failure leaves nothing.
- While either runs, the menu shows its progress (MB done of how many) and
  **Stop** in place of the three entries; the result is a message.

Review moves files or idea folders between folders (like Yue2); the folder is
the source of truth for the verdict on both sides (for ideas: inbox, library
or discarded; the star lives in `idea.rsn`). A `notify` watcher picks
up external moves/deletes. Deleting goes to the system trash.

### Drafts

Forms that haven't been submitted yet are autosaved **2 s after the last
edit**, and immediately when switching projects, adding the idea, or
quitting:

- **Generate**: one draft per project, `projects/<project>/draft.rsn`
  (`GenerateDraft { label, prompt, negative_prompt, width, height, preset,
  steps, cfg, flow_shift, seeds, first_seed, alpha, refs, source_idea }`; the
  model preset by name). Selecting a project restores its draft (a fresh
  form if it has none); the label moves on if a run already uses it, and the
  first seed continues after the project's seeds unless the draft chose 0.
  A project with runs but no draft (e.g. runs made before drafts existed)
  starts from its latest run's prompt, refs and settings.
  Queueing keeps the draft (with the next label and seed) so a variation is
  one edit away; **Clear** resets it. Copying a project copies its draft.
  Queue again before that run or sweep is saved (a double-click; the label
  moves on only once it is) says "r05 is being queued already" and saves
  nothing: a sweep saved twice showed each style twice on its contact sheet.
- Run labels count from `r00` (a sweep counts once), always past the highest
  `rNN` the project already has, so libraries numbered from `r01` carry on.
- **New idea**: `input/draft.rsn` (`IdeaDraft { title, text, tags, images }`;
  the images are already in the input store). Restored when the library is
  opened, removed when the idea is added.
- **Clean up** counts draft refs and draft images as references.
- The forms show "unsaved changes…" while a save is pending and "draft
  saved" for a moment after.

### Generate history (prompt history)

The last **1000** substantial edits are kept and can be viewed and restored
(`increart_core::history`), as JSON lines next to what they belong to:

- a project's Generate form: `projects/<project>/prompt-history.jsonl` (the
  name from when it held only the prompt; a duplicated project starts with an
  empty history). Each entry has the prompt and negative and, as `form`, the
  whole form as the autosaved draft has it: size, model preset, steps, cfg,
  flow shift, seeds, the image list with each image's roles, strength,
  source idea and sweep settings. A **queued** checkpoint has `runs`, the
  IDs of the runs it made (a sweep's all). Every entry has `after`, the
  project's newest run before it (an edit "since r05"). Entries from before
  this have only the texts: their runs are found by the label in "queued
  r05" (or the sweep's name), and `after` by time;
- the New idea form (text and title): `input/new-idea-history.jsonl`;
- an idea's text (Edit idea): `<idea folder>/text-history.jsonl` (moves with
  the idea).

When an entry is made:

- **edit**: at a pause of 2 s (the draft autosave), after typing or after
  changing a setting. Identical to the newest entry (texts and settings;
  the label and first seed, which move on by themselves, don't count):
  nothing. Fewer than ~20 characters changed (word diff) within 60 s of the
  previous edit: the previous edit is **replaced** instead of adding one, so
  typing in bursts gives one entry per burst; a change to settings alone
  counts as small, so trying steps or cfg values gives one entry too.
- **checkpoints**, never merged and trimmed last: **queued r05** (the form
  a run or sweep was queued with, recorded once the library has made the
  runs), **before clear**, **before restore**, **before tag update**,
  **before preset**, **saved** (an idea added from the New idea form, or
  saved in Edit idea). A project's checkpoints have the whole form too.
- The file is appended to (a merge rewrites the last line) and compacted
  to 1000 entries once it reaches 1100, dropping the oldest plain edits
  first.

**History…** (next to the prompt, in New idea, and in Edit idea) opens a
window: entries newest first (age, kind, first line), search, "checkpoints
only", a word diff of the selected entry against the current text, and
**Restore** / **Restore with title** / **Copy**; ↑/↓ browse, Enter or
double-click restores. Restoring first records the current text or form
("before restore"), so it can itself be undone from the history.

A project's window, **Generate history**, also shows the settings and runs:

- Each row: the settings changed since the entry before ("steps 20 → 30 ·
  +1 image · 1024×768"), or "prompt only" for an older entry; an edit's
  row says which run it followed ("since r05"); a queued row has a chip per
  run, in the run's current label (hover: project, model, prompt, images
  made and reviewed). A click on a chip opens the run's details, its images
  button the run's images in the Output library (a source of its own,
  "run r05 of fox-cafe"). A sweep's runs are one chip, "r05 – r12 · sweep
  styles", opening the sweep's contact sheet. A run since deleted is "deleted
  run", struck through.
- The selected entry: "queued as · 12 images, 4 reviewed" with its chips,
  the word diff, and its settings compared with the current form.
- Filters: **settings changes** (entries whose settings differ from the
  entry before), and **led to:** a run, which shows only the entries since
  the run before it, up to that run's queued checkpoint.
- **Restore all** (Enter, double-click) puts back the prompt and every
  setting; the label and first seed stay the next run's. Images no longer in
  the library are left out and a model preset that's gone keeps the current
  one, with a message. **Restore prompt only** and **Restore prompt +
  negative** keep the settings. Restoring opens the project in Generate if
  it isn't. History entries don't protect their images from Clean up.
- **Show in history** in a run's details (Jobs, the history's chips) and
  **history** next to the run in the Output library's lineage panel open
  the project's history filtered to what led to that run, at its queued
  checkpoint.

### Prompt presets

Ready-made prompts for the Generate form, by task: the three style tasks
(below), **Remove background**, **Repair / restore**, **Edit content**,
**Relight / mood**, **Extend / reframe**, **Character** (one thing changed,
the rest of the character locked, under headings: Pose & setting, Drawing
style, Colour, Clothing, Equipment, Action; each group's prompts end
saying what stays: "Keep the line art, shading, design and pose
identical."), **Combine images**, **Text**; 240 built in, in
`crates/increart-core/default-presets.rsn` (embedded in the app; its header
notes the conventions below). They are shared by every library, in a
`prompt-presets` folder next to `config.rsn`:

```
prompt-presets/
    default-presets.rsn            the built-in set, rewritten on every start
    user-state.rsn                 favourites, deleted built-ins, user tasks
    Modify style/                  a folder per task (name made file-safe)
        3D & craft/                a folder per group, if the preset has one
            Fusilli.rsn            one preset per file
            Claymation.rsn         a built-in one, changed: it replaces it
    Remove background/
        Mine.rsn
```

Only what differs from the built-in set is written: a changed or added preset
is a file (its task, group, name, description, prompt and negative); a deleted
built-in one, a favourite, a user task (and its description, or a built-in
task's changed one, or a task kept with no presets) are in `user-state.rsn`.
So presets added to the built-in set in a new version show up, and a file
copied into a task folder (from someone else) is a new preset: a hand-made one
may leave out `task` and `group`, which then come from its folders, and `name`
(the file name). Loading puts the built-in set first, in its order, minus the
deleted; a user preset takes the place of the built-in one of its name when in
the same group, else goes after the last preset of its group (or at the end of
the task), in name order; user tasks come after the built-in ones, in name
order. So there is no reordering. Saving removes the files of presets no
longer there, and folders left empty.

A preset file that doesn't load is reported and left alone (skipped, never
written over or removed); a `user-state.rsn` that doesn't load is reported
and the built-in set is used, nothing saved. A `prompt-presets.rsn` (the
single file of earlier versions) is moved into a new folder on the first start
and renamed `prompt-presets.rsn.old`.

A preset has a name, an optional group (a heading within its task), a
description, the prompt, an optional negative, and a favourite flag. The
description loosely defines the name for someone who doesn't know the term,
then says what you get ("Claymation: stop-motion animation with plasticine
figures: soft clay, fingerprints, slightly lumpy handmade shapes"). Artist,
studio and brand names are left out of the built-in set.

Prompts name reference images `<image1>`, `<image2>`… and leave blanks as
`{slot}` (a short name in braces: `{colour}`, `{scene}`).

Restyling is three tasks, first in the list, split by what they ask of the
user:

- **Lazy Modify Style**: no `{slot}`, so a sweep of any of them needs
  nothing filled in. Prompts read "Convert <image1> into …. Keep the
  composition, pose and identity unchanged."
- **Effortful Modify Style**: at least one `{slot}`. Prompts read
  "Transform <image1> …. Keep the subject recognisable." and may change the
  scene, scale, time or physics, not only the look ("Giant monster", "Split in
  time", "Snow globe"). Slots come from a shared few (`{material}`,
  `{element}`, `{objects}`, `{place}`, `{backdrop}`, `{era}`, `{genre}`,
  `{topic}`, `{surface}`, `{colour}`, `{style}`, `{creature}`, `{animal}`,
  `{plant}`, `{action}`, `{weather}`, `{mood}`, `{amount}` (a price or
  reward), `{flavour}`, `{music}` (a music genre), `{era2}`, …), so a sweep
  of the task asks for few values. Every built-in style has at least two,
  in different roles and varied between neighbouring styles (Giant monster
  `{place}` + `{weather}`, Tiny world `{objects}` + `{animal}`), and
  each has one job, so one value suits every style that has it: `{place}`
  where it happens ("Giant monster", "Billboard"), `{backdrop}` what's
  behind or blended in ("Double exposure", "Dream merge"); `{genre}` a kind
  of story or world ("Genre shift", "Mirror world"), `{topic}` what it's
  about or taken from ("Museum diorama", "Palette stolen from"). There's no
  `{subject}`: prompts call the image's subject that. `{action}` is an -ing
  phrase read after "the subject" ("juggling flaming torches"); the **In
  action** styles use it ("Action shot", "Gone wrong", with `{action2}`
  "Multitasking").
- **Effortful Modify Style — Text**: a slot for the words, quoted in the
  prompt, named by the part they play, so a sweep's title isn't also its
  sound effect: `{title}` (covers, title screens, headlines), `{slogan}`
  (propaganda, signs, banners, mottos), `{name}` (labels, cards, a shop, a
  wanted man), `{dialogue}` (speech and thought balloons), `{sound}` (a
  sound effect), `{caption}` (narration, a cartoon's caption), or `{text}`
  (any, for the user's own). The **Text in a scene** ones put the words in
  a place from `{place}` (Propaganda city: `{era}`): billboard, hologram,
  screens, rally banners, drones, a projection. The **Comics & manga** ones
  draw them as dialogue, a sound effect, a caption or a title (manga page,
  comic cover, four-panel strip, noir narration, editorial cartoon…), with
  `{place}`, `{action}`, `{element}` or `{genre}` beside.

The editor's **Save** and **Save as preset…** refuse a preset breaking its
style task's rule. Presets folders, favourites, deletions and sweep ticks
from before the split (the one task **Modify style**) are moved on load: a
preset by its slots (a words slot such as `{text}` to Text, another slot to Effortful, none to
Lazy), Double exposure to Effortful, and saved so.

Lazy styles that turn a flat image into 3D (**Voxel**, **Animatronic**)
keep only "the subject, pose and identity", so the camera may change, and
carry a negative prompt (Voxel's "pixel art, 2D, flat, sprite, pixelated";
Animatronic's also rules out drawings and a generic sci-fi android): kept to
the composition, the model kept the flat front view and voxels came out as 2D
pixel art.

- **Presets…** (next to **History…** above the prompt, or the prompt's
  right-click menu) opens a window over the images pane (resizable both
  ways; the lists take its height): ★ Favourites and the
  tasks on the left (task descriptions in their tooltips), each task with
  groups a collapsible folder of them (the task shown starts open; click
  the task for all its presets, a group for just that group, and Save
  prompt as preset… then goes into that group), the presets on the
  right (name, description underneath, the full prompt in the tooltip), and a
  search over names, descriptions, groups and prompts in every task.
- Click a preset to **replace** the prompt (and the negative, when the preset
  has one); the old prompt goes to the prompt history ("before preset").
  **Shift+click** inserts it at the prompt's cursor instead. The first
  `{slot}` of what came in is selected, ready to be typed over; with none,
  the cursor goes after it.
- A preset needing more references than the form has (its highest
  `<imageN>`) is greyed out, its tooltip saying how many.
- While the prompt has a `{slot}` (a single run, or a setting sweep), the
  **Blanks** table under it gives each a value, with prompt enhance's
  suggestions and Suggest / Random picks as in a style sweep's Shared
  values (the same values, `sweep.slots`; asked with the prompt alone, no
  styles); the run is queued with them filled in, the form keeping its
  blanks. Typing over a blank in the prompt works too. **Queue** refuses
  while one has no value, selecting it in the prompt.
- With two or more references, **Queue** first asks (a modal, **Queue
  anyway** or Cancel) when some `<imageN>` is named by neither the prompt
  nor any of the sweep's prompts (each style's, or the prompt with its
  blanks filled): likely a tag forgotten. A single unnamed reference is
  usual and isn't asked about. The Images pane's "not named in the prompt
  (or the sweep)" under a reference goes by the same prompts.
- ☆ / ★ marks a favourite (saved at once).
- **Edit presets…**: a modal over a working copy. Tasks (add, delete,
  rename, description), the task's presets (add after the selected one,
  duplicate, delete), and the selected preset's name, group
  (typed, or picked from the task's groups), task (moves it), description,
  prompt (with the references and slots it needs below), negative. **Restore
  defaults** puts the built-in set in the working copy (saved, it removes
  every user preset file). **Save** checks that
  tasks and presets have names, unique (ignoring case; presets within their
  task), and prompts, and the style tasks' slot rules; **Cancel** drops everything.
- **Save as preset…** (above the prompt, the prompt's right-click menu, or
  the window's **Save prompt as preset…**): the prompt and negative as a new
  preset: task (the one shown in the window), name (unique in the task),
  group, description.

### Sweeps

Trying one prompt in many styles, or at many values of a setting, at once. A
sweep is a group of ordinary runs, one per prompt preset or per value, made
by one **Queue**; nothing about generating or storing them is special.

The **Queue** button ("Queue sweep: K runs × N seed(s)", or "Queue N
seed(s)"), **Clear**, **Review N new**, the unsaved-changes note, the time
estimate, why Queue refuses, and the no-servers note are a strip pinned
under the Generate form, so they stay in view however long the form (a
sweep's tables) gets. **Review N new** opens Review on the open project's
unreviewed images (every run and sweep, oldest first, as the project
tile's **Review** does); with none it's greyed ("nothing to review yet"),
and its tooltip says how many more are generating.

- Generate's **Model preset**: the model presets (config `recipes`) the open servers
  can run. Once a server is open the app asks what it can load: an
  sd-server its `model-files`, a started KoboldCpp the files in its model
  folders, one started elsewhere the diffusion model it runs (matched by
  file name). A preset is offered when one server has all its files; the
  others are named under the list ("Not on these servers: …", each
  server's reason on hover), and a model a KoboldCpp runs that no preset
  uses is named too. A started KoboldCpp doesn't count the `.safetensors`
  with int8 (`I8`) weights in its folders, sd-server's `int8_convrot`
  files: it can't load them, and falls back to a full-size buffer (31 GB
  pinned for Qwen-Image's). Queueing a preset no open server runs (a draft
  from before, or a model sweep) asks first, OK/Cancel: "No open server
  can run this model preset", each server's reason, **Queue anyway** or
  Cancel; when a preset of the same sd-server recipe runs (the same model,
  e.g. KoboldCpp's GGUFs for sd-server's int8 files) it's offered first:
  **Use <preset> and queue**. A draft, a run's settings or a restored form
  whose preset no open server runs moves to such a twin by itself, keeping
  its steps and CFG, with a message (projects made elsewhere, like the
  example library's, name presets from another setup). Otherwise, if the
  chosen preset isn't runnable, the first one that is is chosen; until a
  server has answered, every preset is offered. A model sweep's checkboxes
  follow the same list (a ticked preset stays to be unticked).
- The **⚙** beside the Model preset list (and beside the server preset
  editor's Image model line) opens **Model presets**, a modal editor of
  config `recipes`: the list (a ✓ when an open server runs it, naming which
  on hover; new, duplicate, move, delete) and the selected one's name,
  recipe, steps, CFG, flow shift (off: the recipe's), custom sigmas
  (comma-separated; empty: the recipe's) and files: a slot (`diffusion_model`,
  `llm`, `vae`, `clip_l`, …) and a file ID per row, typed or picked from the
  files the open servers list (one no server lists is shown amber). Save is
  refused, with the reason beside it, for an empty or repeated name, no
  recipe, no diffusion model file, an empty file or sigmas that aren't
  numbers. Saving keeps Generate on its model through renames and moves (a
  model sweep's ticks follow renames); if that preset's settings changed
  the form takes them, and if it was deleted the first preset is chosen.
  Other projects' drafts name their preset, so a rename moves them to the
  form's current one when opened. **Reset to built-ins** replaces the list.
  Above the file rows is the model directories list, Server settings' own
  editor (type, browse, ↑ ↓ to reorder, × to remove, Add folder…), saved
  as it's changed (Cancel doesn't undo it), with what the order means: a
  path is the first folder's that has it, LoRAs are in each one's
  `loras/`, and a running sd-server sees a change once restarted. Each
  file row has Browse… (writes the path relative to the model directory
  that holds it, or says why a file outside them can't be used) and Show
  (its folder in the file manager; its hover names the file found).
- A model preset's **LoRAs**: a file and a weight per row (−4–4; 1 as
  trained, below 0 against it), applied to every image. The file is in a
  Model directory's `loras/` (or `Lora/`), named as `loras/x.safetensors`
  (Browse… refuses one elsewhere). Both servers take LoRAs per request
  (`lora: [{path, multiplier}]`, the path inside that folder; sd-server's
  name for one outside the first model directory's `loras/` is looked up in
  its `/sdapi/v1/loras`), so changing them needs no reload and they're no
  part of what a server loads. sd-server finds them in its model dirs; a
  started KoboldCpp is given `--sdlora` with every model directory's LoRA
  folder with each model, lists what it found then (at
  most one folder down), takes at most 10, and skips one it doesn't have
  without a word, so the app checks `/sdapi/v1/loras` first and fails the
  seed saying why. The run and each image's recipe record them (ID,
  SHA-256, weight); the image details and run info show them.
- Generate's **Mode**: **Single prompt** | **Sweep**, and the sweep box's
  **Vary**: **Style** (a prompt preset per run, below) or a setting (see
  *Setting sweeps*). In a style sweep the prompt is a **template**: `{preset}` marks where each preset's prompt goes
  ("cute robot mascot sticker, {preset}"); a template without it is replaced
  by the preset's prompt (a note says so).
- **Styles**: the presets ticked for the sweep, shown as "N ticked ·
  Choose…"; the button opens **Choose styles**, a modal browser (the Presets
  window only picks a prompt). Its left side is a tree: **☑ Ticked (N)**,
  **★ Favourites**, **All**, then each task as a folder of its
  groups (folded at first unless it has ticks or is the one shown; a task
  without groups is a plain row), each with "(ticked / presets)". Above the lists: a search (names,
  descriptions, prompts, every task; Esc clears it), **Filter** (favourites
  only, usable with Generate's reference images, never used in this project,
  has a Good image; the button shows how many are on) and **Sort** (as
  listed, A–Z, most Good, recently used, never used first; kept in the
  config). Each style is a row: a tick, ☆, its newest Good image (else its
  newest image), name, task → group, description, and what it has made in
  every project (Good / Neutral / Bad / unrated, when last used, runs in this
  project; or "never used"). A click on the row ticks or unticks it. Ticks
  whose preset was renamed or deleted (skipped by the sweep) come last in the
  Ticked view under **No longer there**, to untick. The footer gives the
  count and estimate, **Tick all shown** (leaving out those needing more
  reference images), **Untick all shown**, **Clear all ticks**, and **OK** /
  **Cancel** (Esc; the sweep's ticks as they were; clicking outside does
  nothing). It opens on Ticked when something is ticked, else Favourites,
  else All. A draft from before, whose source was ★ Favourites, a task or a
  group, has those presets ticked on opening. **Use** all of them or a
  random K; the pick is repeatable (its seed is kept in the draft) and
  **Reroll** picks again.
- Presets needing more reference images than the form has are left out, and
  the panel says how many. `{slot}`s of the template and the chosen presets
  are asked once and filled in every run; Queue refuses while one is empty.
  They are a table (egui_extras): **Blank** | **Value, in every run** (an
  empty one outlined) | **Needed by** (the template, the style, or "N
  styles" with their names on hover; a blank only one of several styles
  has reads "only <style>" in amber, its tooltip saying the rest don't use
  it and a scene or era for every style goes in the template). Under it, **Each style's prompt (N
  styles)**: see *Style prompts* below.
- **Suggest** (✨, prompt enhance; see *Prompt rewriting*): each blank's
  shared value has a ✨ button (a spinner while
  asked: its tooltip says where the ask is, and a click stops it) and, once
  answered, the values offered as rounded chips to click (filled when it's
  the value), wrapping onto more lines with the row growing to fit; a chip
  shows at most 6 words and 36 characters ("…", the whole value in its
  tooltip); a row's height is what its chips took when last drawn
  (estimated the first time). An "N ▾" menu lists all of them too. The
  two tables are separate boxes, each titled with what it's for: **Shared
  values (N blanks)** and **Each style's prompt (N styles)**, and each folds
  with the caret before its title (for the session). While PE is suggesting values for blanks, its chips appear as
  each answer comes and can be picked (and the "N ▾" menus, and a blank's
  spinner stops its ask), but the rest is locked: the value fields, ✨,
  Replace / Omit, the clear buttons, Random picks, Suggest, and the Styles and
  Use rows. The PE line reads "Suggesting 5 of 13 · … · until it's done,
  only suggestions can be picked" with **Stop**. The values offered are
  the project's: saved in its Generate draft (the sweep's `suggestions`:
  each blank's chips shown, and every value ever offered for it), so they
  come back with the project and don't show in another; clearing the form,
  opening a run in it or restoring a draft keeps them, and switching
  project stops asks still out for the blanks. Each ask names every value
  offered as
  "Already suggested (offer different ones, not these or close variations
  of them)", the latest 60 at most. Small models repeat themselves anyway,
  so an answer's values that are one offered before, ignoring case, or one
  with words added or taken away ("obsidian" / "black obsidian"), are
  dropped. An answer with nothing new (nothing, nothing readable, or only
  repeats) is asked again, at most twice, each 0.2 hotter (to 2.0) and told
  the last answer had nothing new, in the same batch ("· retry 1 of 2" in
  the PE line and the spinner's tip); still nothing, and a message says so
  and the blank shows ⚠ with why ("only repeated values offered before,
  asked 3 times") until it's asked again. A failed request (the server's
  error) isn't asked again. Clicking ✨ again asks for
  more, added to the chips; 🎲 (**reroll**, once some are offered) asks for
  new ones in place of them; 🧹 (once some are offered) clears that blank's
  chips (asks still avoid them) and ✕ at the end of the value field
  empties the value, each leaving the other. The sliders button opens **Suggest…**, a
  modal for that blank: instructions sent with the ask ("only stones",
  "stranger"), how many (1–30), temperature and `min_p` for this ask,
  **Look at the images** (with a vision server and images), **Not the N
  values offered before** with **Forget them**, add to or replace the
  values offered, **Suggest** (a spinner and Stop while asked), the values
  offered as chips to pick (whole, not cut) with **Clear**, the blank's
  value, and **The message sent**, the ask as it would go now. Above the tables: **Clear values** empties every value
  (the shared ones and the styles' own); **Clear suggestions** clears every
  blank's chips;
  **Random picks** gives every blank with suggestions one
  of them at random (again for others); **Basic suggest (N)**
  asks, with the default settings, for every shared one with fewer than 6
  suggestions (whether it has a value or not), and the
  sliders button after it opens **Suggest for blanks…**: a checkbox for
  each shared blank, one a row with how many suggestions it shows
  (highlighted when fewer than How many; ⚠ when its last ask failed) and
  its value (the same ones ticked at first, until settings are kept), and
  **Tick only** buttons that tick exactly those they name and untick the
  rest, each with how many: **All**, **None**, **Fewer than N suggestions**
  (N: How many), **No value**, **Failed ⚠**, the same settings as **Suggest…** (avoiding each blank's
  values offered before), **Suggest (N)**, one ask for each blank ticked
  as one batch, closing it, and the first one's message. Both dialogs
  remember their settings in the sweep, so per project (`draft.rsn`): each
  blank's **Suggest…** its own (with its values offered), **Suggest for
  blanks…** one set and its ticks; temperature and `min_p` only once set
  away from the server's in Settings, which they follow until then. These sit next to the PE state ("starts when
  asked", "starting… 23 s (loading the model; 58 s last time)", "ready",
  "failed: …", and while asking how many and where the first is) with
  **Stop**. A value's request
  names every chosen style using the blank, gives the template, the blanks already filled, and attach the reference
  images (then the start image) when the server has vision, so the values
  suit what the images show. Either leaves the rest of the pick as it was;
  the removed ones are listed with **Put back**, and **Reroll** or another
  source puts them back. A preset's negative, when it has one, replaces the
  form's.
- **Style prompts**: **Each style's prompt (N styles)** has a row per style
  that will run (the references fit it), part of the form (it scrolls with
  the rest, no scroll of its own). A row is a line with its name (group and
  prompt on hover), its group, where it is, ✨ (improve it; a spinner while
  asked, a click stops it), ↺ (back to as built, when it has its own),
  **Replace** (a random pick takes another style; only when one is left)
  and **Omit**, and under it the prompt its run is sent, the form's full
  width, in a text field that grows with it. A row is **as built** (the template with `{preset}` replaced by the
  style's prompt and the values filled in, following them live),
  **enhanced** (prompt enhance's; edited after, still enhanced) or
  **edited** (by hand). One enhanced or edited is the sweep's
  `style_prompts` in the project's Generate draft (the prompt, the prompt
  built when it was made, whether enhanced) and its run is sent that, any
  blank still in it filled from the values; Queue needs no blank left. When
  the template or a value changes after, the built prompt differs from the
  one it was made from and the row says **older values** (hover: the prompt
  as built now; ✨ makes it again); it's kept, not replaced. Editing a row
  back to the built prompt drops its own. A run with an enhanced prompt has
  `enhanced` in its sweep tag. Saved styles' own blank values (before style
  prompts) become their prompts when the sweep is next shown: an edit made
  from the shared values, or dropped if the same.
- Between the two tables: **All back to as built**, then **Basic enhance
  (N)**, which asks, with the default settings, for each style as built or
  enhanced from older values (edits by hand are left alone), a ⚙ button
  for **Enhance…**, and while asking "Enhancing 4 of 6 · …" with
  **Stop**. A row's ✨ asks for that style with the defaults too. Each
  style is one request with the `style_prompt` job's system prompt (built
  in: improve the prompt for the style, keep the subject, the template's
  asks and the style's look, fill every blank, keep `<imageN>` tags, reply
  with only the prompt): the images (512 px, as for blanks, so they're read
  once for all the styles), the template, the style's name and prompt,
  "Improve this prompt:" and the prompt, "Blanks to fill: {…}" when any are
  left, and the instructions when given. **Enhance…** is a modal: a table
  of the styles that will run (a checkbox each, where its prompt is: as
  built / enhanced / edited, "older values" when made from values changed
  since, ⚠ when its last ask failed, and the prompt, whole on hover);
  **Tick only** All / None / Not enhanced yet / Failed ⚠ (each with its
  count, unticking the rest); instructions; **Improve** the prompt as built
  / as shown (an edit or enhancement improved further); **Images**;
  temperature and `min_p` (the server's for `style_prompt` until moved,
  **Default** puts them back); **Longest** (tokens, 400 by default); the
  server's system prompt for it (edited there, **Default**); **The message
  sent** for the first ticked; **Enhance (N)**, which asks and closes, and
  **Close**. Its settings and ticks are kept with the project's sweep
  (`enhance_ask`) and come back as left.
- An answer that's empty or gives back the prompt as sent is asked once
  more, 0.2 hotter, with "The last answer was empty or unchanged: write an
  improved prompt." after the instructions; failing again (or an error)
  leaves the prompt as it was, posts a message and marks the row ⚠ with
  why ("gave the prompt back unchanged, asked 2 times") until it's next
  asked. On one R9700: 1.5–2.4 s a style (about 110 tokens written).
- Blanks an enhanced prompt brings that the prompt it was made from didn't
  have (`{metal}`, `{pole}`) go in **Shared values** like any other (a
  style's own prompt's blanks are listed, needed by and suggested for as
  its preset's are, as Queue asks for them filled). When the batch ends
  (answered or stopped) with any, a modal lists them by style: **Keep as
  blanks**, or **Make them words** (braces dropped, `_` and `-` to spaces:
  `{scrap_metal}` → "scrap metal"); closing it keeps them, as does leaving
  the project before it's answered.
- The summary reads "K styles × N seeds = X images ≈ time"; above 100 images
  Queue asks first. Every style gets the same seeds, so a column of the
  contact sheet compares styles on one seed.
- The sweep takes the form's label (`r05`, one number for the whole sweep);
  its runs are labelled `r05 · Claymation` and tagged in `run.rsn`
  (`sweep: Sweep { name: "r05", task: "Modify style", preset: "Claymation" }`).
  The scheduler's round robin already starts every style before any gets its
  second seed.

**Setting sweeps.** Vary **Steps**, **CFG**, **Flow shift**, **Strength**
(only with a start image) or **Model**: a run per value, everything else as
the form has it, the same seeds in each.

- **Values**: typed (`8, 12, 20, 30`; commas, spaces or semicolons), sorted,
  repeats dropped; **from … to … by … Fill** writes a range (at most 50).
  Choosing a setting starts from values around the form's own (steps 20 →
  `10, 15, 20, 30, 40`; CFG 2 → `1, 1.5, 2, 3, 4`; strength `0.3 … 0.9`).
  Steps are whole, 1–200; CFG 0–30; flow shift above 0 to 20; strength above
  0 to 1. A wrong value is named under the field and Queue refuses.
- **Model**: a checkbox per model preset (the form's ticked to start); each
  run takes its preset's own steps, CFG and flow shift, so models are
  compared at their own settings.
- The prompt is a plain prompt: `{preset}` is refused, other `{slot}`s must be
  filled in. The summary reads "K values × N seeds = X images ≈ time", the
  time summed run by run (steps change it); at CFG 1 or below a note says
  the negative prompt isn't used.
- Runs are labelled `r05 · cfg 3` and tagged with the value
  (`sweep: Sweep { name: "r05", task: "", preset: "", param: SweepParam {
  name: "cfg", value: "3" } }`; `name` is `steps`, `cfg`, `flow_shift`,
  `strength` or `model`, whose value is the preset's name). Older runs
  without `param` read as style sweeps.
- On the contact sheet each row is a value ("20 steps", "cfg 3", "shift 3.1",
  "strength 0.6", a model's name), in order; a CFG row at 1 or below says
  "no negative prompt". The row's button is **Use these settings**.
- Tiling the sheet's picks keeps rows and columns, so with the **style /
  setting · seed** caption ("cfg 3 · seed 5") the tiled image is a labelled
  comparison chart; **settings** captions any image with its run's
  "20 steps · cfg 3 · shift 3.1 · strength 0.6".

**Review.** A project's runs are listed under it in Review's scope, newest
first: a single run by its label (`r03 (unreviewed)`, scoping
Review to that run's images), a sweep once (`sweep r05 (unreviewed)`). Only
the selected project's runs are shown, unless **runs of all projects** is
ticked (kept for the session). The sweep scope adds a **Contact sheet** view: a
row per style, a column per seed, the newest revision of each seed; `…` for
an image still to come, `–` for one cancelled or failed. Cells fill in as
images arrive.

- Click selects a cell, double-click opens the image; 1/G, 2/N, 3/B rate the
  selected image and move on (right, then the next row's first seed);
  arrows move as the sheet is shown; Ctrl+Z undoes.
- Each row: the style, its counts, **G N B** (rate the row's *unreviewed*
  images only), and **Use this style** (its run in Generate as a single
  prompt, with the next label and seed; the old prompt goes to the history).
- **N more seeds for K kept styles** adds a revision of N seeds to every
  style with a Good image (of the styles shown).
- **New sweep from their K styles…** (the picks bar and a picked image's
  right-click menu, in a sweep's scope: the contact sheet, Grid or Single): the styles of the images picked (one
  image of a style is enough; no separate style selection) as a new sweep
  of the same project in Generate, to change and queue. The form is the one
  the sweep was queued with (its checkpoint in the prompt history:
  template, blank values, settings, and the styles' own prompts), ticked to
  just those styles (Use all of them); a setting sweep's picks give its
  values ("cfg 3, cfg 5") or models. The next label and seeds; the form
  there before goes to the history. Without the checkpoint (an older sweep,
  a trimmed history), the first run's settings, the template `{preset}`,
  and each style its run's very prompt as its own (edited) prompt; a style
  no longer in the presets can't be ticked and is named as left out.
- **Filter by name**: only the rows whose style (or run label) has every
  word typed, any case; the seed columns stay. Esc or ✕ clears it; leaving
  the sweep clears it too.
- **Zoom** (− / slider / +, or Ctrl+scroll over the sheet): cells from 64 to
  320 points, the thumbnails' own size, so they stay sharp. Kept in
  `config.rsn` (`sheet_cell`).
- **Rows side by side**: when a row is narrow (one seed, or a few), as many
  rows as fit the window sit side by side in bands, read left to right
  then down (a setting sweep's values stay in order). ↓ / ↑ move a line
  (that many rows), ← / → cross from one band into the next; the large
  preview's ↑ / ↓ still go a style up or down. With one seed its number
  is in the toolbar, not over every band. Ctrl+scroll changes how many fit.
- **Row headers** (the toolbar's Full / Compact / Auto button cycles them,
  kept in `config.rsn` as `sheet_rows`): **Full** (the default) is the
  style, counts, **G N B** and **Use** beside the row (rows fold only if
  those fit too); **Compact** is the style as a caption above the row (⚠
  for "no negative prompt"), with **G N B** and **Use** in its right-click
  menu; a compact caption takes no width, so many more rows fit. **Auto**
  is Full while rows sit one per line and Compact once they would fold.
- Leaving the sweep scope leaves the sheet for Single.
- Looking at a sweep's image anywhere in Review (Single, Grid, A/B compare),
  the top bar shows **Sweep r05 contact sheet**: it opens that sweep's sheet
  with the image selected.

### Tiling

Several images can be picked in Review (Grid, the contact sheet, Single's
filmstrip) and in the Output library's Grid, then rated together or laid
out as one image.

- **Ideas**: the Idea inbox Grid picks ideas the same way (Ctrl+click,
  Shift+click, a box, Ctrl+A, Esc; numbered in pick order). While any are
  picked, a bar shows **N picked · Tile… · New project… · Save to input library… ·
  Discard · Clear**, 1/2/S save them all (one dialog, below) and 3/B discard them, P makes a project from them and T
  tiles them (without picks the keys act on the cursor's idea, as before).
  The Input library's selection gains Shift+click (a range) and the box;
  with several selected the same bar (no Save / Discard) is shown above the
  grid and T tiles them. A picked or selected card's right-click starts
  with what to do with them all; every card with images has **Tile its
  images…**. Tiling ideas tiles all their images, in pick order (the
  grid's order in the library); ideas without images add none.

- **Picking**: Ctrl+click toggles an image (starting from the one the cursor
  is on), Shift+click adds the range from the last picked, a drag on empty
  space draws a box (Ctrl+drag adds to the picks), Ctrl+A picks everything
  shown. A plain click, a click on empty space or Esc drops the picks; so
  does another tab, scope or source. Picks are numbered in the order picked,
  which is the order they are tiled in.
- While anything is picked, a bar shows **N picked · Good Neutral Bad ·
  Tile… · Clear**, and in Review 1/G, 2/N, 3/B rate every picked image (each
  is a separate Ctrl+Z). Rated images leave Review's queue, so the picks go
  with them there; the contact sheet keeps them.
- **Tile…** (T) opens the Tile window (a modal: the rest dims; Close or
  Esc closes it, a click outside doesn't, so the layout isn't lost by
  accident). Picks from the contact sheet keep
  their rows and columns as shown (bands side by side; holes included); others start about square
  (⌈√n⌉ columns). Drag a tile onto another to swap them, or onto any empty
  cell (an extra row below takes drops); click one to give it a caption or
  a span (2 × 1, 2 × 2, …: the tiles in its way move to free cells). The
  **×** on a hovered or selected tile, Delete (or Backspace) on the
  selected one, or its right-click → **Remove from the tiling** takes it
  out (the image itself stays where it was).
  **Columns** re-places what no longer fits; **Reflow** packs the tiles in
  order.
- **Text blocks**: right-click an empty cell (or double-click it) → **Add
  text here**. A text block is a tile like an image (drag, span, remove)
  holding lines of text, word-wrapped and centred in its cells, in the
  caption colour; it has no caption and isn't a source.
- Generate's Images panel has **Tile…** (by Add file… / From library…):
  the Tile window with all its images (references, the start image and
  parked ones), in the strip's order, about square; the picks are untouched.
  Greyed with none. The start image's `{kind}` is "start".
- **More images**: **+ Images…** (or an empty cell's right-click → **Add
  images…**) opens the reference picker (Generate's From library…) for the
  tiling: the chosen go from that cell on (the next free cells, row by
  row), else after the last tile, in the order chosen. Its sources gain
  **Inputs of these images** (the tiled outputs' start images and
  references) and the **Idea inbox** (for Generate too); images already
  tiled are marked but can be chosen again.
- **Inputs**: **+ Inputs ▾** adds what the tiled outputs were made from
  (start image, then references): **as a column** (each row's inputs in
  new columns at its left; one input on rows one after another spans
  them) or **before each image** (input → output pairs, repeated where
  shared; twice the columns). An image tile's right-click → **Add its
  inputs here** puts them just left of it (in the empty cells there, else
  its row moves over, widening the grid if it must). Greyed for images
  made from text only.
- **Pasted images**: Ctrl+V over a cell (or right-click → **Paste image
  here**, or **Paste**) adds the clipboard's image; image files' paths or
  `file://` URIs as text are read instead; image files dropped on a cell
  are added too. They are held in memory for the session, are not sources
  of the tiled idea, and fit into the cells, which are sized by the
  outputs (by the inputs or pasted images only when there are no outputs;
  the preview reads their sizes from the files).
- **Tile the clipboard…** starts a tiling from what the clipboard holds
  (or adds it to the open one). It's in the right-click menu on the empty
  background of the Folder views and of every grid: Review (Grid, the
  contact sheet, Single's filmstrip), the Output library Grid, the Idea
  inbox Grid and the Input library Grid. With picks, the same menu starts
  with **Tile the N picked…**.
- **Title / subtitle**: lines centred above the grid (a gap below them), the
  subtitle at half the title's size, in the caption colour. Their text
  belongs to the layout, not the settings.
- **Text sizes** (captions, the title, each text block) are **auto**, a
  **% of the cell**'s height, or exact **px**; the panel shows what each
  comes to. Auto: captions 5 % of the cell; the title 4 % of the tiled
  image's width, at least 10 % of the cell (so more columns, a larger
  title); a text block as large as fits its cells, up to the title's size.
  Auto and % follow the scale, so 25 % looks like 100 % only smaller.
  Switching keeps the size it came to. A caption too wide shrinks (down to
  60 %) before it's cut short with "…".
- Cells are the size of the largest image, or a set **cell** size (w × h
  px; **auto** unticked starts from the largest image's), at the chosen
  **scale** (25 / 50 / 100 % or any), with a **gap** between and around them; images
  **fit** (letterboxed) or **fill** (cropped to the cell). **Background**:
  transparent, dark, white, black or any colour. **Captions**: one line
  under each tile (shrunk, then cut short with …), all filled from the style or setting
  (a sweep's row: "Claymation", "cfg 3"), that and the seed, the seed, the
  settings ("20 steps · cfg 3"), title, run label or prompt, then edited
  one by one; size (above) and colour. A caption can hold placeholders, filled per
  image when shown and composed: `{n}` (the tile's number), `{style}`,
  `{seed}`, `{settings}`, `{steps}`, `{cfg}`, `{shift}`, `{strength}`,
  `{title}`, `{run}`, `{prompt}`, `{project}`, `{kind}` (output, input,
  start or pasted); unknown ones stay as typed. Captions from… and
  **Format … Apply** (the format is kept in `tile`) fill generated images'
  captions only: inputs, marked-up copies and pasted images have no seed,
  run or settings, so they keep the captions written by hand (Captions
  from **clear** empties them all). The
  settings are kept in `config.rsn` (`tile`); the layout is not.
- **Copy** puts the tiled image on the clipboard, **Save {format}…** writes a
  file in the format chosen under **Save**: PNG, WebP (lossless, at most
  16383 on a side), JPEG (transparency flattened onto white) or AVIF (only
  in builds with the `avif` feature); the lossy ones (JPEG, AVIF) have a
  quality slider, 70 % by default. The file name's extension follows the
  format. Both are kept in `tile` (`save_format`, `save_quality`); **Add to input library** makes a new idea at the top of the input
  library with the image, its source `Tiled { images }` (they count as used,
  so Clean up keeps them). Composing reads the full-size images on its own
  thread; over 250 MP is refused.

### Crop and mark

**Crop / mark…** crops an image and draws over it, mainly to show the edit
model what to change ("replace the cup circled in red in `<image2>`").
It opens from a Generate image (its pencil button or right-click menu)
and from the image right-click menu everywhere else (Review, both
libraries, the full-size preview, the Tile window's tiles).

- The editor is a modal over nearly the whole window: a toolbar, the
  image, a footer. Tools: **Crop** (drag a box; drag inside it to move it,
  a corner to resize; aspect free, the original's or Generate's size;
  optionally rounded to multiples of 32 when the drag ends), **Pencil**,
  **Rectangle** and **Ellipse** (Shift: square / circle), **Eraser**
  (removes whole marks). A colour (six swatches or any), an opacity
  (25, 50, 75 or 100%; 100% to start; lower lets what's under a mark
  show), a width in thousandths of the image's longer side (3 to start). Keys C, P, R, O, E; Ctrl+Z /
  Ctrl+Shift+Z; scroll zooms about the pointer, right- or middle-drag pans,
  0 fits. Enter saves; Escape or Cancel closes (a click outside doesn't: it
  would lose the marks). Footer left: **Clear marks**, **Reset crop**.
- What is drawn stays vectors in the original's pixels
  (`ImageEdits { crop, marks }`) until Save; the copy is drawn on the
  library's thread (marks with soft edges, each blended once at its
  opacity, so a line crossing itself isn't darker; then the crop;
  transparency kept) and imported into the input store.
- The copy is an idea in `input/edited/` with the source
  `Edited { image, edits }`: out of the inbox, the input library's lists and
  counts. **show edited copies** in the Input library's filters lists them
  (with no folder selected); the library picker has **Edited copies,
  hidden**; an idea's details list its images' **Edited copies**, and a
  copy's Source shows what it was made from. Clean up keeps the original
  (it can be edited again).
- From Generate, Save puts the copy in as a reference right below the
  image, which keeps its role and `<imageN>` (later numbers move on, asking
  to retag the prompt as usual). Opening an edited copy reopens its
  original with the crop and marks as drawn; from Generate, Save then
  replaces that copy in place (same `<imageN>`). Elsewhere Save only makes
  the copy; **Save and use in Generate** also adds it to the selected
  project's form. Each save makes a new copy; earlier ones stay.

### Opening the file manager

**Open in file manager** / **Show in file manager** (folders, projects,
idea folders, collections, the drop folder, images, the library path in
the status bar) opens the folder (a file's folder) with `file_manager`
from `config.rsn` if set (the command, the folder appended), else
Explorer on Windows, Finder (`open`) on macOS, and on Linux the first of
`xdg-open`, `gio open`, `thunar`, `nautilus`, `dolphin`, `nemo`, `pcmanfm`
that works; if none does, the status bar says how to set one up. Text
files (config, logs) open in `editor` if set, else Notepad on Windows,
TextEdit (`open -t`) on macOS, and on Linux the desktop's app for them.

### Server logs

The servers IncreArt starts write their output to the log folder, `logs`
in the app's state folder (`$XDG_STATE_HOME/increart` when set, else
`~/.local/state/increart` on Linux, `%LOCALAPPDATA%\increart` on Windows,
`~/Library/Application Support/increart` on macOS):
`sd-server-<name>.log`, `koboldcpp-<name>.log` (KoboldCpp image servers),
`pe-<name>.log`. Servers it only connects to have none.

- What a server writes (its output and errors, one stream) is timed as it
  comes: each line starts `HH:MM:SS.mmm `. A server left running when the
  app quits (`leave_running`) writes to the file itself, untimed.

- Each start first adds a line of the app's own to the log:
  `--- IncreArt, <date time>: <command line>` (the environment it set
  first, such as `CUDA_VISIBLE_DEVICES=1`; passwords as `***`). A server
  that can't be started at all gets `--- IncreArt: <why>` after it, so the
  log says why even when the server wrote nothing: a binary that isn't
  there is named as such ("koboldcpp not found: it isn't a file in the
  working folder, and isn't on PATH"; with a folder, "… isn't there").
- A KoboldCpp image server starts with no image model; each load of a
  run's model adds `--- IncreArt, <date time>: reloading with <file>.kcpps,
  the settings for: <command line>` (the launch's plus `--sdmodel`,
  `--sdvae`, `--sdclip…` for the recipe's files). Load errors are looked
  for in what the server wrote, not in these lines.
- The preset editor shows each started server's **Command line**, as its
  log's first line will (the admin password as `***`), with Copy.
- Under a preset's image servers a greyed **Image model** line says no
  image model is set there: each run's comes from its model preset (files
  in the model directory); its ⚙ opens Model presets. A KoboldCpp entry's
  Chat model is the one model a server preset names.
- On the Servers screen a failed server's row says **failed**, and why on
  its own lines under it, wrapped to the dialog's width; of a long log tail
  the first line and the last two (all of it on hover). A started server
  whose binary isn't there ("… not found", "… isn't there") also gets
  "Set its binary in **Server settings…**", a button that opens them.
- **Logs…** (the startup Servers screen's footer, the top bar's Servers
  window) opens the log viewer, a modal; a started server's row there has a
  log button that opens it on that server's log.
- Left: the logs by kind (App · Image servers · Prompt enhancer · Other),
  newest first in each, named for what they are ("KoboldCpp kcpp-gpu1",
  not the file name; it, the size and age on hover). A server in use has
  its state beside it (ready green, failed or retrying red); a log with an
  error since its latest launch is red with a ●.
- Right: the chosen log's last 1 MB as a terminal shows it (lines
  rewritten with `\r`, such as progress bars, as their last state; colour
  codes out), refreshed as it grows; **follow** keeps to the end
  (scrolling back, by the wheel or the bar, unticks it; ticked again, it's
  back at the end), **wrap** (the last 3000 lines, wrapped to the width).
  When the last 1 MB starts after the latest launch line, that line is
  pinned above it.
- Launches: the latest launch's lines to start with (chosen from the
  list, it's always that); the menu picks an earlier one by its time, or
  **all**. A KoboldCpp's reloads are part of their launch.
- Easier to read: times dimmed; launch lines bright; errors red (sd.cpp's
  `[E]`, "error", "failed", "out of memory", "panicked", a traceback… not
  "no error"), warnings amber; sd.cpp's ` --- file.cpp:N` endings
  dimmed. Runs of 8 or more lines of one kind (`print_info: …`,
  `load: …`, progress bars) fold into one "▸ print_info: 66 lines"
  (clicked, they show; again, they fold), unless a warning or error is
  among them; a line repeated is shown once with ×N.
- **errors only**: the warnings and errors (and the launch lines), each
  with the 2 lines around it, ⋯ between.
- Find (Ctrl+F, or the box): matches marked, the one gone to in amber,
  "N of M"; typed, it goes to the latest; Enter or F3 the next, Shift for
  the one before (or ↑ ↓), which unticks follow. **only these lines**
  shows only the lines containing it.
- Footer left: **Open in editor** (`editor` in `config.rsn`, the file
  appended; else the desktop's default app for it, then gnome-text-editor,
  gedit, kate, mousepad, xed), **Show in folder**, **Copy** (the lines
  shown). Footer right: **Save for a bug report…**, then Close at the
  edge. Esc or Close closes.

The app keeps a log of its own there too, `increart.log`, listed with the
servers' in the viewer (titled **Logs**):

- At each start, `--- IncreArt, <date time>: started, increart <version>
  (<commit>); config: <path> [(INCREART_CONFIG)]; arguments: …`.
- Every message the Messages list gets, as `<date time> info|error|alert:
  <text>` (config warnings at start among them), and a panic's, as
  `<date time> panic: thread '<name>' at <file>:<line>: <what>`.
- Grown past 2 MB, it's moved to `increart-previous.log` at the next start.
- A screenshot tour (`INCREART_TOUR`) keeps none.

**Save for a bug report…** asks where (Downloads to start with, named
`increart-bug-report-<date>-<time>.zip`; `.zip` added to a name without it)
and writes, on a thread of its own, a zip whose folder is named after it:

- `config/`: `config.rsn` and the copies kept beside it (`config.rsn.*`:
  unreadable ones, ones from before a migration).
- `logs/`: each log in the log folder, its last 2 MB ("[the first … MB left
  out]" above when cut), passwords masked (`…password='***'`,
  `--…password ***`: KoboldCpp prints its admin password as it starts).
- `model-directories.txt`: each model directory in order, the files under
  it (four levels deep, at most 3000) with their sizes, or that it isn't
  there.
- `about.txt`: the version and the start line's config and arguments, the
  system, the config and log folders, when, and what's in the zip.

No images, prompts or library. Its hover says so, and that paths show the
user name; saved, a message names the zip and its size.

### Project folders

Projects are organised in a tree of folders and subfolders, and the tree is
real directories under `projects/`, so it looks the same in a file manager.

- A directory containing `project.rsn` is a project; any other directory is a
  folder. Projects don't nest inside projects.
- A project's identity is the ID in `project.rsn`, never its path. Ideas
  (`projects: Vec<ProjectId>`), recipes and the scheduler refer to projects
  by id, so renaming or moving a project or folder breaks nothing. The
  `ImageRef` points into a store, never into a project, so it is unaffected.
- The scheduler resolves a project's current path when it writes a result, so
  moving or renaming a project mid-run is safe.
- Names are directory names: not empty, no `/` or `\`, not starting with `.`,
  and unique among siblings, case-insensitively. Folders and projects share
  one namespace per parent.
- Tree operations (right-click on a folder, a project, or the empty area for
  the top level):
  - **New folder**: created with a free name ("new folder", "new folder 2",
    …) and opened for rename.
  - **New project**: asks the name first (the same modal, titled New
    project, prefilled with a free "new project" name); the project is made
    on **Create**, and Cancel / Escape makes nothing.
  - **Generate** (on a project, first in its menu, above Show outputs):
    selects it and opens Projects → Generate.
  - **Rename**: a modal over a dimmed window (also from `F2` and the
    folder views, and for collections). Enter or **Rename** commits;
    Escape, **Cancel** or a click outside cancels. An invalid name disables
    Rename; a clashing one is refused with a message.
  - **Copy** / **Cut** then **Paste here** on a folder (or the top level).
    Cut + paste moves (a directory rename). Copy duplicates a project's setup
    — `project.rsn` with its sources and run definitions, as unqueued
    templates — but **not** its images; copying a folder copies its subtree
    the same way. A pasted name that clashes gets a number suffix. Pasting a
    folder into itself or a descendant is refused.
  - **Discard…** (a project) / **Delete…** (a folder): confirmation
    showing the folders, projects and images inside and how many pending
    seeds won't be generated. Each project moves to `discarded_projects/`
    (numbered if the name is taken there) with a `discarded.rsn` noting its
    name, the folder it was in and when; it leaves the tree, the queue,
    Review and the Output library, and shows under **Projects → Discarded**.
    A folder itself goes to the system trash once its projects are out.
  - **Drag and drop** moves, like cut + paste: drop on a folder to move into
    it, on a project to move beside it, or on "All projects" / the empty
    area for the top level. The target is outlined blue, or red when the drop
    is refused (a folder into itself); dropping where the item already is
    does nothing.
- Keyboard shortcuts act on the selected node, on the Projects tab, whenever
  no text field has focus: `F2` rename, `Delete` delete (with the same
  confirmation), `Ctrl+C` / `Ctrl+X` copy / cut, `Ctrl+V` paste into the
  selected folder (or beside the selected project, or the top level). These
  don't clash with Review's keys: Review uses no F-keys or Delete, and
  egui delivers Ctrl+C/X/V as clipboard events, not key presses, so they
  never trigger Review's `C`. Copy/Cut also put the node's path on the
  system clipboard as text, because egui only reports Ctrl+V when the
  clipboard isn't empty.
- Selecting a folder scopes Review to every project beneath it (the "all
  projects" view is the top level). Generate needs a single project.
- Folders collapse: the caret before a folder (or a double-click on it)
  hides what it holds, and **Collapse all folders** / **Expand all folders**
  on "All projects" act on the whole tree. A collapsed folder shows its
  unreviewed count and a sparkle when one of its runs is active, and is
  accent-coloured when the open project is inside. Searching shows matches
  inside collapsed folders without expanding them. Which folders are
  collapsed is remembered per library in the config
  (`collapsed_folders`); deleted or renamed folders drop out.
- New projects from ideas land, unless another folder is chosen, in a
  folder of their own name where the last one went, else at the top level.

### Input library folders

The input library has the same kind of folder tree as projects, stored as
directories under `input/library/`, with the same tree operations, drag and
drop and shortcuts (see Project folders). The differences:

- Leaves are ideas. An idea's directory is its ID, so folder names must be
  unique among sibling folders but idea titles may repeat. Renaming an idea
  edits its title (free text, may be empty).
- Kept ideas from the Idea inbox land at the top level; filing them is a
  separate step.
- **Copy** duplicates an idea (new ID, same text, variants and tags;
  new links to the same store images, nothing is copied; `projects` starts empty). Copying a
  folder copies its subtree.
- **Delete** on an idea discards it (to `input/discarded/`, undoable with
  `Ctrl+Z`), with no confirmation. Deleting a folder asks first, then removes
  the folder and discards every idea inside; `Ctrl+Z` brings those ideas back
  at the top level.
- Cards in the grid are drag sources too: dragging a selected card carries
  the whole selection, so several ideas can be filed at once. With several
  cards selected, `Delete` and `Ctrl+C`/`Ctrl+X` act on all of them.
- Selecting a folder limits the card grid to that folder and its subfolders;
  the other filters (text, starred, has images) apply on top.

## GUI

Top-level tabs, in pipeline order after **Home**: **Idea inbox** · **Input library** ·
**Projects** (Grid · Folder · Generate · Jobs · Discarded; shows the project count, and
✦ while generating) ·
**Review** (the output queue) · **Output library**. Review is top-level, like
the Idea inbox, because it is the busiest screen during batches and works
across projects. Clicking a server status in the top bar (ROCm0 ●, PE …) opens the
server status window (as the Servers… button does); the PE status's tooltip
shows the rewrite queue. The statuses take the room left of the top bar's buttons: in full
(`ROCm1 ◐ warming up`), then name and dot, then dots only; with no room for
those the Servers… button shows the worst dot (and, in a narrow window, only
its icon; the library name is cut to 10 characters). Tooltips give the full
status. In full, a status takes a fixed width (180 px, cut with "…"; the PE's
without the chat server's name, `PE ● ready`), and which form fits is worked
out from those widths, so the bar doesn't move as states change many times a
second. The same goes for the servers list (Jobs, its popover, the server
status window: the state column a fixed 240 px, cut), the jobs popover (a
fixed 520 px) and the server status window's PE line (Start now / Stop
before the state, which is cut to the room left). The window is at least
1000 × 640.

Each tab label is an icon, a Title Case name and a count (Home has none):
🏠 **Home**, 📥 **Idea Inbox
(7)**, 📚 **Input Library**, 🚀 **Projects** (✨ while runs generate),
👍 **Review**, 🖼 **Output Library**. When the full labels would leave the
right side less than its compact width (a narrow window or a large zoom),
every tab drops its name and keeps its icon and count; the tooltip names it.

**Back / forward** (← → at the top left, `Alt+←/→`, the mouse's side
buttons): a history of places like a browser's. A place is the tab, the
sub-tabs, and what the side panels select (project or project folder,
input-library folder, Review scope, Output library source); each user action
that changes it is one step (Show outputs, which switches tab and source, is
one). A new step drops the forward ones; up to 100 are kept. In a folder view
the keys and mouse buttons go through that view's own history instead.

- **Home** (the tab the app opens on): how the tabs fit together, as six
  numbered cards in rows (Collect: scraped or downloaded files saved into
  the drop folder, dragged in, or written; Sort the inbox: keep or discard
  into the Input library; Input library; Projects; Review; Output library),
  each with its keys, a count and a button to its tab (Collect's opens the
  drop folder, and counts only files waiting there: they are imported as
  soon as they have finished copying). Above them: **Start a
  new project…**, **Write an idea**, **Import images…**, **Open the drop
  folder**, and links to the ideas and images waiting. Below: the six
  projects with the latest runs. **Start a new project** asks the name
  (`new project`, numbered if taken; made at the top of `projects/`) and
  whether to add reference images: from the libraries (the default; From
  library… opens on its Generate form once it is made), image files (Add
  file…'s picker), or not now.
- **Idea inbox**: tabs **New idea** · **Grid** · **Folder** · **Import** ·
  **Discarded**. (PE rewrites have no tab; see Prompt rewriting.) Files dragged from the desktop onto any Idea
  inbox tab are imported as new ideas (images re-encoded as usual, text files
  become the idea's text); the window shows a drop highlight while dragging. Opening the Idea inbox shows **New idea** when there are no
  ideas to review, **Grid** otherwise. Shows the count of unreviewed ideas as a
  badge on the top-level tab.
  - **Grid** (input review): idea cards (title, first images, text, variant
    count, PE rewrite state), newest first. Idea cards here and in the Input
    library are all one size: the first image square across the full width
    (a "T" when there is none), then the title, a line of counts, two lines
    of text and the tags, each cut to fit. On every idea card (here, the
    Input library and Discarded), double-click shows the first image full
    size (the editor when there's no image), and a pencil button on the
    image (on hover, or when selected) opens the editor. Hovered or
    selected, the image also shows two buttons along its bottom, as on
    project tiles: here **Save** and **Discard** (`S`/`3`); in the Input
    library **New project** and **Add as refs** (New project alone for a text-only
    idea); none in Discarded. They act on that card only; on a small card
    (zoomed out) they're icons, the name on hover (`widgets::cover_buttons`).
    The side panel showing the current idea keeps its buttons pinned to its
    bottom (here **Save to input library…** / **Discard**, then **New project**,
    **Edit…**, **Copy text**, **Open folder**) and its header at the top;
    the details scroll between. Keys as in the
    table above, plus `Home`/`End` = the first / last idea, `Enter`/`T` = open the **Edit idea** window (title, tags,
    text, variants, images, and a finished PE rewrite's diff with Accept /
    Reject). Right-click a card, in groups: new project from idea (`P`), edit (`Enter`),
    rewrite with PE (in the Edit window, so Use changes its text) · save to
    input library / discard (`S`/`3`) · copy
    text, copy image and describe image with PE (its first image) · open
    folder in file manager. (Planned: accept all ready rewrites.)
  - **Save to input library** (the card's Save, the side panel's button,
    the menus, `1`/`2`/`S`; the New idea form's **Add & save to input
    library…**, for the draft) always asks first, in a modal. Left, the
    **Folder**: a filterable tree of the input library's folders, **New
    folder** under it (named in the folder chosen; an existing one of that
    name, any case, is used). Right, the **Name** (the idea's title to start
    with; renames it; several picked ideas keep theirs and the field is
    hidden), and where it's **Saved in**. The name has the focus as it
    opens, a new folder's name once asked for; neither takes it back from a
    field clicked since. It starts in the folder the last save went to (this
    session). **Save** (`Enter`) moves the ideas there unstarred, making the
    folder; **Save & new project** saves them, then makes a project
    from them at once, named as the idea (the first valid title of several),
    where the last project from ideas went, in its own folder (the New project dialog asks
    when that name won't do; the draft is saved in the folder chosen, not
    the top level); **Cancel** (`Esc`) leaves them in the inbox (the draft in
    the form). Discard doesn't ask.
  - **New idea** (`Ctrl+N` from anywhere): title, text, tags, images (drop /
    paste / browse), **Rewrite with PE**. **Add to inbox** (review it later),
    **Add & save to input library…** (asks the name and folder, as below),
    **Add & new project…** (keep it and start a project). The draft is kept while
    switching tabs.
  - **Folder**: the file-manager view (see Projects → Folder) rooted at the
    inbox's own places only: `inbox_drop/` (waiting files, and `imported/`
    originals), `unreviewed/` (one folder per idea, holding its image links)
    and `discarded/`, with a short Places list instead of a tree. Its
    location and history are its own, independent of Input library →
    Folder. Moving an idea folder between `unreviewed/` and `discarded/` (drag,
    or cut + paste) discards or restores it, undoably; `Del` on a discarded
    idea asks to delete it permanently. `unreviewed/` has no subfolders: the
    inbox is a queue, filing happens in the library. Files in `inbox_drop/`
    can be opened (= imported now) or deleted; `imported/` originals can be
    viewed or deleted. `F2` on an idea opens the Edit idea window.
  - **Import**: drop-folder watcher status and files waiting (with
    **Import now**); recent imports with what changed on import (from
    `increart:source`) and duplicates detected by pixel hash ("existing
    image reused"); import settings (strip GPS, delete `imported/`
    originals after N days, clear them now; see Open questions).
  - **Discarded**: discarded ideas (`input/discarded/`), newest first.
    Right-click: restore to the Idea inbox, restore to the input library ·
    copy text, copy image, describe image with PE · delete permanently (asks first; the idea's folder goes to the system
    trash, its images stay until Clean up). **Empty discarded…** deletes them
    all. Restores can be undone with `Ctrl+Z`.
- **Input library**: tabs **Grid** (default) · **Folder** (see Projects →
  Folder) · **Discarded** (the same Discarded view as in the Idea inbox). In
  Grid, the left panel holds the filters (tag / starred / text /
  has-images / **used in a project** or not), selection actions, and the library folder
  tree (see Input library folders); the grid shows the selected folder's
  ideas, newest first (or most used first); cards show PE rewrite state like the inbox's. Multi-select → new project
  (button or `P`/`Space`); ← → ↑ ↓ and `Home`/`End` select the previous /
  next / first / last idea and show it in the panel, `Ctrl+A` selects all,
  `Enter`/`T` edits; a **Keys** list is pinned at the bottom left; drag an image into a project's reference strip; drag
  cards onto a folder to file them. Right-click a card, in groups as in the
  inbox: new project, edit, rewrite with PE, add its images as refs to the
  current project, star/unstar · back to the Idea inbox, discard · copy
  text, copy image, describe image with PE · open folder in file manager.
  (Planned: accept all ready rewrites.) When the card is part of a multi-selection, the
  bulk actions (star, new project, rewrite, back to inbox, discard) apply to the
  whole selection. A plain click on a card opens the **idea panel** on the
  right: the idea's images (double-click: full size), text, tags, created
  date and folder, what its images were used for, the projects it was
  **used in** (links), and Edit…, New project, Add as refs, Copy text, Open
  folder pinned to its bottom (the details scroll above them); × closes it. While the panel is closed it opens a moment after
  the click, so a double-click (full size) doesn't open it; while it's
  open, a click shows the clicked idea at once. Ctrl/Shift+click only
  selects.
- **Projects** side panel: **New project** and **New folder** (at the top
  level; a folder's right-click menu makes them inside it) at the top, then a search field that narrows the tree to
  the projects and folders whose names contain the text (a matching folder
  shows all it holds; `Esc` or × clears it), **runs** (list the selected
  project's runs under it), the project and folder orders (**Created**:
  newest made first; **Last run**: most recently queued first, a project
  that has never run by when it was made; or **A–Z**, any case; a folder
  goes by the newest project anywhere in it, empty folders last; saved in
  the config as `project_order`, default Created, and `folder_order`,
  default A–Z; the grid uses the project order too) and **narrow to
  siblings** (below; ticked by default, unticked saved as
  `project_click_keeps_scope`), then the tree: in each folder, its
  subfolders in their order, then its projects in theirs. A **Keys** section is pinned to the bottom (open
  by default, foldable): on the Folder tab the folder view's keys, else an
  opened image's (← → previous / next, rate, undo, E / D / R / I, zoom, Esc),
  and Alt+← → / Ctrl+N from anywhere.
- **Projects → Grid**: a flat overview: one tile per project in the folder
  selected in the tree and all its subfolders, in the tree's order, no
  folder tiles. Selecting a project (in the grid or the tree) narrows it to
  the project's folder only when that holds other projects (here and below)
  and **narrow to siblings** is ticked; otherwise the grid keeps showing what
  it did, if the project is in it, else the project's folder (the one above
  when the project is alone in it, as a project from ideas' own folder is). After
  Back / Forward that last rule decides. Each tile's cover is its best image: highest
  rated (good > neutral > unreviewed > bad), most recent among equals. A
  project with no output image yet shows its references and start images
  (its Generate draft's, then its runs', up to 8) as small thumbnails
  scattered around the empty cover, clear of its icon in the middle; the
  tile also shows the project's folder path. Tiles show the unreviewed count as a badge on the
  cover, good / neutral / bad / unreviewed counts below it, and a
  marker while runs are generating. A breadcrumb changes the scope; click a
  project to select it, double-click it to review it; right-click gives the
  same menu as the tree.
- **Projects → Grid and Folder: the details pane** on the right (300 px),
  shown by default; × or the tab bar's **Details** toggle hides it (for the
  session, like the Output library's). Its buttons are pinned to its
  bottom, the rest scrolls. It shows:
  - **A project** (selected in Grid; in Folder, selected or the one the
    view is in): cover, name, folder, created and last run; counts (runs,
    images, good / neutral / bad, new to review, seeds queued); its source
    ideas (a click opens one in the Input library); the latest run (prompt
    with a copy button, size, model, steps / CFG, mode, input images);
    **Recent runs**, newest 8 (label, images, date; a click shows its images
    in the Output library, ⓘ its details); and its best outputs (good, then
    neutral, then new). Buttons: **Generate**, **Review N**, **Show
    outputs**, **Open folder**; Rename, Duplicate and Delete stay in the
    right-click menu.
  - **A run** (Folder: selected, or inside `runs/<id>/`): label, project,
    prompt and negative prompt, settings, input images, images made, seeds
    queued / cancelled / failed, time per image and its revisions. Buttons:
    **Open in Generate** (a new run with its settings), **Show outputs**,
    **Details** (the run window).
  - **An image** (Folder): the Output library's Lineage pane.
  - **A folder** (Grid with no project selected, or Folder): totals for the
    projects under it, and those with images to review. Buttons: **New
    project** (in it), **Show outputs**, **Open folder**.
  - **Several entries** (Folder): their count.
  - Later: size on disk, the project's "use outputs as references" setting,
    and a note when Generate has unsaved changes.
- **Projects → Folder**, and the same as **Input library → Folder**: a
  file-manager view (like Thunar) of the tree's on-disk layout. For projects
  that includes each project's `runs/`, `unreviewed/` and
  `reviewed/{good,neutral,bad}/` with the image links; in the library an idea
  is a folder holding its image links.
  - Toolbar: back / forward / up (`Alt+←/→`, `Backspace`), a path bar of
    clickable segments that turns into an editable path with `Ctrl+L`, a
    metadata toggle (`Ctrl+H`, shows `project.rsn` / `run.rsn` / `idea.rsn`),
    and **Icons** (with a size slider) / **Details** (name, kind, info, size,
    modified) / **Compact** modes. Directories are plain folder icons; image
    files show thumbnails. The view buttons go to a second line when they
    don't fit beside the path.
  - Side pane: the tree, kept in sync both ways.
  - Selection: click, `Ctrl`-click, `Shift`-click, rubber band on empty
    space, `Ctrl+A`, arrow keys, type-ahead. `Enter` or double-click opens: a
    folder navigates, an unreviewed image opens in Review, a reviewed one in
    the Output library, an idea image opens the Edit idea window.
  - Operations: `F2`, `Del`, `Ctrl+C/X/V`, drag and drop and a right-click
    menu (open, open in system file manager, rename, copy, cut, paste into,
    delete, properties); empty space has its own menu (new folder / project /
    idea, paste, open in system file manager, properties). Folders, projects
    and ideas behave as in the tree. Image links can be moved (cut + paste or
    dragged) between `unreviewed/` and `reviewed/*/` of their own project,
    which is a review (undoable); moving them into another project is
    refused (use → input ref). They can't be renamed (the name carries run,
    revision and seed) or copied; deleting one moves the link to the trash,
    the image stays in the store until Clean up. Project subfolders, idea
    images and metadata files are managed by the app.
  - Status bar: item count, selection count and size, current path (cut
    short when narrow).
  - Later: a dual-pane split (`F3`).
- **Projects sidebar**: the project folder tree (see Project folders), with
  per-project unreviewed counts; the selected project lists its runs under
  it, newest first, as in Review's scope (a single run by its label, a
  sweep once as `sweep r02`), each with its unreviewed count and ✦ while
  running. Click: the run's details (a sweep's newest run's); right-click:
  Review this run / the contact sheet, Show outputs (a single run), Details,
  Open in Generate, Show in history. The **runs** checkbox above the tree
  hides them (kept in `config.rsn`, `hide_project_runs`).
- **Generate panel**: prompt (+ **Rewrite** with diff; **Presets…** and
  **Save as preset…**, see Prompt presets), negative, size presets
  from `config.rsn`, editable, plus a one-off custom size (see Config file →
  Resolutions), steps/cfg/flow-shift,
  seeds count and first seed (see Seeds), preset recipe, **Transparent** toggle, reference strip
  (chips tagged `<image1>`…, "N / 9"), with **Add image file…** (system file
  picker; imported into the input store), **From library…** (a picker over
  nearly the whole window, browsing like the library screens: a source tree
  with the input library and its folders, and the output library's
  projects, collections and smart folders (by project as a tree, by date,
  by source idea, transparent); a text filter (idea title, text and tags, or
  the run's prompt); Kept / Good / All for outputs; a thumbnail size slider.
  Click images to choose them, numbered with the `<imageN>` they will be;
  images already referenced are marked and can't be chosen; double-click
  for the full-size preview. The footer counts the references after adding
  (red past 9). **Add N reference(s)** or Enter adds them, Esc or Cancel
  closes, and it reopens on the last source), and image files
  dropped on the panel (imported as references, not as ideas); the img2img
  start image with strength (see Output → input).
- **Status bar jobs indicator** (bottom right): an overall progress bar and
  "N run(s) · done / total seeds" (or "no jobs · N server(s) ready"). Click
  it for a popover (a fixed 520 px): **Finish & stop all** / **Stop all
  now** (always there, above the runs, greyed out with none), the runs in
  the engine as in Projects → Jobs (below), the server states, and **Open
  Projects → Jobs** / **Change servers…**. Cancelled and failed seeds don't count
  towards the total.
- **Interface zoom** (status bar, left of the bell): "− 100% +" scales the
  whole interface from 70% to 300%. The buttons and Ctrl + / Ctrl − step to
  the next 5%, Ctrl 0 is 100%; the percentage can also be typed (it
  applies on Enter) or dragged. It's kept in the config (`ui_zoom`), not
  per library. egui's own keyboard zoom is off, so the config always has it.
  A divider separates it from the bell.
- **Messages** (status bar, after the library path): the latest one, with
  "(+N more)" when there are others and ✕ to dismiss it. Clicking it opens
  a popover with every message, newest first, each with ✕, and **Clear all**.
- **Discarded** (Projects' last tab, with its count): discarded projects,
  newest first, as tiles (cover, name, rating counts, "from projects/…",
  when). Hovered, a tile shows **Restore** and **Delete** on its cover;
  right-click: Restore, Open in file manager, Delete permanently…
  **Restore** puts the project back in the folder it was in (the top level
  if that folder is gone), under its name (numbered if taken); its pending
  seeds generate again. **Delete permanently** (asks first) sends its folder
  to the system trash; the images stay until Clean up. **Empty
  discarded…** (asks first) does it for all of them.
- **Jobs**: per-run progress and per-server state (loading / draining /
  generating), with a live preview frame from `jobs/{id}`. Under **Runs**,
  the order toggle and **Finish & stop all** / **Stop all now** stay put
  (always shown, greyed out with nothing generating); the runs scroll
  under them, by project: a project is a box in the order its first run is
  in the queue, folded until opened (▸, for the session), its header the
  name, "3 runs · 46 / 120 seeds · ~12 min left" (its runs' images left at
  their time per image, as many at once as there are servers), and
  **Finish & stop** / **Stop now** for all its runs (Stop now asks), then a
  bar over its seeds. Open, its runs in queue order: a run on its own as a
  block (label, prompt, a bar over its seeds that counts partly sampled
  seeds by their steps, one thin bar per seed being sampled, its timing,
  Finish & stop / Stop now); a sweep's runs together in one more folded box
  ("r05 · sweep", its seeds, time left and stops, "now: Ink s3 14/20, …"
  and a bar), open a line per run: its row (the style, "cfg 3"), a bar,
  what it's sampling and its stops. A click on a run's block or line opens
  its details. Preview is `"proj"`
  by default; `"tae"` once TAEQI2.1 is in the `tae` slot.
- **Job details**: clicking a run in Jobs or in the jobs popover opens a
  modal: its progress, the prompt and negative, the input images as
  thumbnails labelled by their tag (`<image1>`, `<image2>`…, **start image**;
  click one to see it large over the modal), and the settings (created, mode
  and strength, model, size, steps / cfg, sampler, flow shift, alpha, seeds
  per revision, sweep, source idea, cancelled seeds), with the errors of
  failed seeds. **Copy prompt**, **Open in Generate** (the run's settings in
  the form, as Use this style), **Cancel run** while it is in the engine,
  **Close** / Esc.
- **Review** (output queue, top-level tab): a left panel picks the scope
  (all projects / a folder / a project, with unreviewed counts), independent
  of the Projects tab's selection so browsing projects doesn't change the
  queue; "review this project" actions (double-click in Projects → Grid,
  opening an unreviewed image in a folder view) set it. Single view + grid view + A/B compare (`C` or
  the A/B button pins the current image as A and shows the next as B; both can
  be rated, and their sides swapped; **Compare with…** (`Shift+C`, the
  toolbar, the compare bar, right-click A → Replace A with…, or any
  image's right-click menu) makes A an image picked in the From library…
  picker instead, B staying the image it was opened on: one click picks,
  and its tree starts with **This image**, the image's references (its
  start image first) and its project's (every run's, newest run first),
  opening on the first that has any, else where it was last; an input
  image as A has no rating buttons. B, the image the arrows
  change ("B (← →)"), is on the left and the pinned A on the right;
  **Swap sides** changes that (again: back), each keeping its label;
  which image is pinned doesn't change. A made into B (its start image or a
  reference) is tagged so ("B's reference 2 · <image2>"), and under Its
  references the picker's thumbnails say "start" or "ref N". The Output library's
  right-click Compare with… turns its Grid into the same A | B view, B the
  selected image, which ←/→ (or the bar's arrows) move through the images
  shown, the lineage panel following B; Esc or **Leave compare** goes back
  to the grid, as does changing tab), over a
  checkerboard for alpha images. Single and Grid have an info panel on the
  right: "3 / 18", the image, and its recipe (project, run, **queued** (its
  revision's time), **generated** (when it was stored, from the run's
  timing; older runs without one give the image ID's minute, "(from its
  ID)"), seed, recipe, size, …), times as "2026-10-19 13:00 UTC · 8 days
  ago". Double-clicking Single's image opens it
  full size in a modal (click or Esc closes; Review's keys wait meanwhile). The
  modal (the same one every large preview uses) is a fixed box nearly the
  window's size, and the image zooms inside it: wheel or pinch at the
  pointer, `+`/`-`, `0` fit, `1` 100%, and a bar under the image (−, %, +,
  Fit, 100%, ✕); zoomed in, dragging pans and a click no longer closes it.
  Opened from Review (single or grid) or the Output library's grid, `←`/`→`
  (or ‹ › with "3 / 18" in the bar) step through that list, and the review
  cursor or Output selection follows, so closing leaves you on that image.
  Opened from the contact sheet, `↑`/`↓` go to the same seed one style up or
  down (skipping cells with no image yet), and the selected cell follows.
  There (contact sheet included) the image can also be rated and used without
  closing: `1`/`2`/`3` (or `G`/`N`/`B`; `1` then no longer zooms), `Ctrl+Z`,
  `E`/`D`/`R`/`I`, the bar's right end (↶ undo · Good · Neutral · Bad, the
  current verdict marked · ⋯ the use menu), or right-click (the verdicts over
  the use menu). In Review's queue the rated image leaves it and the preview
  goes on to the next one (closing after the last); on the contact sheet it
  steps on; in the Output library it stays. A use that opens another tab
  closes the preview. The bar's ⌨ button opens a popover listing the keys
  that apply there (navigate, rate, use, zoom, close). The keys are listed in a collapsible
  **Keys** section pinned to the bottom of the scope panel: `1`/`2`/`3` (or `G`/`N`/`B`) =
  good/neutral/bad, arrows = next/prev, `Home`/`End` = the oldest / newest
  (on the contact sheet the row's first / last seed, with `Ctrl` the
  sheet's; Grid, the strip and the sheet scroll to the image moved to),
  `Space` = toggle grid/single, `C` =
  compare with pinned, `E` = new edit run from this image, `R` = add to input
  library as ref, `I` = new idea from this image, `Ctrl+Z` = undo last review
  action.
- **Output library**: the place for finished work, as opposed to Projects
  (where images are made and triaged). Tabs **Grid** (default) · **Folder**
  · **Discarded**.
  The full-height left panel has, from the top: the **search** box with
  **in Images | Project names** under it (Images: an image's title or its
  run's prompt; Project names: narrows the "by project" smart folder to the
  projects and folders whose names have the text, a matching folder with all
  it holds, every folder shown open; switching takes the text along), the
  filters (has refs, has alpha, and one checkbox per rating: good, neutral,
  bad, unreviewed), then the **sources**, collections first and smart folders
  after; whichever is picked drives Grid and Folder. In Grid, a **Keys**
  list is pinned at the bottom of that panel; ← → ↑ ↓ and `Home`/`End`
  select the previous / next / first / last image (shown in the lineage
  panel, scrolled to). The sources:
  - **All projects** (the default), first in the collections list as a
    virtual collection of everything, then **All collections**.
  - **Collections**: your own folder tree of kept images, independent of
    projects (e.g. "Portfolio / animals", "Client picks / acme"). An image can
    be in several collections. Fill them by dragging images from Grid onto a
    collection, or with **Add to collection ▸** in the right-click menus of
    the Output library, Review and the folder views (which also offers **New
    collection**). On disk: `collections/<folder>/…/<image-id>.png`, links into
    `output_images/`; removing an image from a collection removes only that
    link, deleting a collection removes its links (asks first), the images
    keep their rating and project.
  - **Smart folders** (virtual, read-only, computed): by date (per day, from
    the image IDs), by source idea, by project, transparent. **By project**
    follows the project tree: folders and subfolders (each a source of its
    own, its projects' kept images together) down to the projects, showing
    only what has kept images; in the side panel a folder holding the
    selected source starts open, and each project expands to its runs with
    kept images, newest first (a sweep's runs by style: `r02 · Claymation`),
    each a source of its own (open while one of them is shown).
  - **Grid**: flat grid of the source's images, under a row of preset tabs
    that each count the source's images: **Kept** (good + neutral, the
    default) · **Good** · **Rejected** (bad) · **Unreviewed** · **All**.
    Clicking a tab replaces the filters (sets its ratings, clears text, refs
    and alpha); once the filters are changed the tab shows "•" and clicking
    it again resets them. **Rejected** follows the source, with **Delete these
    N rejected…** (asks first; they go to **Discarded**, their seeds not
    generated again) and right-click to re-rate, send back to Review or
    **Delete…**. **Unreviewed** has **Review them**
    (Review scoped to the source). Double-clicking an image opens it full
    size over the window (the image preview: click, Esc or a click outside
    closes it; right-click copies it). Selecting an image shows the
    lineage view (source idea → parent ← image → children) with its recipe
    (as in Review's info panel);
    double-click any of its images for the full-size preview. Right-click: change rating, back to Review, edit run, → input ref, → new
    idea, add to collection, **Use as project cover** (the Projects Grid then
    shows it instead of the best-rated image), **Export…**, show project, copy
    path. **Export these N…** exports everything shown.
  - **Folder**: the file-manager component over the collections tree, with a
    `smart/` folder at the top for the smart folders. Collections behave like
    folders (new, rename, move, copy, delete); images in them can be moved or
    copied between collections, removed (`Del`), opened (Grid + lineage),
    exported, or set as project cover. Its location and history are its own.
    Projects → Folder stays the place for project files (`runs/`,
    `unreviewed/`, triage); the "by project" smart folder is the project
    tree with only the kept images.
  - **Discarded** (with its count): the images deleted from projects (the
    Rejected tab's delete, Delete in a folder view), newest first, framed in
    their rating's colour. Each moves to its project's
    `discarded/<its rating folder>/`, kept out of the folder views, Review,
    the queue and the other tabs, and its run records the seed as deleted
    (not generated again, not a missing output). Hovered, an image shows
    **Restore** and **Delete**; right-click: Restore, Show in file manager,
    Delete permanently… **Restore** puts it back in its project, rated as it
    was, and its seed is no longer deleted (refused if a link of that name is
    there). **Delete permanently** (asks first) sends the link to the system
    trash; the image stays until Clean up. **Empty discarded…** (asks first)
    does it for all of them. Clean up and store conversion count discarded
    images' links, so a restore finds its image.
  - **Titles**: an output image can be given a title (**Set title…** in the
    Grid's right-click menu, `F2` on the selected image, or the pencil in
    the lineage view). A small dialog edits it: Enter saves, an empty title
    removes it. Titles are stored per project in `project.rsn` (`titles`,
    image ID → title; not copied when a project's setup is copied), show
    in the image's tooltip and lineage view, and are matched by the text
    filter (and the reference picker's).
  - **Show outputs (N kept)** jumps here from a project or project folder:
    the right-click menus of project tiles, the project tree's projects and
    folders, the button at the top left of a hovered tile, the Projects
    Grid's breadcrumb (for the folder in view), and the Projects Folder
    view's right-click menus: on a folder, project or run, or on empty
    space for the folder or project being viewed. It opens Grid on the Kept
    tab for that project or every project under the folder, and selects it
    in the "by project" smart folder: the tree starts over, every folder
    collapsed but the ones on the way to it (that header open too, even if
    you closed it), the row scrolls into view, and a
    project-name search that would hide it is cleared. The other
    filters are kept, but a banner says how many images they hide, with
    **Reset filters**; if the source has no kept images it says so, with
    **Show all N, bad included** (the All tab) and **Review them** when some
    are waiting in Review. The banner
    goes when another source is picked or it is dismissed. The lineage panel
    links back to the image's project. A plain click on an image opens the
    lineage panel, but while it's closed only a moment after the click, so
    a double-click (full size) doesn't open it; while it's open, a click
    shows the clicked image at once.
  - **Export…** (an image, a selection, a collection, a smart folder, or a
    folder in either folder view): copies the PNGs to a chosen folder, with
    or without the embedded metadata, named by run/revision/seed or by image
    ID. The library is unchanged. A modal: Enter exports, Escape or a click
    outside cancels.
- **Dialog buttons**, the same everywhere:
  - The footer row's ways to finish are right-aligned: the main action in
    the corner and bold, the other ways to finish to its left, then
    **Cancel** / **Close** furthest left (as GNOME and macOS do). Tools
    that don't finish the dialog (Add, Reset, Copy, Open folder, tick all,
    a wizard's Back…) and status text go on the left of the same row.
    When both don't fit, the tools go on a row of their own under the ways
    to finish, rather than being clipped (decided from the widths drawn
    the frame before). One helper lays every row out
    (`widgets::dialog_footer`).
  - Names say what happens: Save, Apply, Create, Export…, never "OK" or
    "Done". A confirmation's button is its verb (**Delete**, **Stop now**,
    **Switch library**, **Queue**, **Leave**…), not "Yes". **Cancel**
    when something would be lost, **Close** when nothing is pending.
  - Key hints in the label: `Esc` on Cancel / Close where Escape closes;
    `Enter` on the main action where Enter does it (every single-step
    dialog).
  - One way out of a modal: no ✕ beside its Cancel / Close (movable
    windows keep their title bar's ✕).
- **Icons**: Phosphor (regular) via the `egui-phosphor` crate, registered as
  a fallback font after the text font.

### Timing

Every image is timed as it is made, and the statistics come from those
records, so they cover runs from earlier sessions too (images made before
timing existed have none).

- **Recorded per image** (`run.rsn` `timings`, written with the image):
  `elapsed_s` generating on the server (sd-server's job `started` →
  `completed`), `queued_s` waiting in the server's queue first (`created` →
  `started`), `step_s` seconds per sampling step, timed by the engine from
  the progress reports (first to latest step of the first pass; 0 when the
  job sent no previews), the device, and when it was stored.
- **Per image**: in its tooltips (Review, Output library, contact sheet) and
  the Output library's lineage panel: "generated in 34.2 s · 1.71 s/it ·
  waited 1.0 s · ROCm0".
- **Per run** (job details window, **Timing**): per image (mean, fastest,
  slowest), per step (measured mean, else `≈` generating time ÷ steps, which
  also counts encoding and decoding), GPU time (summed), wall clock (first
  image started → last stored, so runs sharing the GPUs and pauses count),
  server queue wait, and per device. Below it, the project's totals.
- **Live** (each run in Jobs, and "now" in its details): seconds per step of
  the images sampling now (from the GUI's own clock as their steps arrive),
  seconds per image, how many are done and in what time, and roughly the
  time left: the images left (partly sampled ones by their steps) at the
  run's seconds per image, as many at once as are sampling now.

Durations read as "0.84 s", "34.2 s", "4 min 12 s", "2 h 05 min".

### Timeouts

Each is a stall limit, not a cap: how long something may go without a sign
of progress before it's given up on, so a slow download, answer or load that
keeps coming is never cut off. Seconds, at least 1, in `config.rsn`'s
`timeouts` (written only when not the defaults), set in Settings →
**Timeouts** and at the bottom of **Server settings** (the same section),
with **Reset to defaults**. Used by servers and downloads started from then
on.

- **Server start** (300 s): a server being started (sd-server, KoboldCpp,
  prompt enhance's KoboldCpp) gives up once its log has had no new line for
  this long (`process::LogStall`); its exit or **Cancel** end the wait
  sooner.
- **Image server requests** (60 s): sd-server and KoboldCpp requests, per
  read and write (ureq's `timeout_per_read`/`_per_write`), so a big result
  over a slow link isn't cut off. sd-server's `models/load` (silent until
  loaded) keeps a 1 h cap, and KoboldCpp's generate a 1 h cap (a cancel ends
  it sooner).
- **Prompt enhance answer** (300 s): no word yet (with images on a CPU the
  first can take minutes) or none since the last; the whole answer has no
  cap.
- **Model downloads** (60 s): no data. A broken or cancelled download keeps
  its `.part` and carries on from there next time (`Range`; from the start
  when the server takes no ranges).

The short ones stay fixed: connecting (5 s; 20 s for downloads), the 3 s
"is it up" checks, the 20 s download-size check.

### Notifications

**Settings** (⚙ at the far right of the top bar; a modal, changes apply as made) has three tabs:
**Library** (the open library's store format and its conversion, see Image
stores), **Timeouts** (see Timeouts) and **Notifications**, with three
independent alerts. Its footer shows the version line (`increart 0.1.0
(b6bd961, 2026-10-29)`: the crate version, then the commit built from and
its date, from `build.rs`; also `increart --version`) with a copy button,
for bug reports. The two conditions below each sound when its condition starts holding, then
again every *repeat* minutes while it still holds; once the condition ends it
is reset, and the next time it holds it counts again from the start.

- **GPU idle**: a server that is ready (not starting or loading a model) with
  no job for more than *X* minutes. It applies to **any GPU** or to **all
  GPUs** (every server idle that long). It counts only while servers are
  running; with none started there is nothing to wait for. The engine reports
  `idle` with each server state; the GUI notes when each one became idle.
  **Prompt enhance asks count as work** (on by default): while any ask is
  going no GPU counts as idle, and idle time counts again from the last
  one.
- **Review backlog**: more than *N* images waiting in Review (the count the
  Review tab shows, over every project).

The third is a moment, not a condition: **GPU queue cleared** sounds once, at
once, when a GPU that had work goes idle (ready, no job in flight and none
left for it in the pool), for **each GPU** or **only when all GPUs are done**
(no server still has work). A server that was idle from the start, or that
dropped out (connecting, unreachable), doesn't count; nor does one whose runs
were all stopped with **Stop now** (the user did it and saw it; the engine
says which runs each server has jobs of). **Finish & stop** still sounds when
the last images are done. It doesn't repeat, and
a snoozed or held-back one is not played later; the bell shows it until the
window gets the focus or a GPU gets work again.

Each alert has its own sound: a built-in one (chime, double beep, bell),
synthesized as a WAV in the temp folder at the chosen volume, or a file of
your own. The sound is played by a **player** command, so it works on any OS:

- **automatic** tries, in order, the players usual on the OS and uses the
  first one found: `paplay`, `pw-play`, `aplay` on Linux; `afplay` on macOS;
  PowerShell's `Media.SoundPlayer` on Windows.
- a ready-made command from that list, or **your own command**, split like a
  shell would (quotes, backslashes) but run without a shell. Placeholders:
  `{file}`, `{volume}` (0.00–1.00), `{volume_percent}`, `{volume_pa}`
  (0–65536, PulseAudio's scale). A built-in sound is already at the chosen
  volume, so its `{volume}` is 1.00.

**Test** plays a sound now and says which player played it or why none did.
Options: only when IncreArt isn't the active window (a sound held back for
this, or by a snooze, plays as soon as that ends, if the alert still holds);
also ask the window manager for attention (taskbar flash); also put the reason
in the status bar's messages. The attention request is withdrawn (on X11 the
urgency hint cleared) once the window has the focus or nothing holds any more,
so the taskbar entry doesn't stay lit.

The **bell** at the right of the status bar shows the state: plain, ringing
(filled solid amber while an alert holds and isn't snoozed, so it's seen at a
glance; plain again once nothing holds), snoozed, or a warning (filled red)
when a sound couldn't play. Its menu lists what holds and snoozes for 5 min,
30 min, 1 h, **Snooze for…** (a dialog: a time typed in as `45`, `45m`,
`1h30`, `1h 30m`, `1.5h` or `1:30`, checked as it's typed; it says how long
it reads, or why it can't; **Snooze** and Enter work only for 1 min to 24 h;
the text is remembered for the session), or until restart,
and links to the Notifications settings. An alert's message in the status
bar (when so set) has **Snooze…** (opens the bell's menu) and ⚙ (the
Notifications settings) before its text, and in the messages list.

The **bell** in the status bar shows the state: crossed out when every alert
is off (a click opens the settings), ringing while an alert holds, `zz` when
snoozed, a warning when the last sound failed. Its menu says what holds, and
snoozes for 5 min, 30 min, 1 h or until IncreArt restarts. Snoozing isn't saved;
the settings are, in `config.rsn` (`notifications`, left out while default).

The checks run once a second from the app's `logic` step, which eframe also
calls while the window is minimized or hidden, with a clock of the app's own
(egui's input time stops while the window is hidden).

## Testing

- gui-core reducer: headless unit tests (same pattern as Yue2).
- core client: tests against a mock HTTP server replaying recorded sd-server
  responses (job lifecycle, 503/409/422 paths, cancel-while-generating, job
  TTL expiry) and recorded PE chat responses.
- Scheduler: drain → load → resume, round order, back-off.
- Optional integration tests gated on `INCREART_SDSERVER_URL` and
  `INCREART_PE_URL`.
- A real KoboldCpp on each OS's CPU (`crates/increart-gui-core/tests/
  koboldcpp_cpu.rs`, `.github/workflows/koboldcpp-cpu.yml`: nightly, by hand,
  and before each release publishes): the app starts it, two 256 × 256
  one-step images come back through an admin reload, a generation is
  stopped partway (gone from the app within 5 s), the server stops (port
  free again), then a second KoboldCpp started on demand answers two
  prompt enhance asks (a rewrite and an image description). The models are
  the smallest set (Q2_K diffusion, IQ1_S encoder, 5.8 GiB), one Actions
  cache entry for all three OSes; the images and answers aren't looked at,
  only that they arrive, decode and leave the library intact. CI's runners
  have no GPU, so the GPU paths are tested by hand.
- `INCREART_LINK_MODE` (see Links): CI runs the tests on Windows with hard
  links and with copies too.
- Library format: `crates/increart-core/tests/common/sample.rs` writes a
  deterministic on-disk sample library (stores, links, IDs, pixel hashes,
  RSN/JSON chunks) file by file, and `verify.rs` beside it checks one
  independently (its own PNG decoder, the RSN read as text); together
  they are the fixture for `increart-core`'s loaders and writers.
  `increart-core`'s tests parse a freshly generated sample (every RSN file
  must also re-serialize byte for byte), then build a library through the
  API alone and run the verifier on it. Tests turn `fsync` off
  (`fsutil::set_fsync(false)`); on an overlay filesystem it costs about a
  second per write.
- gui-core flows (`crates/increart-gui-core/tests/flows.rs`): the reducer
  with the real runtime and a real temp library (create library, add /
  review / restore / make projects from ideas, queue a run, trees, drop files, review +
  undo) and the server dialog as a pure reducer.
- The app's screenshot tour (`INCREART_TOUR`, see [gui/README.md](gui/README.md))
  drives every screen through the reducer and saves one PNG per screen.
  `INCREART_TOUR_SCENES=01,02,03,99` runs a subset; scene `99` edits a
  project's prompt and closes the window on the next frame (inside the 2 s
  autosave delay), checking that the draft is flushed on quit: afterwards
  `projects/experiments/glass-fish/draft.rsn` must hold the edit.
  Scenes `R…` take the README's shots and `G…` its gallery
  ([readme/GALLERY.md](readme/GALLERY.md), retaken by
  `scripts/make-readme-gallery.sh` as WebP), both on the example library.
- GUI runs that need no human input (screenshots, synthetic input) run on a
  private Xvfb display via `scripts/headless.sh`, never on the desktop. POC1's
  screenshot walkthrough and documentation tour are described in
  [POC1.md](POC1.md#6-testing).

## Measurements so far

HIP build, one R9700, 1024², 20 steps, base int8 model, sd.cpp `fd21ecf`:

- First image after load: 132 s (text encode incl. lazy load 50 s, diffusion
  tensor load 45 s, sampling 77 s incl. a 47 s first step).
- Warm image, same prompt: 34 s (1.56 s/it, conditioning cache hit, decode
  2.5 s).

Through IncreArt's engine (2026-10-24, `examples/smoke.rs`: one server on
ROCm0, 1024², 20 steps, **CFG 6**, recipe flow schedule):

- sd-server up in 14 s; `models/load` returns in 2 s (weights load lazily);
  the 512² 1-step warm-up then took 145 s, absorbing the lazy load as
  intended.
- 65.5–66 s per image (63 s sampling, 2.5 s decode): CFG above 1 runs the
  model twice per step, so CFG 6 costs about double the CFG 1 timing above.
  The resolution editor's time estimate must scale with CFG.
- RGBA prompt (`cute robot mascot sticker`): 65% of pixels fully
  transparent. Plain prompts: RGBA output with alpha 253–255.
- With `preview: none` the job JSON carries no step progress at all, so the
  GUI only sees queued → generating → completed. Per-step progress needs a
  preview mode (`proj` is the cheapest); worth measuring its cost.

Three reference images at once (2026-10-24, `examples/refs.rs`, same
server; refs 1024² each, output 1344×896, CFG 6, prompt "Place the three
subjects side by side in one row, left to right: the red fox from image 1,
… image 2, … image 3 … Plain pastel yellow seamless studio background"):

- Layout, order and background followed in both seeds with a plain
  template prompt; subjects recognisably taken from their refs.
- 237–289 s per image vs 66 s without refs: each 1024² ref adds ~4096
  tokens to every attention call. Downscaling refs (e.g. to 512²) is the
  obvious lever; to be measured against fidelity.
- Quality problems: both images washed out / over-bright with a glow; in
  seed 0 the fox's fur turned flat green with purple blotches. Purple is the
  hidden colour under the transparent area of the RGBA robot ref, so alpha
  refs may leak; CFG 6 may also be too high with refs.
- Same refs and seeds at **CFG 2** with a prompt that first says what each
  ref is ("Image 1 shows a red fox, image 2 …, image 3 …. Create a new
  photograph of these three together in a row on a pastel yellow seamless
  studio backdrop: … Keep each subject's look from its reference image.
  Natural colours …"): clean, natural colour, no blotches, all three
  subjects faithful to their refs. CFG 6 was the main cause of the washed-out
  look (prompt changed at the same time, so not fully separated).
- The same prompt written with `<image1>`/`<image2>`/`<image3>` tags (CFG 2,
  same seeds): near-identical images, same layout and poses per seed;
  seed 1's robot slightly crisper. With distinct subjects and an explicit
  order, the tags matter little; they remain the documented convention.
- Time per image unchanged by CFG (236–257 s): the cost is the three 1024²
  refs. Default CFG for runs with refs: 2 (base model), pending a wider
  A/B.

Server configurations (2026-10-28, [benchmark-servers.html](benchmark-servers.html),
`scripts/bench-servers.sh`): one 1 MP edit with one 1024² ref, CFG 2,
20 steps, and PE with Qwen3-VL 8B, on each single-GPU setup:

- sd-server on ROCm with flash attention is fastest: 102 s per image with
  the conditioning cached (126 s for a new prompt), 25.6 GiB VRAM.
- `--offload-to-cpu` on ROCm costs 4% (105 s) and cuts VRAM to 4.5 GiB
  (17.3 GiB RAM instead). On Vulkan it saves little (18 GiB VRAM).
- Without flash attention ROCm is 3.5× slower; sd-server on Vulkan is 4.7×
  slower, and ~50 min per image without flash attention.
- KoboldCpp 1.123.1 works with GGUF files set up as its official
  Qwen-Image 2.1 `.kcppt` (Q4_K diffusion, Qwen3-VL Q4_K_M + mmproj as
  `--sdclip1`, `--sdclipdevice main`): ~201 s per image, 14.5 GiB VRAM
  with offload, 22 GiB with Q8_0 and no offload. It encodes the prompt again
  for every image (no conditioning cache). Its default runs the text encoder
  on the CPU (minutes per image), and the int8 files expand to 29 GiB and
  fail.
- PE on the GPU: 2–3 s per answer, ~58 tokens/s, 7.2 GiB VRAM. On the CPU:
  ~9 tokens/s, 6–80 s per answer.
- PE and image editing on one card overflow its 32 GiB: images +41% and PE
  down to ~3 tokens/s while both work. Keep PE on the other card.

KoboldCpp presets by VRAM (2026-10-29, [vram-tiers.md](vram-tiers.md)):
image editing and PE in one KoboldCpp on one card of 8–32 GB. The image
VRAM cap (`--sdvramlimit`) is the lever: speed holds down to a 7 GiB cap,
then drops ~75%. 24 and 32 GB keep PE resident (207–217 s per image,
18.1 and 22.7 GiB peak). 12 and 16 GB swap PE and the image model with
autoswap (~208 s per image, plus ~5–9 s per swap; the app swaps them
itself, see **Swapping** above). 8 GB swaps too, with
the text encoder on the CPU (364 s, 6.3 GiB).

Why ROCm beats Vulkan on the R9700 (2026-10-29,
[backend-performance.md](backend-performance.md)): not int8 convrot, which
KoboldCpp lacks on every backend (a ROCm build included). RADV on Mesa
25.0.7 exposes no cooperative matrix for RDNA4, so Vulkan matmuls skip the
matrix cores that hipBLAS uses. The same sd-server and int8 model takes
103 s per image on ROCm and 480 s on Vulkan.

## Open questions

- EXIF from imported photos is kept verbatim, including GPS location if the
  camera recorded it. Strip GPS (or all EXIF) on import, keep as is, or make
  it a setting?
- `inbox_drop/imported/` keeps every imported original forever unless you
  clear it. Auto-delete after some days, or leave it to the user?

- A Settings screen: the resolution editor is only reachable from the
  Generate panel for now. A general Settings place (servers, PE, library
  root, resolutions) would be the natural second entry point once there is
  more to put in it.

- New idea form: which add button is the primary one (Add to inbox, Add &
  keep, Add & new project)? POC1 shows all three with equal weight.
- IDs use a separate counter per kind, so the same string can be an idea, a
  run and an image. Keep that, or use one counter across kinds so every ID is
  unique in the library?

- Ideas from outside: is a watched drop folder enough, or is a small HTTP
  endpoint wanted (e.g. a phone share target or browser bookmarklet)?
- Captioning image-only ideas: the i2i PE model or a Qwen3-VL chat server could
  describe an image to seed its text. Is that worth adding to the PE service?
- Base-model CFG: 1.0 (recipe default) vs ~6.0 (sd.cpp example). A/B on a fixed
  prompt set before choosing the preset.
- GGUF availability (and llama.cpp support) for the Qwen3.5-9B PE models.
- Upstream appetite for a disk-backed conditioning cache vs. carrying a local
  patch.

## To benchmark

- PE beside an offloading image server on one card (should fit; see the
  server benchmark).
- Splitting one model across both GPUs (`diffusion=ROCm0&ROCm1`) vs one server
  per GPU: per-GPU is expected to win on throughput for batches.
- 2048² time per image, to decide whether it stays a preset or becomes a
  "final render" option.
- Conditioning cache size vs. hit rate with interleaved runs.
- TAEQI2.1 preview cost vs `proj`.

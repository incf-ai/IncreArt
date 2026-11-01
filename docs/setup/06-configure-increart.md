# 6. First run and `config.rsn`

Sources: `crates/increart-gui/src/startup.rs`, `crates/increart-gui/src/main.rs`,
`crates/increart-core/src/config.rs`, `kobold.rs`, `pe.rs`, `tiers.rs`,
[../DESIGN.md](../DESIGN.md) (Startup, Config file). Screenshots:
[../gui/README.md](../gui/README.md).

Order at start-up: **library screen → servers screen → main window** (on its Home tab).

## 6.1 Create a library

**Goal:** a library folder (holds ideas, projects, images; has `library.rsn`).

**Preconditions:** guide 1 done.

1. `./target/release/increart`
2. "Open a library" screen ("No libraries yet…" the first time):
   **Create a new library…** ([../gui/53-library-wizard.webp](../gui/53-library-wizard.webp)).
3. Pick **In your home folder** (`~/IncreArt`, preselected), **In the current
   folder**, or **Somewhere else:** (type or **Browse…**).
4. **Create library**. The folder is created if missing.

**Expected result:** the servers screen opens.

**Verify:**

```sh
test -f ~/IncreArt/library.rsn && echo library ok
```

**If it fails:**
- "This folder isn't empty: choose another one." → pick an empty or new
  folder.
- "There is a library here already." → **Open it instead**.
- Existing library elsewhere: **Open an existing library…**, pick the folder
  that contains `library.rsn`.

## 6.2 Servers

**Goal:** a server preset saved and opened.

**Preconditions:** 6.1; guide 3 (KoboldCpp) or 4 (sd-server) prepared.

With no presets, the servers screen says "No presets yet." Use **Set up
servers…**:

- KoboldCpp: guide [3.5](03-koboldcpp.md#35-run-the-wizard-downloads-models-writes-the-preset).
- sd-server: guide [4.4](04-sd-server.md#44-let-increart-start-it).
- By hand: **New preset**, then **+ KoboldCpp**, **+ sd-server**, **+ Connect
  to sd-server**, **+ Chat server**; **Save and open**.
- Shared settings (KoboldCpp binary, sd-server binary and devices, models
  folder, leave servers running): **Server settings…** (bottom right).
- **Continue without servers**: browse and review work; runs queue and wait.

**Expected result:** the main window, on Home; the top bar shows each server
starting, then up. With **Continue in background** the window opens while
they start.

**Verify:** top bar **Servers…** → each server's state and model. From a shell
(ports from your preset):

```sh
curl -s http://127.0.0.1:5001/api/extra/version; echo        # KoboldCpp
curl -s http://127.0.0.1:1234/sdcpp/v1/capabilities | head -c 200; echo   # sd-server
```

**If it fails:** the dialog stays open on failure with the error and the tail
of the server log; see [07-troubleshooting.md](07-troubleshooting.md).

## 6.3 Skip the start-up screens next time

```sh
./target/release/increart --library last --servers last
```

`--servers "<preset name>"` opens a named preset; `--no-servers` opens
without. A screen still shows, saying why, when its argument doesn't work.

## 6.4 Model presets (Generate → Model preset)

`model_presets` (`ModelPreset { … }` entries) in `config.rsn`: which files to load and the default steps / CFG.

- Built in (added when `model_presets` is empty): **"Qwen-Image 2.1"**, recipe
  `qwen-image-21`, sd-server int8 files (guide 2.2), 20 steps, CFG 2.
- The KoboldCpp wizard adds **"Qwen-Image 2.1 GGUF Q8_0 (CFG 2, refs)"** (or
  `Q4_K`) with `diffusion_model`, `llm`, `clip_l`, `vae` under `koboldcpp/`.
- A preset is offered when an open server has all its files. Otherwise it's
  listed under "Not on these servers: …", with each server's reason on hover.
- File IDs: for sd-server, the `id`s from `GET /sdcpp/v1/model-files`; for a
  started KoboldCpp, paths relative to a models folder (or absolute).
  Slots: `diffusion_model`, `model`, `llm`, `vae`, `clip_l`, `clip_g`,
  `t5xxl`. KoboldCpp takes `diffusion_model`→`--sdmodel`, `llm`→`--sdllm`,
  `vae`→`--sdvae`, `clip_l`→`--sdclip1`, `clip_g`/`t5xxl`→`--sdclip2`.

**Verify:** Projects → Generate → **Model preset** lists your preset without a
"Not on these servers" note.

## 6.5 `config.rsn` fields that matter

Location and `INCREART_CONFIG`: guide [1.5](01-build-increart.md#15-where-settings-live-increart_config).
Format: [RSN](https://github.com/khonsulabs/rsn), Rust-like literals. Unknown
fields are ignored; missing ones take defaults. `Option` values are written
bare (`last_preset: "x"`); `Some(…)` is accepted too.

| Field | Type / values | Default | Meaning |
|---|---|---|---|
| `recent_libraries` | list of paths | `[]` | libraries offered at start-up, newest first (max 10) |
| `server_presets` | list of `ServerPreset { name, servers }` | `[]` | the servers screen's presets |
| `last_preset` | preset name | none | used by `--servers last` and the main button |
| `programs` | `Programs { sd_server, koboldcpp }` | see below | the programs every server entry shares |
| `programs.sd_server` | `SdServerSettings { … }` | see below | shared by every sd-server **and** the models folders KoboldCpp loads from |
| `programs.sd_server.binary` | path or name on `PATH` | `"sd-server"` | |
| `programs.sd_server.model_dirs` | list of paths | `[]` | `--model-dir` roots; also KoboldCpp's model folders |
| `programs.sd_server.args` | list | `["--fa", "--conditioning-cache-size", "32"]` | for every sd-server |
| `programs.sd_server.preview` | `none` / `proj` / `tae` / `vae` | `"none"` | preview sent with each sd-server job |
| `programs.sd_server.commit` | string | `""` | sd.cpp commit, recorded in recipes |
| `programs.sd_server.leave_running` | bool | `false` | started servers outlive the app |
| `programs.koboldcpp` | path or command line | found at start-up | the KoboldCpp every `KoboldCpp` entry runs unless it names its own `binary` |
| `model_presets` | list of `ModelPreset` | built-in one | model presets (6.4) |
| `resolutions` | list of `Resolution { name, width, height, default }` | 6 built-ins, 1024² default | multiples of 32; others are snapped with a warning |
| `pe` | `Pe { system_prompts, sampling }` | built-in | PE prompts per job (`fill_blanks`, `style_prompt`, `rewrite`, `describe`) |
| `job_order` | `Spread` / `BySweep` / `Sequential` | `Spread` | how queued runs share servers |
| `file_manager`, `editor` | command | empty = desktop default | open folders / logs |

Server entries (`servers: [ … ]`) are named for the program they are. Each has
a `run`: `Launch { … }` (the app starts it) or `Connect { url, … }` (it is
already running).

| Entry | Fields |
|---|---|
| `SdServer { name, run }` | sd-server, makes images. `run: Launch { host, port, gpu, args }` (`host` default `127.0.0.1`) or `Connect { url, gpu }` (`gpu` optional, for recipes) |
| `KoboldCpp { name, run, images, chat, swap, when }` | KoboldCpp. `run: Launch { binary, config, host, port, gpu, args }` (`binary` default `programs.koboldcpp`; `config` a `.kcpps` to start from) or `Connect { url, password, gpu }`. `images` (default `true`) makes images; `chat: Chat { model, mmproj, context }` makes it a PE server too. With `images: false` it is a PE server only: it needs `chat` and `run: Launch`, and `when: OnDemand` (default, at the first ask) or `AtOpen` (when the preset opens) says when it starts. `swap: true` for 8–16 GB presets |
| `OpenAiChat { name, url, when }` | a running OpenAI-compatible chat server, for PE |

`gpu`: `Rocm(0)`, `Vulkan(1)`, `Cuda(0)`, `Metal([])`; a KoboldCpp the app
launches may take several (`Vulkan([1, 2])`; `Vulkan([])` = every GPU), and
`Cpu(16)` runs it on the CPU with 16 threads (`Cpu(0)`: its own count). An
sd-server, and any entry with `run: Connect`, names one device: `Cpu(0)`, not
a thread count or a list. Omitted: `Auto`, the program's choice. A name
sd-server's `--list-devices` gives that isn't one of these is written
`Backend("…")`.

Configs written before these names (`Start`, `Connect`, `Kobold(KoboldServer
…)`, `Chat(PeServer …)`, `recipes`, `sdserver`, `koboldcpp` at the top) still
load, and the app writes them the new way at its next save.

## 6.6 Annotated minimal examples

Replace every `<…>` with an absolute path (the code does no `~` expansion that
we could find). Comments are for reading here: the app drops them when it
saves.

**Before editing by hand, back the file up.** If it doesn't parse, the app
starts with defaults ("settings not loaded (…); using defaults"), keeps a
copy of your file as `config.rsn.unreadable` (`.unreadable-2` and on if
taken), and its next save writes the defaults over `config.rsn`.

```sh
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/increart/config.rsn"
mkdir -p "$(dirname "$CFG")"; [ -f "$CFG" ] && cp "$CFG" "$CFG.bak"
```

### A. KoboldCpp, one 24 GB GPU (Vulkan device 0), PE alongside

What the wizard writes for this case (args from the 24 GB tier):

```rsn
Config {
    recent_libraries: ["<home>/IncreArt"],
    model_presets: [
        ModelPreset {
            name: "Qwen-Image 2.1 GGUF Q8_0 (CFG 2, refs)",
            recipe: "qwen-image-21",
            files: {
                "diffusion_model": "koboldcpp/qwen_image_2.1-Q8_0.gguf",
                "llm": "koboldcpp/Qwen3VL-8B-Instruct-Q4_K_M.gguf",
                "clip_l": "koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf",
                "vae": "koboldcpp/qwen_image_2.1_vae_bf16.safetensors",
            },
            steps: 20,
            cfg: 2,
        },
    ],
    server_presets: [
        ServerPreset { name: "KoboldCpp Vulkan0", servers: [
            KoboldCpp {
                name: "kcpp-gpu0",
                run: Launch {                        // binary: programs.koboldcpp
                    port: 5001,
                    gpu: Vulkan(0),                  // vulkaninfo's GPU0; NVIDIA: Cuda(0)
                    args: ["--sdflashattention", "--sdoffloadcpu", "--sdtiledvae", "512",
                           "--sdvaedevice", "main", "--sdclipdevice", "main", "--sdvramlimit", "10240"],
                },
                chat: Chat {                         // also the PE chat server
                    model: "<home>/models/koboldcpp/Qwen3VL-8B-Instruct-Q4_K_M.gguf",
                    mmproj: "<home>/models/koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf",
                    context: 8192,
                },
                // swap: true,                       // only for the 8/12/16 GB presets
            },
        ] },
    ],
    last_preset: "KoboldCpp Vulkan0",
    programs: Programs {
        sd_server: SdServerSettings {
            model_dirs: ["<home>/models"],        // KoboldCpp finds koboldcpp/… here
        },
        koboldcpp: "<home>/koboldcpp/koboldcpp-linux-x64-nocuda",
    },
}
```

### B. sd-server (HIP) on two GPUs, PE on the CPU

```rsn
Config {
    recent_libraries: ["<home>/IncreArt"],
    // model_presets omitted: the built-in "Qwen-Image 2.1" (int8 convrot files) is used
    server_presets: [
        ServerPreset { name: "ROCm0 + ROCm1", servers: [
            SdServer { name: "gpu0", run: Launch { port: 1234, gpu: Rocm(0) } },
            SdServer { name: "gpu1", run: Launch { port: 1235, gpu: Rocm(1) } },   // args: ["--offload-to-cpu"] if VRAM is short
            KoboldCpp {
                name: "Qwen3-VL 8B (CPU)",
                run: Launch { port: 1240, gpu: Cpu(16) },
                images: false,                       // chat only
                chat: Chat {
                    model: "<home>/models/koboldcpp/Qwen3VL-8B-Instruct-Q4_K_M.gguf",
                    mmproj: "<home>/models/koboldcpp/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf",
                    context: 8192,
                },
                when: AtOpen,                        // load at preset open; default OnDemand
            },
        ] },
    ],
    last_preset: "ROCm0 + ROCm1",
    programs: Programs {
        sd_server: SdServerSettings {
            binary: "<home>/sdcpp/build-hip/bin/sd-server",
            model_dirs: ["<home>/models"],        // first root: holds diffusion_models/, text_encoders/, vae/
            args: ["--fa", "--conditioning-cache-size", "32"],
            preview: "proj",
            commit: "fd21ecf",
        },
        koboldcpp: "<home>/koboldcpp/koboldcpp-linux-x64-nocuda",
    },
}
```

**Verify** a hand-written config:

```sh
INCREART_CONFIG="$CFG" ./target/release/increart --library last --servers last
```

**Expected result:** the main window opens and the preset's servers start.
If the preset name is wrong, or sd-server / the models folder is missing, the
servers screen stays up with the reason. If the file doesn't parse, the status
bar says `settings not loaded (…); using defaults`: close the app, restore
`$CFG.bak` (or the kept `config.rsn.unreadable`) and fix
the syntax (unbalanced braces, missing commas between fields, an enum variant
spelled differently from the tables above).

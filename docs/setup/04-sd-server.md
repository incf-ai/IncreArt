# 4. sd-server (stable-diffusion.cpp) — alternative image backend

Sources: `scripts/build-sdcpp.sh`, `scripts/run-sdserver.sh`,
`crates/increart-core/src/launcher.rs`, `sdserver.rs`,
`crates/increart-gui-core/src/state.rs` (`sdserver_problem`),
`.devcontainer/Dockerfile`, [../DESIGN.md](../DESIGN.md) (Backend, Startup:
servers), [../backend-performance.md](../backend-performance.md), upstream
`docs/build.md` and `examples/server/` at the pinned commit.

Use this path for AMD GPUs with ROCm: measured ~103 s per image against
~204 s for KoboldCpp on Vulkan on the same card. You build sd-server yourself
and provide the int8 convrot models (guide 2.2). The project's own builds are
HIP (and Vulkan for benchmarks); **CUDA and Metal builds are untested** here.

Pinned commit: `fd21ecfceef2665d2c5f2db8536e3202d864fd30` (recorded as
`commit: "fd21ecf"`). The app uses only sd-server's native async API
(`/sdcpp/v1/...`); other commits may change it.

## 4.1 Build prerequisites

**Goal:** compiler, CMake, Ninja, ccache, and the GPU SDK.

**Preconditions:** Debian/Ubuntu.

```sh
sudo apt-get install -y build-essential cmake ninja-build ccache git libgomp1
```

Then one of:

- **HIP (AMD, ROCm):** ROCm userspace with `rocm-dev rocm-libs` from
  `repo.radeon.com` (the devcontainer uses ROCm 7.2.4; RDNA4 / gfx1201 needs
  ROCm ≥ 7.2). The kernel `amdgpu` driver must be on the host; your user in
  groups `video` and `render`. Follow AMD's install instructions for your
  distribution; then:
  ```sh
  export PATH=/opt/rocm/bin:$PATH
  export LD_LIBRARY_PATH=/opt/rocm/lib:/opt/rocm/llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
  ```
- **Vulkan:** `sudo apt-get install -y libvulkan-dev glslc spirv-headers mesa-vulkan-drivers vulkan-tools`
- **CUDA (NVIDIA):** the CUDA toolkit (`nvcc`). Untested here.

**Verify:**

```sh
cmake --version && ninja --version && ccache --version | head -1
/opt/rocm/llvm/bin/clang++ --version | head -1      # HIP
rocminfo | grep -o 'gfx[0-9a-f]*' | sort -u          # HIP: your GPU target, e.g. gfx1201
vulkaninfo --summary | grep deviceName               # Vulkan
nvcc --version | tail -1                             # CUDA
```

**If it fails:** `rocminfo` permission error → `sudo usermod -aG video,render
$USER`, log out and in.

## 4.2 Build

**Goal:** `sd-server` binary.

**Preconditions:** 4.1; ~a few GB disk for source and build.

HIP or Vulkan, with the repo's script (it clones, checks out the pinned
commit, initialises submodules, builds `sd-server` and `sd-cli`; skips the web
UI when `pnpm` is missing). Override its default location with `SDCPP_ROOT`:

```sh
export SDCPP_ROOT="$HOME/sdcpp"
GPU_TARGETS=gfx1201 scripts/build-sdcpp.sh hip      # your target from rocminfo
# or
scripts/build-sdcpp.sh vulkan
# both: scripts/build-sdcpp.sh hip vulkan
```

`-j` comes from `CARGO_BUILD_JOBS` (default 16).

CUDA (not in the script; upstream's `-DSD_CUDA=ON`; **untested**):

```sh
export SDCPP_ROOT="$HOME/sdcpp"
git clone https://github.com/leejet/stable-diffusion.cpp "$SDCPP_ROOT/src"
git -C "$SDCPP_ROOT/src" checkout fd21ecfceef2665d2c5f2db8536e3202d864fd30
git -C "$SDCPP_ROOT/src" submodule update --init --recursive
cmake -S "$SDCPP_ROOT/src" -B "$SDCPP_ROOT/build-cuda" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DSD_CUDA=ON -DSD_SERVER_BUILD_FRONTEND=OFF
cmake --build "$SDCPP_ROOT/build-cuda" --target sd-server -j
```

Don't set `SD_USE_UPSTREAM_GGML=ON`: that build rejects int8 convrot files.

**Expected result:** `built $SDCPP_ROOT/build-<backend>/bin/sd-server`.

**Verify:**

```sh
"$SDCPP_ROOT/build-hip/bin/sd-server" --list-devices
```

One `name<TAB>description` line per device, e.g. `ROCm0  AMD Radeon …`. The
first word (`ROCm0`, `ROCm1`, `Vulkan1`, `CUDA0`, …) is the device name you
give IncreArt. Under Vulkan an integrated GPU may come first (`Vulkan0`), so
a discrete card can be `Vulkan1`.

**If it fails:**
- HIP: `clang: not found` → ROCm not at `/opt/rocm`; the script hardcodes
  `/opt/rocm/llvm/bin/clang`.
- `unknown backend` → the script only accepts `hip` and `vulkan`.
- At run time `libomp.so` / `libamdhip64.so` not found → the
  `LD_LIBRARY_PATH` export from 4.1.

## 4.3 Smoke test by hand (optional; loads the model onto a GPU)

**Goal:** prove the server and models work outside the app.

**Preconditions:** 4.2; models from guide 2.2 in `$MODELS`; the GPU idle.

```sh
SDCPP_ROOT="$HOME/sdcpp" MODEL_DIR="$HOME/models" scripts/run-sdserver.sh 0 1234 &
# (BACKEND=vulkan scripts/run-sdserver.sh 0 1234 for Vulkan; it maps GPU 0 to Vulkan1)
sleep 10
curl -s http://127.0.0.1:1234/sdcpp/v1/capabilities | head -c 300; echo
curl -s http://127.0.0.1:1234/sdcpp/v1/model-files | grep -o '"id":"[^"]*qwen[^"]*"'
curl -s http://127.0.0.1:1234/sdcpp/v1/models/load -H 'content-type: application/json' -d '{
  "recipe": "qwen-image-21",
  "models": {
    "diffusion_model": "diffusion_models/qwen_image_2.1_int8_convrot.safetensors",
    "llm": "text_encoders/qwen3vl_8b_int8_convrot.safetensors",
    "vae": "vae/qwen_image_2.1_vae_bf16.safetensors" } }'
curl -s http://127.0.0.1:1234/sdcpp/v1/models/status; echo
kill %1
```

**Expected result:** `capabilities` returns JSON; `model-files` lists the
three IDs; `models/load` blocks until loaded; `models/status` has `"state":
"ready"`.

**If it fails:**
- IDs differ from the ones above → use the IDs `model-files` returns in the
  app's model preset (guide 6.4).
- `models/load` → 422 `model_load_failed` → read the server's output; usually
  out of memory: add `--offload-to-cpu` to the server's `args`.

## 4.4 Let IncreArt start it

**Goal:** a server preset with one `Start` entry per GPU.

**Preconditions:** 4.2; guide 2.2; a library open.

In the app: servers screen → **Set up servers…** → **sd-server, with your own
models**:

1. **1. sd-server**: the binary (`$SDCPP_ROOT/build-hip/bin/sd-server`) and
   the models folder.
2. **2. GPUs to start it on**: the devices from `--list-devices`, every GPU
   ticked, ports 1234, 1235, …
3. Optional **Add a chat server…** (guide 5).
4. **Save as preset and open**. The preset is named after its devices
   ("ROCm0 + ROCm1").

The app runs, per entry (`launcher::command_args`):

```text
sd-server --listen-ip 127.0.0.1 --listen-port 1234 --backend ROCm0 \
  --model-dir <models> [--model-dir …] --fa --conditioning-cache-size 32 [entry args]
```

- `programs.sd_server.args` defaults to `--fa --conditioning-cache-size 32`.
- Waits for `GET /sdcpp/v1/capabilities` to answer, giving up once the log
  has had no new line for the server start timeout (300 s by default;
  Settings → Timeouts, or Server settings).
- Logs to `<state>/logs/sd-server-<name>.log`.
- An sd-server already answering on the port is used instead of starting
  another.
- Stops with the app (Linux: parent-death signal; Windows: job object;
  macOS: a watcher), unless **Leave started servers running** is on.

**Expected result:** the top bar shows each server up; Generate's **Model
preset** offers "Qwen-Image 2.1".

**Verify:**

```sh
curl -s http://127.0.0.1:1234/sdcpp/v1/capabilities >/dev/null && echo up
curl -s http://127.0.0.1:1234/sdcpp/v1/models/status; echo
tail -n 20 ~/.local/state/increart/logs/sd-server-*.log
```

**If it fails:** the servers dialog shows `No sd-server binary set`,
`sd-server not found at …` or `Models folder not found: …` with **Fix…** →
correct it in **Server settings…**. Other errors:
[07-troubleshooting.md](07-troubleshooting.md#sd-server).

## 4.5 Connecting to a running sd-server

An `SdServer { name, run: Connect { url, gpu } }` entry (wizard: **Or connect
to a server that's already running**) uses a server you started yourself, e.g.
on another machine. It's left running when the app exits. `gpu` (`Rocm(0)`, …)
is optional and only recorded in recipes.

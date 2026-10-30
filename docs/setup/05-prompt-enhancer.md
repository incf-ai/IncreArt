# 5. Prompt enhancer (PE) chat server

Sources: `crates/increart-core/src/pe.rs`, `config.rs` (`chat_pool`),
`crates/increart-gui-core/src/runtime.rs`, [../DESIGN.md](../DESIGN.md)
(Prompt rewriting → PE server and requests),
[../todo-kcpp-support.md](../todo-kcpp-support.md).

PE suggests values for a sweep's blanks, rewrites prompts and ideas, and
describes images. It is optional: generation works without it.

Model: **Qwen3-VL-8B-Instruct** GGUF (`Qwen3VL-8B-Instruct-Q4_K_M.gguf`,
4.68 GiB) with its vision projector (`mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf`
0.70 GiB, or `-F16` 1.08 GiB). Qwen's own Qwen-Image 2.1 PE models exist only
as int8 / w4a8 safetensors, which KoboldCpp can't run.

## 5.1 Choose how PE runs

| Option | When | Setup |
|---|---|---|
| A. Inside the KoboldCpp image servers | You used the KoboldCpp wizard | nothing: done by guide 3.5 |
| B. A KoboldCpp the app starts for chat only | sd-server path, or a PE GPU of its own, or CPU | 5.2 |
| C. Connect to a running OpenAI-compatible server | a server you run yourself or on another machine | 5.3 |

Memory: on a GPU ~7.2 GB (model 4.5, 8192-token cache 1.2, projector 1.1,
work 0.5); measured ~0.5–1 s per ask on one R9700. On CPU (16 threads): ~60 s
to load, ~18 s per ask with one image, a few seconds without.

## 5.2 Option B: a chat KoboldCpp started by the app

**Goal:** a `Chat(PeServer { run: KoboldCpp { … } })` entry in a server preset.

**Preconditions:** KoboldCpp binary (guide 3.1–3.2); model + mmproj (guide
2.1, the `Qwen3VL…` and `mmproj…` files); a free port.

**Manual test first** (same flags the app uses, `pe::command_args`; Vulkan
GPU 0 shown; for CPU replace the device flags with `--usecpu --threads 16`):

```sh
K="$HOME/koboldcpp/koboldcpp-linux-x64-nocuda"
M="$HOME/models/koboldcpp"
"$K" --model "$M/Qwen3VL-8B-Instruct-Q4_K_M.gguf" \
  --mmproj "$M/mmproj-Qwen3VL-8B-Instruct-Q8_0.gguf" \
  --host 127.0.0.1 --port 5001 --skiplauncher --quiet --jinja \
  --usevulkan 0 --gpulayers 999 --contextsize 8192 & KPID=$!
until curl -sf http://127.0.0.1:5001/v1/models >/dev/null; do sleep 2; done
curl -s http://127.0.0.1:5001/v1/chat/completions -H 'content-type: application/json' -d '{
  "messages":[{"role":"user","content":"Say OK."}],"max_tokens":10}'
echo; kill $KPID
```

**Expected result:** `/v1/models` answers 2xx (this is the app's readiness
check); the completion returns JSON with `choices[0].message.content`.

Then in the app: preset editor (✎ or **New preset**) → **+ Chat server**, or
the sd-server wizard's **Add a chat server…**. Fields: name, **KoboldCpp**,
binary, model, vision file (mmproj), address `127.0.0.1`, port, **Runs on**
(GPU with a runtime and GPU numbers / CPU threads / KoboldCpp's choice),
context `8192`, **Starts** when first asked (`OnDemand`) or when the preset
opens (`AtLaunch`). **Add**, then **Save** the preset.

GPU numbers: Vulkan as `vulkaninfo --summary` lists them, CUDA as
`nvidia-smi`, ROCm as `rocm-smi` (HIP order). CUDA/ROCm need the CUDA build
or a ROCm build; the `nocuda` build is Vulkan only.

**Verify** (after the first ask, or at preset open with `AtLaunch`):

```sh
curl -s http://127.0.0.1:5001/v1/models; echo
tail -n 20 ~/.local/state/increart/logs/pe-*.log
```

In the app: **Servers…** → Prompt enhance row "ready"; Generate → **Rewrite**
returns text.

**If it fails:**
- `<name>: KoboldCpp exited (…) while starting; log …` → read that log.
- `<name>: KoboldCpp didn't answer, its log quiet for 300 s (the server
  start timeout); log …` → the model load stopped printing (stuck, or a
  very slow disk); read the log, and raise the server start timeout
  (Settings → Timeouts) if it was only slow.
- The chat server and an sd-server on one GPU: PE asks slow images down and
  need ~7.2 GB beside it; on small cards use CPU.

## 5.3 Option C: connect to a running server

**Goal:** `Chat(PeServer { name, run: Connect { url } })`.

**Preconditions:** a server exposing `GET /v1/models` (2xx) and streaming
`POST /v1/chat/completions` with `image_url` data-URL parts (OpenAI format).
KoboldCpp is the tested one; others (llama.cpp's `llama-server`, …) are
**untested**. Stopping an ask calls KoboldCpp's `/api/extra/abort`; other
servers just see the connection close. A Connect server is assumed to have
vision.

**Verify:**

```sh
curl -s http://<host>:<port>/v1/models; echo
```

**If it fails:** `<name>: no chat server answers on <url>` → wrong URL, or the
server isn't up / is firewalled.

## 5.4 Which chat server is used

- Each server preset holds its chat servers. Asks are shared among the
  preset's `Chat` entries and its KoboldCpp image servers that have `chat`.
  KoboldCpp entries that swap are left out while a non-swapping one exists.
- With no chat server in use, asking says: `No chat server in use: add one to
  a server preset (Servers…), or pick one to continue without sd-servers`.
- **Continue without servers** lets you pick a chat server alone.
- System prompts and sampling are shared by every chat server: `pe: Pe {
  system_prompts: { … }, sampling: { … } }` in `config.rsn`, editable in the
  server status window. Missing means built-in.

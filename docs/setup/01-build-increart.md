# 1. Build and launch IncreArt

Sources: `.github/workflows/ci.yml`, `crates/increart-gui/Cargo.toml`,
`crates/increart-gui/src/main.rs`, `crates/increart-core/src/config.rs`,
`.devcontainer/Dockerfile`, `.github/workflows/release.yml`.

## 1.0 Or download a release (no build)

**Goal:** a ready-built IncreArt: skip 1.1 (except its "run" libraries on Linux) to 1.3, then launch as in 1.4.

**Preconditions:** a release on the GitHub Releases page with a zip for your
platform: `increart-<version>-linux-x86_64.zip`,
`increart-<version>-windows-x86_64.zip` or
`increart-<version>-macos-arm64.zip`. If there is none for your platform,
build instead (1.1 onwards).

**Commands (Linux):**

```sh
unzip increart-<version>-linux-x86_64.zip
cd increart-<version>-linux-x86_64
./increart
```

**Expected result:** the folder holds `increart` (`increart.exe` on
Windows), `README.md`, `LICENSE` and `Example library/`. The start-up list offers `Example library`,
tagged "example".

**Verify:** the window opens. The run-time libraries in 1.1's "run" line
must be installed on Linux. The Linux build is made on Ubuntu 22.04, so it
needs glibc 2.35 or newer (Ubuntu 22.04+, Debian 12+); on an older system,
build from source.

**If it fails:** the builds aren't signed. On macOS, if the app is blocked,
run `xattr -dr com.apple.quarantine .` in the unzipped folder. On Windows,
choose **More info → Run anyway** in SmartScreen.

## 1.1 System packages

**Goal:** the native libraries the GUI crate (eframe/egui, libwebp, fontconfig)
needs to build and to open a window.

**Preconditions:** Debian or Ubuntu with `sudo`. Other distributions: the
package names below are Debian's; equivalents are **untested**.

```sh
sudo apt-get update
# build: what CI installs on ubuntu-latest, plus a C toolchain (libwebp-sys compiles C)
sudo apt-get install -y build-essential pkg-config git curl \
    libfontconfig1-dev libxkbcommon-dev
# run: what the devcontainer installs to open the window on X11
sudo apt-get install -y libx11-6 libxcursor1 libxrandr2 libxi6 \
    libxkbcommon0 libxkbcommon-x11-0 libgl1 libegl1
# helpers: zenity (installed by the devcontainer; dialogs), vulkan-tools
# (vulkaninfo: the KoboldCpp wizard lists Vulkan GPUs with it)
sudo apt-get install -y zenity vulkan-tools
```

**Expected result:** apt exits 0.

**Verify:**

```sh
pkg-config --modversion fontconfig xkbcommon && cc --version | head -1
```

Prints two version numbers and a compiler line.

**If it fails:**
- `Unable to locate package` → run `sudo apt-get update` first; on older
  releases search with `apt-cache search libxkbcommon`.
- Windows / macOS: CI installs nothing extra there (`ci.yml`); skip this step.

## 1.2 Rust toolchain

**Goal:** stable Rust, new enough for the dependencies.

**Preconditions:** none. The workspace uses edition 2024; the highest
`rust-version` among the locked dependencies is **1.95** (egui 0.37). CI uses
the current stable.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustup update stable
```

**Expected result:** `rustup` installed, `stable` default.

**Verify:**

```sh
rustc --version   # must be 1.95.0 or newer
```

**If it fails:** `rustc 1.8x` → `rustup default stable && rustup update`.
A distro-packaged Rust is usually too old; use rustup.

## 1.3 Build

**Goal:** a release binary.

**Preconditions:** 1.1, 1.2; the repository cloned; you are in its root.

```sh
cargo build --release -p increart-gui
```

Optional AVIF output for Tile → Save needs `--features avif` and extra native
libraries (dav1d); that is **not covered** here.

**Expected result:** `Finished \`release\` profile`; the binary is
`target/release/increart` (`target\release\increart.exe` on Windows).

**Verify:**

```sh
./target/release/increart --help
```

Prints `usage: increart [--library <folder>|last] [--servers <preset>|last | --no-servers]`
and exits 0. `--version` prints `increart 0.1.0 (<commit>, <date>)`; the
commit has `-dirty` when the tree had uncommitted changes, and is left out
when built outside a git checkout. On Windows these print to the console
they're run from, too. (`--help` is parsed before any window opens, so it works without
a display.)

**If it fails:**
- A build-script error from `yeslogic-fontconfig-sys` or `libwebp-sys`, or
  `fontconfig` not found by pkg-config → step 1.1.
- `feature 'edition2024' is required` or `requires rustc 1.95` → step 1.2.
- `error: linker 'cc' not found` → `sudo apt-get install -y build-essential`.

Optional, as CI does: `cargo test --workspace` (no GPU or server needed; the
tests that need real servers skip themselves unless
`INCREART_SDSERVER_URL` / `INCREART_KOBOLDCPP` are set).

## 1.4 Launch

**Goal:** the window opens on the library screen.

**Preconditions:** 1.3; a desktop session (X11 or Wayland).

```sh
./target/release/increart
```

Command line (from `main.rs`):

| Flag | Effect |
|---|---|
| `--library <folder>` | open that library, skipping the library screen |
| `--library last` | the most recently opened library |
| `--servers <preset>` | open that server preset (name, any case); the main window opens while servers start |
| `--servers last` | the preset opened last |
| `--no-servers` | continue without servers |
| `-V`, `--version` | print the version and the commit built from, e.g. `increart 0.1.0 (b6bd961, 2026-10-29)`, and exit (Settings shows the same line) |
| `--check-library <folder>` | check a library without opening a window: every link reaches its image, every image the library names is there, every stored image decodes to the pixels it records, every generated image still has its output link. Prints the problems and a summary (with the seeds still to generate, if any); exits 0 if none, 1 if some, 2 if it can't be opened. The release workflow runs it on each zip's example library, and fails if it has seeds to generate |

A screen is still shown when its argument doesn't work (no `library.rsn`,
unknown preset, sd-server or models folder missing), with the reason. An
unknown argument prints the usage and exits with status 2.

**Expected result:** a 1400 × 900 window titled "IncreArt" with "Open a
library" (see [../gui/01-library-dialog.webp](../gui/01-library-dialog.webp)).
Continue with [06-configure-increart.md](06-configure-increart.md).

**Verify (without looking at the screen):**

```sh
./target/release/increart & sleep 5; pgrep -a increart; kill %1
```

`pgrep` shows the process still running after 5 s. SIGTERM closes it cleanly
(drafts saved, started servers stopped).

**If it fails:** an error naming `libEGL`, `libGL`, `libxkbcommon-x11` or
"no display" → install the run packages from 1.1, and check `echo $DISPLAY
$WAYLAND_DISPLAY` is not empty.

## 1.5 Where settings live: `INCREART_CONFIG`

**Goal:** know which `config.rsn` the app reads and writes.

**Preconditions:** none.

Order (`main.rs`, `Config::default_path`):

1. `INCREART_CONFIG=<file>` if set.
2. Else `increart/config.rsn` under the config folder:
   - Linux: `$XDG_CONFIG_HOME/increart/config.rsn`, else `~/.config/increart/config.rsn`
   - Windows: `%APPDATA%\increart\config.rsn`
   - macOS: `~/Library/Application Support/increart/config.rsn`
   - `XDG_CONFIG_HOME`, when set, wins on every OS.

A missing file means built-in defaults; the file is written the first time a
setting is saved. The GUI rewrites the whole file on save, so comments you add
by hand are lost.

State (logs, KoboldCpp admin folders): `$XDG_STATE_HOME/increart`, else
`~/.local/state/increart` (Linux), `%LOCALAPPDATA%\increart` (Windows),
`~/Library/Application Support/increart` (macOS, the same folder as the
config). Logs are in its `logs/`.

```sh
# a separate config, e.g. for testing
INCREART_CONFIG="$HOME/increart-test/config.rsn" ./target/release/increart
```

**Verify:** after saving any setting in the app (e.g. Server settings… →
close):

```sh
ls -l "${XDG_CONFIG_HOME:-$HOME/.config}/increart/config.rsn"
```

**If it fails:** the status bar says `settings not loaded (…); using
defaults` → the file doesn't parse; see
[07-troubleshooting.md](07-troubleshooting.md#config).

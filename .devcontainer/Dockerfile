# .devcontainer/Dockerfile
FROM mcr.microsoft.com/devcontainers/rust:1

# egui/eframe on the host's X server (DISPLAY and /tmp/.X11-unix are passed
# through in devcontainer.json), plus the C/C++ toolchain stable-diffusion.cpp
# needs (see scripts/build-sdcpp.sh).
RUN apt-get update && apt-get install -y \
    libx11-6 \
    libxcursor1 \
    libxrandr2 \
    libxi6 \
    libxkbcommon0 \
    libxkbcommon-x11-0 \
    libgl1 \
    libegl1 \
    xterm \
    zenity \
    build-essential \
    cmake \
    ninja-build \
    ccache \
    git \
    libgomp1 \
    && rm -rf /var/lib/apt/lists/*

# --- Vulkan (sd.cpp's SD_VULKAN backend) ------------------------------------
# RADV from Mesa talks to the host amdgpu driver through /dev/dri, so no
# vendor SDK is needed. vulkan-tools gives `vulkaninfo --summary` for checks.
RUN apt-get update && apt-get install -y \
    libvulkan-dev \
    glslc \
    spirv-headers \
    mesa-vulkan-drivers \
    vulkan-tools \
    && rm -rf /var/lib/apt/lists/*

# --- AMD ROCm userspace (sd.cpp's SD_HIPBLAS backend) -----------------------
#
# Same setup as IncreMusic-Yue2: two AMD Radeon AI PRO R9700 (Navi 48, RDNA4,
# gfx1201). gfx1201 is officially supported from ROCm 7.2; 7.2.4 is the newest
# build published under https://repo.radeon.com/rocm/apt/ as of this writing.
#
# Base image is Debian 13 (trixie); AMD's Debian 13 instructions point at the
# Ubuntu 24.04 ("noble") binary repo, which is intentional on AMD's part.
#
# Only the userspace runtime and HIP compiler are installed. The amdgpu kernel
# driver lives on the host and is passed through via /dev/kfd and /dev/dri, so
# amdgpu-dkms is never installed here.
#
# Only what sd.cpp's SD_HIPBLAS build links against: ROCm's clang, the HIP
# runtime and headers, hipBLAS and rocBLAS (which pull in rocSOLVER and
# hipBLASLt), plus rocminfo and rocm-smi (increart-core runs rocm-smi to read
# VRAM). The rocm-dev + rocm-libs metapackages made this layer 21 GB;
# MIOpen, rocFFT, RCCL, composable_kernel's static libs etc. aren't used.
#
# rocBLAS and hipBLASLt ship prebuilt kernels for a dozen GPU architectures.
# Kernels for anything but GPU_ARCH are deleted in the same RUN, so they never
# reach a layer. Files without an architecture in their name are kept.
ARG ROCM_VERSION=7.2.4
ARG GPU_ARCH=gfx1201
RUN mkdir --parents --mode=0755 /etc/apt/keyrings \
    && wget -qO- https://repo.radeon.com/rocm/rocm.gpg.key | gpg --dearmor > /etc/apt/keyrings/rocm.gpg \
    && printf 'deb [arch=amd64 signed-by=/etc/apt/keyrings/rocm.gpg] https://repo.radeon.com/rocm/apt/%s noble main\ndeb [arch=amd64 signed-by=/etc/apt/keyrings/rocm.gpg] https://repo.radeon.com/graphics/%s/ubuntu noble main\n' "$ROCM_VERSION" "$ROCM_VERSION" > /etc/apt/sources.list.d/rocm.list \
    && printf 'Package: *\nPin: release o=repo.radeon.com\nPin-Priority: 600\n' > /etc/apt/preferences.d/rocm-pin-600 \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        rocm-llvm hipcc hip-dev hip-runtime-amd rocm-device-libs rocm-cmake \
        hipblas-dev rocblas-dev \
        rocminfo rocm-smi-lib \
    && find /opt/rocm-${ROCM_VERSION}/lib/rocblas/library /opt/rocm-${ROCM_VERSION}/lib/hipblaslt/library \
        -type f -name '*gfx*' ! -name "*${GPU_ARCH}*" -delete \
    && rm -rf /var/lib/apt/lists/*

# /dev/kfd and /dev/dri are owned by the host's video/render GIDs (`getent
# group render video` on the host: render=992, video=44). Match them and put
# vscode in both so the GPUs work as a non-root user with --cap-drop=ALL.
ARG HOST_VIDEO_GID=44
ARG HOST_RENDER_GID=992
RUN (getent group video >/dev/null || groupadd video) \
    && groupmod -g ${HOST_VIDEO_GID} video \
    && (getent group render >/dev/null || groupadd render) \
    && groupmod -g ${HOST_RENDER_GID} render \
    && usermod -aG video,render vscode

# ROCm's clang links sd.cpp against its own OpenMP runtime in /opt/rocm/llvm/lib.
ENV PATH=/opt/rocm/bin:${PATH}
ENV LD_LIBRARY_PATH=/opt/rocm/lib:/opt/rocm/llvm/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}

# File manager for browsing the image library on the host's X server. Kept in
# its own late layer so adding it doesn't rebuild the ROCm layer above.
RUN apt-get update && apt-get install -y --no-install-recommends thunar \
    && rm -rf /var/lib/apt/lists/*

RUN chown -R vscode:rustlang "$CARGO_HOME" "$RUSTUP_HOME"

USER vscode
RUN rustup toolchain install nightly --component llvm-tools-preview \
    && cargo install cargo-llvm-cov --locked

# Sound through the host's PipeWire/PulseAudio socket (mounted at /run/pulse in
# devcontainer.json). ALSA's default device goes to pulse so plain `aplay`
# works; paplay/pactl are there for checks. Last layer so it doesn't
# invalidate the toolchain layers above.
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
    alsa-utils \
    libasound2-plugins \
    pulseaudio-utils \
    && rm -rf /var/lib/apt/lists/* \
    && printf 'pcm.!default { type pulse }\nctl.!default { type pulse }\n' > /etc/asound.conf
USER vscode

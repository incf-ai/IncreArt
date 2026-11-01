//! How config.rsn writes a server preset's entries (DESIGN.md, Server
//! presets): each named for the program it is (`SdServer`, `KoboldCpp`,
//! `OpenAiChat`), started by the app (`run: Launch {…}`) or already running
//! (`run: Connect {…}`), on `gpu: Rocm(0)`, `Vulkan([1, 2])`, `Cpu(8)`, ….
//! The app's own types ([`ServerConfig`]) stay as they were: validation
//! stamps hash them. Entries as older configs wrote them (`Start`,
//! `Connect`, `Chat(PeServer …)`, `Kobold(KoboldServer …)`) are read too.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::ServerConfig;
use crate::kobold::{KoboldChat, KoboldRun, KoboldServer};
use crate::pe::{GpuRuntime, PeDevice, PeRun, PeServer, PeStart};

/// `serialize_with` for `ServerPreset::servers`.
pub fn serialize<S: Serializer>(servers: &[ServerConfig], s: S) -> Result<S::Ok, S::Error> {
    let wire = servers
        .iter()
        .map(Entry::from_config)
        .collect::<Result<Vec<_>, String>>()
        .map_err(serde::ser::Error::custom)?;
    wire.serialize(s)
}

/// `deserialize_with` for `ServerPreset::servers`.
pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ServerConfig>, D::Error> {
    Vec::<Entry>::deserialize(d)?.into_iter().map(|e| e.into_config().map_err(D::Error::custom)).collect()
}

#[derive(Serialize, Deserialize)]
enum Entry {
    /// stable-diffusion.cpp's sd-server: makes images.
    SdServer { name: String, run: SdRun },
    /// KoboldCpp: makes images (`images`, unless `false`), and is prompt
    /// enhance's chat server too with `chat`.
    KoboldCpp {
        name: String,
        run: KoboldCppRun,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        images: bool,
        #[serde(default, serialize_with = "crate::rsnfmt::bare", skip_serializing_if = "Option::is_none")]
        chat: Option<Chat>,
        /// Chat and image model take turns, one loaded at a time.
        #[serde(default, skip_serializing_if = "is_false")]
        swap: bool,
        /// A chat-only one (`images: false`): when it starts.
        #[serde(default, skip_serializing_if = "is_default")]
        when: When,
    },
    /// Any OpenAI-compatible chat server already running, for prompt
    /// enhance.
    OpenAiChat {
        name: String,
        url: String,
        #[serde(default, skip_serializing_if = "is_default")]
        when: When,
    },

    // older configs' entries: read, never written
    #[serde(skip_serializing)]
    Start {
        name: String,
        #[serde(default = "localhost")]
        host: String,
        port: u16,
        device: String,
        #[serde(default)]
        args: Vec<String>,
    },
    #[serde(skip_serializing)]
    Connect {
        name: String,
        url: String,
        #[serde(default)]
        device: String,
    },
    #[serde(skip_serializing)]
    Chat(PeServer),
    #[serde(skip_serializing)]
    Kobold(KoboldServer),
}

#[derive(Serialize, Deserialize)]
enum SdRun {
    Launch {
        #[serde(default = "localhost", skip_serializing_if = "is_localhost")]
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "is_default")]
        gpu: Gpu,
        /// Arguments for this server only (after the shared ones).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    Connect {
        url: String,
        /// The device it runs on, for recipes (optional).
        #[serde(default, skip_serializing_if = "is_default")]
        gpu: Gpu,
    },
}

#[derive(Serialize, Deserialize)]
enum KoboldCppRun {
    Launch {
        /// Empty: the shared one (`programs.koboldcpp`).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        binary: String,
        /// A `.kcpps` from KoboldCpp's launcher to start from.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        config: String,
        #[serde(default = "localhost", skip_serializing_if = "is_localhost")]
        host: String,
        port: u16,
        #[serde(default, skip_serializing_if = "is_default")]
        gpu: Gpu,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    Connect {
        url: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        password: String,
        #[serde(default, skip_serializing_if = "is_default")]
        gpu: Gpu,
    },
}

/// The chat model KoboldCpp runs (beside the image model, or alone).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename = "Chat", default)]
struct Chat {
    #[serde(skip_serializing_if = "String::is_empty")]
    model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    mmproj: String,
    #[serde(skip_serializing_if = "is_default")]
    context: u32,
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
enum When {
    #[default]
    OnDemand,
    #[serde(alias = "AtLaunch")]
    AtOpen,
}

/// What a server runs on: sd-server's `--backend` (one device) or KoboldCpp's
/// GPUs through a runtime (`[]`: every GPU).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
enum Gpu {
    /// The program's own choice.
    #[default]
    Auto,
    /// The CPU, with this many threads (0: the program's choice).
    Cpu(u32),
    Vulkan(Devices),
    Rocm(Devices),
    Cuda(Devices),
    Metal(Devices),
    /// An sd-server `--backend` written as it is.
    Backend(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum Devices {
    One(u32),
    Many(Vec<u32>),
}

impl Devices {
    fn of(list: &[u32]) -> Devices {
        match list {
            [n] => Devices::One(*n),
            _ => Devices::Many(list.to_vec()),
        }
    }

    fn list(&self) -> Vec<u32> {
        match self {
            Devices::One(n) => vec![*n],
            Devices::Many(l) => l.clone(),
        }
    }
}

impl Gpu {
    /// sd-server's `--backend` (`ROCm0`, `Vulkan1`, `CUDA0`, `CPU`).
    fn from_backend(device: &str) -> Gpu {
        let split = device.find(|c: char| c.is_ascii_digit()).unwrap_or(device.len());
        let (prefix, n) = device.split_at(split);
        let one = |n: &str| n.parse().ok().map(Devices::One);
        let gpu = match (prefix, n) {
            ("", "") => Some(Gpu::Auto),
            ("CPU", "") => Some(Gpu::Cpu(0)),
            ("ROCm", n) => one(n).map(Gpu::Rocm),
            ("Vulkan", n) => one(n).map(Gpu::Vulkan),
            ("CUDA", n) => one(n).map(Gpu::Cuda),
            ("Metal", "") => Some(Gpu::Metal(Devices::Many(vec![]))),
            _ => None,
        };
        // only a form written back the same (`ROCm00` stays as it is)
        match gpu {
            Some(g) if g.backend().as_deref() == Ok(device) => g,
            _ => Gpu::Backend(device.into()),
        }
    }

    fn backend(&self) -> Result<String, String> {
        let one = |prefix: &str, d: &Devices| match d.list()[..] {
            [n] => Ok(format!("{prefix}{n}")),
            _ => Err(format!("an sd-server runs on one device, not {d:?}")),
        };
        match self {
            Gpu::Auto => Ok(String::new()),
            Gpu::Cpu(0) => Ok("CPU".into()),
            Gpu::Cpu(n) => {
                Err(format!("an sd-server's thread count goes in its args (`--threads {n}`), and gpu is Cpu(0)"))
            }
            Gpu::Rocm(d) => one("ROCm", d),
            Gpu::Vulkan(d) => one("Vulkan", d),
            Gpu::Cuda(d) => one("CUDA", d),
            Gpu::Metal(d) if d.list().is_empty() => Ok("Metal".into()),
            Gpu::Metal(d) => one("Metal", d),
            Gpu::Backend(s) => Ok(s.clone()),
        }
    }

    fn from_kobold(device: &PeDevice) -> Gpu {
        match device {
            PeDevice::Auto => Gpu::Auto,
            PeDevice::Cpu { threads } => Gpu::Cpu(*threads),
            PeDevice::Gpu { devices, runtime } => {
                let d = Devices::of(devices);
                match runtime {
                    GpuRuntime::Vulkan => Gpu::Vulkan(d),
                    GpuRuntime::Rocm => Gpu::Rocm(d),
                    GpuRuntime::Cuda => Gpu::Cuda(d),
                    GpuRuntime::Metal => Gpu::Metal(d),
                }
            }
        }
    }

    fn kobold(&self) -> Result<PeDevice, String> {
        let gpu = |d: &Devices, runtime| Ok(PeDevice::Gpu { devices: d.list(), runtime });
        match self {
            Gpu::Auto => Ok(PeDevice::Auto),
            Gpu::Cpu(threads) => Ok(PeDevice::Cpu { threads: *threads }),
            Gpu::Vulkan(d) => gpu(d, GpuRuntime::Vulkan),
            Gpu::Rocm(d) => gpu(d, GpuRuntime::Rocm),
            Gpu::Cuda(d) => gpu(d, GpuRuntime::Cuda),
            Gpu::Metal(d) => gpu(d, GpuRuntime::Metal),
            Gpu::Backend(s) => Err(format!(
                "a KoboldCpp the app starts runs on Auto, Cpu(threads), Vulkan(…), Rocm(…), Cuda(…) or Metal(…), not Backend({s:?})"
            )),
        }
    }
}

impl From<PeStart> for When {
    fn from(s: PeStart) -> When {
        match s {
            PeStart::OnDemand => When::OnDemand,
            PeStart::AtLaunch => When::AtOpen,
        }
    }
}

impl From<When> for PeStart {
    fn from(w: When) -> PeStart {
        match w {
            When::OnDemand => PeStart::OnDemand,
            When::AtOpen => PeStart::AtLaunch,
        }
    }
}

impl Entry {
    fn from_config(server: &ServerConfig) -> Result<Entry, String> {
        Ok(match server.clone() {
            ServerConfig::Start { name, host, port, device, args } => {
                Entry::SdServer { name, run: SdRun::Launch { host, port, gpu: Gpu::from_backend(&device), args } }
            }
            ServerConfig::Connect { name, url, device } => {
                Entry::SdServer { name, run: SdRun::Connect { url, gpu: Gpu::from_backend(&device) } }
            }
            ServerConfig::Kobold(KoboldServer { name, run, chat, swap }) => Entry::KoboldCpp {
                name,
                run: match run {
                    KoboldRun::Start { binary, config, host, port, device, args } => {
                        KoboldCppRun::Launch { binary, config, host, port, gpu: Gpu::from_kobold(&device), args }
                    }
                    KoboldRun::Connect { url, password, device } => {
                        KoboldCppRun::Connect { url, password, gpu: Gpu::from_backend(&device) }
                    }
                },
                images: true,
                chat: chat.map(|c| Chat { model: c.model, mmproj: c.mmproj, context: c.context }),
                swap,
                when: When::OnDemand,
            },
            ServerConfig::Chat(PeServer { name, run, start, .. }) => match run {
                PeRun::KoboldCpp { binary, model, mmproj, config, host, port, device, context, args } => {
                    Entry::KoboldCpp {
                        name,
                        run: KoboldCppRun::Launch { binary, config, host, port, gpu: Gpu::from_kobold(&device), args },
                        images: false,
                        chat: Some(Chat { model, mmproj, context }),
                        swap: false,
                        when: start.into(),
                    }
                }
                PeRun::Connect { url } => Entry::OpenAiChat { name, url, when: start.into() },
            },
        })
    }

    fn into_config(self) -> Result<ServerConfig, String> {
        Ok(match self {
            Entry::SdServer { name, run: SdRun::Launch { host, port, gpu, args } } => ServerConfig::Start {
                device: gpu.backend().map_err(|e| format!("{name}: {e}"))?,
                name,
                host,
                port,
                args,
            },
            Entry::SdServer { name, run: SdRun::Connect { url, gpu } } => {
                ServerConfig::Connect { device: gpu.backend().map_err(|e| format!("{name}: {e}"))?, name, url }
            }
            Entry::KoboldCpp { name, run, images: true, chat, swap, when } => {
                if when != When::OnDemand {
                    return Err(format!(
                        "{name}: `when` is for a chat-only KoboldCpp (images: false); one making images starts with its preset"
                    ));
                }
                let run = match run {
                    KoboldCppRun::Launch { binary, config, host, port, gpu, args } => KoboldRun::Start {
                        device: gpu.kobold().map_err(|e| format!("{name}: {e}"))?,
                        binary,
                        config,
                        host,
                        port,
                        args,
                    },
                    KoboldCppRun::Connect { url, password, gpu } => {
                        KoboldRun::Connect { device: gpu.backend().map_err(|e| format!("{name}: {e}"))?, url, password }
                    }
                };
                let chat = chat.map(|c| KoboldChat { model: c.model, mmproj: c.mmproj, context: c.context });
                ServerConfig::Kobold(KoboldServer { name, run, chat, swap })
            }
            Entry::KoboldCpp { name, run, images: false, chat, swap, when } => {
                if swap {
                    return Err(format!(
                        "{name}: `swap` takes turns with the image model, and this one has none (images: false)"
                    ));
                }
                let KoboldCppRun::Launch { binary, config, host, port, gpu, args } = run else {
                    return Err(format!(
                        "{name}: a running chat-only KoboldCpp is OpenAiChat {{ name, url }} (or KoboldCpp with images, if it makes them)"
                    ));
                };
                let Chat { model, mmproj, context } = chat.unwrap_or_default();
                let device = gpu.kobold().map_err(|e| format!("{name}: {e}"))?;
                let run = PeRun::KoboldCpp { binary, model, mmproj, config, host, port, device, context, args };
                ServerConfig::Chat(PeServer { name, run, start: when.into(), text_only: false })
            }
            Entry::OpenAiChat { name, url, when } => {
                ServerConfig::Chat(PeServer { name, run: PeRun::Connect { url }, start: when.into(), text_only: false })
            }
            Entry::Start { name, host, port, device, args } => ServerConfig::Start { name, host, port, device, args },
            Entry::Connect { name, url, device } => ServerConfig::Connect { name, url, device },
            Entry::Chat(s) => ServerConfig::Chat(s),
            Entry::Kobold(k) => ServerConfig::Kobold(k),
        })
    }
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn localhost() -> String {
    "127.0.0.1".into()
}

fn is_localhost(h: &String) -> bool {
    h == "127.0.0.1"
}

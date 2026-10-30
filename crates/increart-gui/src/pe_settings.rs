//! **Prompt enhance settings**: a preset's chat server: its model, what it
//! runs on and when it starts (DESIGN.md, Prompt enhance).

use eframe::egui::{self, RichText};
use increart_core::pe::{GpuRuntime, PeDevice, PeRun, PeServer, PeStart};
use increart_gui_core::{AppState, Msg, PeFile, PeStatus, PickKind};

use crate::widgets::{BAD, dialog_buttons, ic, ph};

/// The chat server in use in one line, with ways to start and stop it: the
/// server status window's PE row.
pub fn summary(ui: &mut egui::Ui, st: &AppState, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        let Some(server) = &st.pe.server else {
            ui.weak("No chat server in use: add one to a server preset (Change servers…).");
            return;
        };
        ui.label(RichText::new(&server.name).strong());
        let (dot, state, color) = crate::app::pe_state(ui, st);
        // the name is just left of it
        let state = state.split_once(": ").map_or(state.as_str(), |(_, s)| s).to_string();
        // the buttons before the state, which changes many times a second
        match st.pe.status {
            PeStatus::Off | PeStatus::Failed(_) => {
                let start = if matches!(server.run, PeRun::Connect { .. }) { "Connect" } else { "Start now" };
                if ui
                    .small_button(ic(ph::PLAY, start))
                    .on_hover_text("load the model now, not when first asked")
                    .clicked()
                {
                    out.push(Msg::PeStart);
                }
            }
            PeStatus::Starting | PeStatus::Ready => {
                if matches!(server.run, PeRun::KoboldCpp { .. })
                    && ui
                        .small_button(ic(ph::STOP, "Stop"))
                        .on_hover_text("frees its memory; asks waiting are dropped")
                        .clicked()
                {
                    out.push(Msg::PeShutdown);
                }
            }
        }
        // cut to what's left (egui shows it whole on hover)
        ui.add(egui::Label::new(RichText::new(format!("{dot} {state}")).color(color)).truncate());
    });
    if let Some(server) = &st.pe.server {
        ui.weak(describe(server));
    }
}

/// How a server runs, in a line.
pub fn describe(server: &PeServer) -> String {
    let when = match server.start {
        PeStart::OnDemand => "started when first asked",
        PeStart::AtLaunch => "started with its preset",
    };
    match &server.run {
        PeRun::KoboldCpp { model, mmproj, config, device, .. } => {
            let file =
                |p: &str| std::path::Path::new(p).file_name().map_or(p.to_string(), |f| f.to_string_lossy().into());
            let vision = if mmproj.is_empty() { "no vision".to_string() } else { format!("vision: {}", file(mmproj)) };
            let on = match device {
                PeDevice::Auto => String::new(),
                PeDevice::Cpu { .. } => " on the CPU".into(),
                PeDevice::Gpu { devices, runtime } if devices.is_empty() => {
                    format!(" on every GPU ({})", runtime.label())
                }
                PeDevice::Gpu { devices, runtime } => {
                    let n: Vec<String> = devices.iter().map(u32::to_string).collect();
                    format!(" on {} GPU {}", runtime.label(), n.join(", "))
                }
            };
            let model = if model.is_empty() { file(config) } else { file(model) };
            format!("KoboldCpp{on} at {}, {when}: {model} ({vision})", server.base_url())
        }
        PeRun::Connect { url } => format!("Connects to {url}, {when}"),
    }
}

/// The settings of `st.pe.editor`'s server.
pub fn window(ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
    let Some(e) = &st.pe.editor else { return };
    let mut d = e.draft.clone();
    let m = egui::Modal::new(egui::Id::unique("pe_settings")).show(ctx, |ui| {
        ui.set_width(600.0);
        ui.heading(ic(ph::MAGIC_WAND, "Chat server"));
        ui.label("The chat model prompt enhance uses to suggest prompt text: KoboldCpp started by IncreArt, or any OpenAI-compatible server already running.");
        ui.add_space(8.0);
        egui::Grid::new("pe_settings_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("Server");
            ui.horizontal(|ui| {
                let kobold = matches!(d.run, PeRun::KoboldCpp { .. });
                if ui.radio(kobold, "KoboldCpp, started by IncreArt").clicked() && !kobold {
                    d.run = kobold_from(&d.run, st.gpu_runtime);
                }
                if ui.radio(!kobold, "Connect to one running").clicked() && kobold {
                    d.run = PeRun::Connect { url: d.base_url() };
                }
            });
            ui.end_row();
            match &mut d.run {
                PeRun::KoboldCpp { binary, model, mmproj, config, host, port, device, context, args } => {
                    file_row(ui, "Binary", binary, "koboldcpp, or a command line (python3 koboldcpp.py)", PeFile::Binary, out);
                    file_row(ui, "Settings", config, ".kcpps from KoboldCpp's launcher (optional; the rest goes over it)", PeFile::Config, out);
                    let hint = if config.is_empty() { ".gguf" } else { ".gguf (empty: the settings file's)" };
                    file_row(ui, "Model", model, hint, PeFile::Model, out);
                    file_row(ui, "Vision", mmproj, "mmproj .gguf (empty: text only, no image reading)", PeFile::Mmproj, out);
                    ui.label("Address");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(host).desired_width(140.0));
                        ui.label("port");
                        ui.add(egui::DragValue::new(port).range(1..=65535));
                    });
                    ui.end_row();
                    ui.label("Runs on");
                    ui.vertical(|ui| device_choice(ui, st, device, out));
                    ui.end_row();
                    ui.label("Context");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(context).range(0..=131072).speed(256));
                        ui.weak(if *context == 0 { "tokens (KoboldCpp's default)" } else { "tokens" });
                    })
                    .response
                    .on_hover_text("--contextsize: room for the images, prompt and answer; 8192 is plenty");
                    ui.end_row();
                    ui.label("More args");
                    let mut text = args.join(" ");
                    if ui
                        .add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).hint_text("--blasthreads 24 …"))
                        .changed()
                    {
                        *args = text.split_whitespace().map(String::from).collect();
                    }
                    ui.end_row();
                }
                PeRun::Connect { url } => {
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(url).desired_width(f32::INFINITY).hint_text("http://127.0.0.1:5001"));
                    ui.end_row();
                }
            }
            ui.label("Starts");
            ui.horizontal(|ui| {
                let (first, launch) = match d.run {
                    PeRun::KoboldCpp { .. } => ("when first asked", "when the preset opens"),
                    PeRun::Connect { .. } => ("connects when first asked", "connects when the preset opens"),
                };
                ui.radio_value(&mut d.start, PeStart::OnDemand, first)
                    .on_hover_text("no memory used until then; the first ask waits for the model to load");
                ui.radio_value(&mut d.start, PeStart::AtLaunch, launch)
                    .on_hover_text("the model loads meanwhile, so the first ask is quick; its memory stays in use");
            });
            ui.end_row();
        });
        if let Some(err) = &e.error {
            ui.add_space(4.0);
            ui.colored_label(BAD, err);
        }
        if st.pe.server.as_ref().is_some_and(|s| s.name == d.name && !s.same_process(&d)) && st.pe.status != PeStatus::Off {
            ui.add_space(4.0);
            ui.weak("Opening the preset again restarts the one in use with these settings.");
        }
        ui.add_space(8.0);
        let enter = st.confirm.is_none() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        dialog_buttons(ui, |ui| {
            let save = if e.index.is_some() { "Save   Enter" } else { "Add   Enter" };
            if ui.button(RichText::new(ic(ph::CHECK, save)).strong()).clicked() || enter {
                out.push(Msg::PeEditorSave);
            }
            if ui.button("Cancel   Esc").clicked() {
                out.push(Msg::PeEditorClose);
            }
        });
    });
    if d != e.draft {
        out.push(Msg::PeEditorEdit(d));
    }
    if m.should_close() && st.confirm.is_none() {
        out.push(Msg::PeEditorClose);
    }
}

/// KoboldCpp settings for a server that connected before (its address
/// kept).
fn kobold_from(run: &PeRun, runtime: GpuRuntime) -> PeRun {
    let (host, port) = match run {
        PeRun::Connect { url } => {
            let rest = url.split_once("://").map_or(url.as_str(), |(_, r)| r).trim_end_matches('/');
            match rest.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), p.parse().unwrap_or(5001)),
                None => (rest.to_string(), 5001),
            }
        }
        PeRun::KoboldCpp { .. } => return run.clone(),
    };
    PeRun::KoboldCpp {
        binary: String::new(),
        model: String::new(),
        mmproj: String::new(),
        config: String::new(),
        host: if host.is_empty() { "127.0.0.1".into() } else { host },
        port,
        device: PeDevice::Gpu { devices: vec![], runtime },
        context: 8192,
        args: vec![],
    }
}

fn file_row(ui: &mut egui::Ui, label: &str, path: &mut String, hint: &str, f: PeFile, out: &mut Vec<Msg>) {
    ui.label(label);
    crate::startup::path_edit(ui, path, hint, PickKind::Pe(f), out);
    ui.end_row();
}

/// Auto / CPU (threads) / GPUs (ticked from those listed; none ticked:
/// every GPU).
fn device_choice(ui: &mut egui::Ui, st: &AppState, device: &mut PeDevice, out: &mut Vec<Msg>) {
    ui.horizontal(|ui| {
        if ui
            .radio(matches!(device, PeDevice::Gpu { .. }), "GPU")
            .on_hover_text("Every layer on the GPU: much quicker")
            .clicked()
            && !matches!(device, PeDevice::Gpu { .. })
        {
            *device = PeDevice::Gpu { devices: vec![], runtime: st.gpu_runtime };
        }
        if ui
            .radio(matches!(device, PeDevice::Cpu { .. }), "CPU")
            .on_hover_text("leaves the GPUs to image generation")
            .clicked()
            && !matches!(device, PeDevice::Cpu { .. })
        {
            *device = PeDevice::Cpu { threads: 0 };
        }
        if ui.radio(*device == PeDevice::Auto, "KoboldCpp's choice").on_hover_text("or what More args say").clicked() {
            *device = PeDevice::Auto;
        }
    });
    match device {
        PeDevice::Auto => {}
        PeDevice::Cpu { threads } => {
            ui.horizontal(|ui| {
                ui.label("Threads");
                ui.add(egui::DragValue::new(threads).range(0..=256));
                ui.weak(if *threads == 0 { "(KoboldCpp's choice)" } else { "" });
            });
        }
        PeDevice::Gpu { devices, runtime } => {
            ui.horizontal(|ui| {
                ui.label("Runtime");
                // numbered differently (listed again once edited)
                if runtime_choice(ui, "pe_runtime", runtime) {
                    devices.clear();
                }
            });
            match &st.pe.gpus {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak("listing the GPUs…");
                    });
                }
                Some(Ok(list)) => {
                    for (n, name) in list {
                        let mut on = devices.contains(n);
                        if ui.checkbox(&mut on, format!("{n}: {name}")).changed() {
                            devices.retain(|d| d != n);
                            if on {
                                devices.push(*n);
                                devices.sort();
                            }
                        }
                    }
                    let unknown: Vec<String> =
                        devices.iter().filter(|d| !list.iter().any(|(n, _)| n == *d)).map(u32::to_string).collect();
                    if !unknown.is_empty() {
                        ui.colored_label(BAD, format!("not found: GPU {}", unknown.join(", ")));
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(BAD, e);
                }
            }
            ui.horizontal(|ui| {
                ui.weak(if devices.is_empty() {
                    "none ticked: every GPU, the model split across them"
                } else if devices.len() == 1 {
                    "one GPU: no splitting, the quickest"
                } else {
                    "the model split across these"
                });
                if st.pe.gpus.is_some() && ui.small_button(ic(ph::ARROWS_CLOCKWISE, "List again")).clicked() {
                    out.push(Msg::PeListGpus(*runtime));
                }
            });
            ui.weak("Shares GPU memory with image generation: about 7 GB for an 8B model.");
        }
    }
}

/// Vulkan / CUDA / ROCm, or Metal on a Mac; true when changed.
pub(crate) fn runtime_choice(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    runtime: &mut GpuRuntime,
) -> bool {
    let before = *runtime;
    egui::ComboBox::from_id_salt(id).selected_text(runtime.label()).show_ui(ui, |ui| {
        let mut list = GpuRuntime::here();
        if !list.contains(&before) {
            list.push(before);
        }
        for r in list {
            let hover = match r {
                GpuRuntime::Vulkan => "Any GPU with a Vulkan driver; every KoboldCpp build but the Mac one has it",
                GpuRuntime::Cuda => "NVIDIA GPUs, with KoboldCpp's CUDA build (not the nocuda one)",
                GpuRuntime::Rocm => "AMD GPUs, with a ROCm build of KoboldCpp (koboldcpp-rocm)",
                GpuRuntime::Metal => "Apple silicon, with KoboldCpp's Mac build",
            };
            ui.selectable_value(runtime, r, r.label()).on_hover_text(hover);
        }
    });
    *runtime != before
}

//! `INCREART_TOUR=<dir>`: screenshots of each screen, driven through the
//! reducer, then exit. `INCREART_TOUR_LIBRARY` is the library to open (a
//! copy of the sample library). Run it on a private display:
//! `scripts/headless.sh env INCREART_TOUR=… increart`.

use std::path::PathBuf;

use eframe::egui;
use increart_core::config::SheetRows;
use increart_core::{ImageRef, Tree};
use increart_gui_core::{
    AppState, IdeaDraft, InboxTab, LibraryDialog, LibraryWizard, Msg, OutputSource, OutputTab, Phase, ProjectsTab,
    ReviewView, ServerDialog, Tab, ZoomGrid, counts,
};

use crate::textures::Textures;

/// Where the folder view's first icon is, in the tour's window.
const FV_FIRST_ENTRY: (f32, f32) = (330.0, 140.0);

const SCENES: &[&str] = &[
    "01-library-dialog",
    "53-library-wizard",
    "55-library-open-wizard",
    "02-server-dialog",
    "02b-pe-settings",
    "02c-koboldcpp-entry",
    "02d-model-presets-from-servers",
    "54-server-wizard",
    "54a-server-wizard-choose",
    "54c-server-wizard-pe-own-gpu",
    "54d-server-wizard-pe-on-one",
    "54b-server-wizard-sd-server",
    "03-idea-inbox-grid",
    "04-idea-inbox-new-idea",
    "05-input-library",
    "06-projects-grid",
    "06b-projects-pane",
    "06c-projects-pane-run",
    "07-projects-generate",
    "07b-generate-blanks",
    "08-projects-jobs",
    "09-review",
    "10-output-library",
    "11-jobs-popover",
    "12-review-grid",
    "13-review-compare",
    "13b-review-follow",
    "13c-review-compare-with",
    "13d-review-compare-input",
    "14-output-library-rejected",
    "14b-output-library-compare",
    "15-generate-img2img",
    "16-servers-opening",
    "17-prompt-history",
    "18-edit-idea",
    "19-idea-inbox-import",
    "20-input-library-filters",
    "21-resolutions",
    "21b-model-presets",
    "22-output-library-lineage",
    "23-export",
    "24-projects-grid-tile-actions",
    // a check, not a screenshot: clicks the selected tile's Generate button
    "25-check-grid-click",
    "26-projects-folder",
    "27-projects-folder-images",
    "28-projects-folder-details",
    "29-projects-folder-project",
    "30-projects-folder-runs",
    "38-scale-dialog",
    "39-output-library-folder",
    "40-output-library-smart-folder",
    "42-input-library-folder",
    "43-idea-inbox-folder",
    "44-projects-folder-compact",
    "45-reference-preview",
    "47-new-project-name",
    "49-output-library-show-outputs",
    // last: it leaves the server dialog open
    "52-server-dialog-edit",
];

/// Scenes run only when named in `INCREART_TOUR_SCENES` (checks, not
/// screenshots).
const EXTRA_SCENES: &[&str] = &[
    "31-check-rename-tree-grid",
    "32-check-rename-tree-folder",
    "33-check-rename-folder-view",
    "34-check-drag-tree",
    "35-check-drag-folder-view",
    "36-check-grid-hover",
    "37-check-width-rounding",
    "41-check-rubber-band",
    "46-check-ref-double-click",
    "48-check-copy-image",
    "50-output-library-show-outputs-empty",
    "51-output-library-show-outputs-any-rating",
    "56-server-status",
    "57-no-server-warning",
    "58-server-dialog-from-main",
    "59-continue-without-servers",
    // after 02
    "60-server-settings",
    "61-duplicate-project",
    "62-rename",
    "63-projects-search",
    "64-prompt-presets",
    "65-prompt-presets-search",
    "66-preset-editor",
    "67-save-preset",
    "68-switch-library",
    "69-style-sweep",
    "70-contact-sheet",
    "71-job-info",
    "72-review-sweep-button",
    "73-promote",
    "74-notification-settings",
    "74b-timeout-settings",
    "74c-bell-ringing",
    "75-promote-in-folder",
    "76-top-bar-wide",
    "77-top-bar-medium",
    "78-top-bar-narrow",
    "79-contact-sheet-filter-zoom",
    "80-stop-buttons",
    "81-stop-finishing",
    "82-preview-zoom",
    "83-preview-next",
    "84-projects-folder-menu",
    "85-ref-picker",
    "86-image-title",
    "87-messages-popover",
    "88-tabs-zoomed",
    "89-review-preview",
    "90-preview-keys",
    "91-sweep-tick-all",
    "91b-sweep-blanks",
    "91b1-sweep-random",
    "91b2-sweep-random-again",
    "91c0-sweep-suggesting",
    "91c-sweep-suggest",
    "91c1-suggest-dialog",
    "91c1a-batch-suggest",
    "91c1b-style-prompts",
    "91c1c-enhance-dialog",
    "91c1d-jobs-enhancing",
    "91c2-rewrite-writing",
    "91d-rewrite-prompt",
    "91e-describe-image",
    "92-check-sheet-arrows",
    "92b-check-sheet-arrows-folded",
    "93-check-sheet-arrows-click",
    "94-check-sheet-arrows-slider",
    "95-check-sheet-preview-arrows",
    "96-review-picked",
    "96b-picked-menu",
    "96c-sheet-band",
    "97-tile-window",
    "97b-check-tile-drag",
    "97c-tile-menu",
    "97d-tile-cell-menu",
    "97e-tile-text-title",
    "97h-tile-picker",
    "15b-crop-mark",
    "97f-project-runs",
    "97g-output-by-project-runs",
    "97h-input-library-panel",
    "97i-check-output-double-click",
    "97j-check-library-click",
    "98-setting-sweep",
    "98b-model-sweep",
    "98c-output-sources",
    "98d-output-project-search",
    "98e-check-show-outputs-reveal",
    "98f-show-outputs-reveal",
    "98g-presets-group",
    "98h-server-logs",
    // after 02, with a config whose first preset's server can't start (a
    // KoboldCpp binary that isn't there): the error under its row
    "98i-server-start-failed",
    // checks, not screenshots: the selected idea card's own buttons
    "98j-check-library-card-refs",
    "98k-check-inbox-card-keep",
    "99-quit-with-unsaved-draft",
    // the README's shots, on the demo library (scripts/demo-library.json),
    // after 02-server-dialog and 03-idea-inbox-grid
    "R1-review-grid",
    "R2-review-compare",
    "R3-generate",
    "R4-output-lineage",
    "R5-input-library",
    "R6-projects-grid",
    // the README's gallery (docs/readme/GALLERY.md, scripts/make-readme-gallery.sh),
    // on the demo library too, after 02-server-dialog
    "G01-server-wizard",
    "G02-idea-inbox",
    "G03-projects-grid",
    "G04-generate-refs",
    "G05-check-select-mika",
    "G06-setting-sweep",
    "G07-prompt-history",
    "G08-jobs",
    "G09-contact-sheet",
    "G10-tile-window",
    "G11-export",
    // last: its status message would show in the shots after it
    "G12-crop-mark",
];

/// The tour library's fox-cafe project (clients/acme/fox-cafe).
fn fox_cafe(st: &AppState) -> Option<increart_core::Id> {
    st.index.projects.iter().find(|p| p.name() == "fox-cafe").map(|p| p.project.id.clone())
}

pub struct Tour {
    /// The scenes to run: all of `SCENES`, or those whose names start with
    /// one of the comma-separated prefixes in `INCREART_TOUR_SCENES`.
    scenes: Vec<&'static str>,
    dir: PathBuf,
    library: String,
    scene: usize,
    frame: u32,
    settled: u32,
    /// When the scene started (egui time), so windows finish fading in.
    started: f64,
    /// Synthetic input for the next frame (click checks).
    inject: Vec<egui::Event>,
    clicked: bool,
    /// A click check's before: the idea clicked, and a count.
    card_before: (Option<increart_core::Id>, usize),
    /// Tile rects before the hover check.
    before: Vec<Option<egui::Rect>>,
    /// The reveal check's step (each restarts the scene's clock).
    stage: u8,
}

impl Tour {
    pub fn from_env() -> Option<Tour> {
        let dir = PathBuf::from(std::env::var_os("INCREART_TOUR")?);
        let library = std::env::var("INCREART_TOUR_LIBRARY").unwrap_or_default();
        std::fs::create_dir_all(&dir).ok()?;
        let scenes = match std::env::var("INCREART_TOUR_SCENES") {
            Ok(list) => {
                let prefixes: Vec<_> =
                    list.split(',').map(str::trim).filter(|p| !p.is_empty()).map(String::from).collect();
                SCENES
                    .iter()
                    .chain(EXTRA_SCENES)
                    .copied()
                    .filter(|s| prefixes.iter().any(|p| s.starts_with(p.as_str())))
                    .collect()
            }
            Err(_) => SCENES.to_vec(),
        };
        Some(Tour {
            scenes,
            dir,
            library,
            scene: 0,
            frame: 0,
            settled: 0,
            started: 0.0,
            inject: vec![],
            clicked: false,
            card_before: (None, 0),
            before: vec![],
            stage: 0,
        })
    }

    /// A stand-in sd-server next to the library: lists two GPUs and a CPU,
    /// and never answers as a server.
    fn stand_in(&self) -> PathBuf {
        let stand_in = std::path::Path::new(&self.library).with_file_name("slow-sd-server.sh");
        let script = "#!/bin/sh\n\
            if [ \"$1\" = --list-devices ]; then\n\
            printf 'ROCm0\\tAMD Radeon RX 7900 XTX\\nROCm1\\tAMD Radeon RX 7900 XTX\\nCPU\\tAMD Ryzen 9 7950X\\n'\n\
            exit 0\n\
            fi\n\
            exec sleep 600\n";
        let _ = std::fs::write(&stand_in, script);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755));
        }
        stand_in
    }

    fn msgs(&self, name: &str, st: &AppState) -> Vec<Msg> {
        match name {
            "01-library-dialog" => vec![Msg::SetLibraryPath(self.library.clone())],
            "53-library-wizard" => {
                let first = crate::startup::suggested_libraries().into_iter().next().map(|(_, p)| p);
                vec![Msg::LibraryWizard(first.map(|p| (LibraryWizard::Create, p)))]
            }
            // after 07 (a project with a prompt), without servers
            "56-server-status" => vec![Msg::ContinueWithoutServers, Msg::ServerStatus(true)],
            "57-no-server-warning" => vec![Msg::ServerStatus(false), Msg::QueueGenerate],
            "98h-server-logs" => vec![Msg::OpenLogs(None)],
            "98i-server-start-failed" => vec![Msg::OpenPresetNow(0)],
            "58-server-dialog-from-main" => vec![Msg::CloseNoServerWarning, Msg::OpenServerDialog],
            "59-continue-without-servers" => vec![Msg::AskContinueWithoutServers],
            "60-server-settings" => vec![Msg::CancelEdit, Msg::ServerSettings(true)],
            "63-projects-search" => vec![
                Msg::CancelDuplicate,
                Msg::CancelRename,
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Grid),
                Msg::ProjectSearch("fox".into()),
            ],
            "64-prompt-presets" => {
                let mut v = vec![
                    Msg::ProjectSearch(String::new()),
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v.extend([
                    Msg::PresetsPanel(true),
                    Msg::PresetsShow(increart_gui_core::PresetsView::Task(0)),
                    Msg::ApplyPreset { task: 3, preset: 2, insert_at: None },
                ]);
                v
            }
            // the Presets window on one group of the first task (its folder open)
            "98g-presets-group" => {
                let mut groups = st.prompt_presets.tasks[0].presets.iter().map(|p| &p.group).filter(|g| !g.is_empty());
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                    Msg::PresetsSearch(String::new()),
                    Msg::PresetsPanel(true),
                ];
                v.extend(
                    groups.next_back().map(|g| Msg::PresetsShow(increart_gui_core::PresetsView::Group(0, g.clone()))),
                );
                v
            }
            "65-prompt-presets-search" => vec![Msg::PresetsSearch("clay".into())],
            "66-preset-editor" => vec![Msg::OpenPresetEditor(0, Some(25))],
            "67-save-preset" => vec![Msg::ClosePresetEditor, Msg::OpenSavePreset],
            "68-switch-library" => vec![Msg::CancelSavePreset, Msg::PresetsPanel(false), Msg::OtherLibrary(None)],
            "69-style-sweep" => {
                let mut v = vec![Msg::CancelSwitch, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                let mut f = st.generate.clone();
                f.prompt = "cute robot mascot sticker, {preset}".into();
                f.seeds = 4;
                f.label = "r03".into();
                f.sweep.on = true;
                f.sweep.source = increart_core::prompts::SweepSource::Task(increart_core::prompts::LAZY_STYLE.into());
                f.sweep.random = 8;
                f.sweep.random_seed = 7;
                v.push(Msg::Generate(f));
                v
            }
            "91-sweep-tick-all" => {
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Preview(None),
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                let mut f = st.generate.clone();
                f.prompt = "cute robot mascot sticker, {preset}".into();
                f.sweep.on = true;
                v.push(Msg::Generate(f));
                v.push(Msg::StylesOpen);
                v.push(Msg::StylesShow(increart_gui_core::StyleView::Task(st.prompt_presets.tasks[0].name.clone())));
                v.push(Msg::SweepTicks((0..4).map(|p| (0, p)).collect(), true));
                v
            }
            "91c0-sweep-suggesting" | "91c-sweep-suggest" => {
                // PE's values for the first empty shared blanks (the tour
                // waits for the answers)
                let f = &st.generate;
                let chosen = increart_core::prompts::sweep_presets(&st.prompt_presets, &f.sweep);
                let texts = std::iter::once(f.prompt.as_str()).chain(chosen.iter().map(|(_, p)| p.prompt.as_str()));
                let keys: Vec<_> = increart_core::prompts::slot_names(texts)
                    .into_iter()
                    .take(3)
                    .map(|slot| increart_gui_core::BlankKey { style: None, slot })
                    .collect();
                vec![Msg::PeSuggest(keys)]
            }
            "91c1-suggest-dialog" => {
                // Suggest… on the first shared blank: more values, stranger
                // (the tour waits for the answer)
                let f = &st.generate;
                let chosen = increart_core::prompts::sweep_presets(&st.prompt_presets, &f.sweep);
                let texts = std::iter::once(f.prompt.as_str()).chain(chosen.iter().map(|(_, p)| p.prompt.as_str()));
                let Some(slot) = increart_core::prompts::slot_names(texts).into_iter().next() else { return vec![] };
                let key = increart_gui_core::BlankKey { style: None, slot };
                let mut v = vec![Msg::PeSuggestOpen(key.clone())];
                if let Some(server) = st.pe.server.as_ref() {
                    let mut sampling = st.config.pe.sampling(increart_core::pe::FILL_BLANKS);
                    sampling.temperature = 1.3;
                    v.push(Msg::PeSuggestDialog(Some(increart_gui_core::SuggestDialog {
                        key,
                        form: increart_gui_core::SuggestForm {
                            how: "stranger, from nature".into(),
                            count: 8,
                            sampling,
                            images: server.vision(),
                            avoid: true,
                            replace: false,
                        },
                    })));
                    v.push(Msg::PeSuggestAsk);
                }
                v
            }
            "91c1a-batch-suggest" => {
                // Suggest for blanks…: three of them ticked, with instructions
                let f = &st.generate;
                let chosen = increart_core::prompts::sweep_presets(&st.prompt_presets, &f.sweep);
                let texts = std::iter::once(f.prompt.as_str()).chain(chosen.iter().map(|(_, p)| p.prompt.as_str()));
                let names = increart_core::prompts::slot_names(texts);
                let mut v = vec![Msg::PeSuggestDialog(None), Msg::PeBatchSuggestOpen(names.clone())];
                if st.pe.server.is_some() {
                    let mut form = increart_gui_core::SuggestForm::from_ask(
                        &Default::default(),
                        st.config.pe.sampling(increart_core::pe::FILL_BLANKS),
                    );
                    (form.how, form.count) = ("cosy, autumnal".into(), 8);
                    let blanks = names.into_iter().skip(1).take(3).collect();
                    v.push(Msg::PeBatchSuggestDialog(Some(increart_gui_core::BatchSuggest { blanks, form })));
                }
                v
            }
            "91c1b-style-prompts" => {
                // every style's prompt improved, one value picked first; one
                // edited by hand (the tour waits for the answers)
                let mut f = st.generate.clone();
                let first = f.sweep.suggestions.iter().find(|v| v.style.is_none() && !v.shown.is_empty());
                if let Some(v) = first.cloned() {
                    f.sweep.slots.insert(v.slot.clone(), v.shown[0].clone());
                }
                f.sweep.style_prompts.clear();
                let styles = increart_gui_core::update::running_styles(st).into_iter().map(|(k, _)| k).collect();
                let a = increart_core::prompts::EnhanceAsk {
                    how: "moody, cinematic lighting".into(),
                    styles,
                    ..Default::default()
                };
                let mut v = vec![Msg::PeBatchSuggestDialog(None), Msg::Generate(f)];
                // the chips out of the way: the styles' prompts in the shot
                for b in &st.generate.sweep.suggestions {
                    v.push(Msg::PeClearSuggestions(increart_gui_core::BlankKey {
                        style: b.style.clone(),
                        slot: b.slot.clone(),
                    }));
                }
                v.extend([Msg::PeStyleDialog(Some(a)), Msg::PeStyleAsk]);
                v
            }
            "91c1c-enhance-dialog" => {
                // reopened as left: the instructions kept, every style ticked
                vec![Msg::PeStyleOpen]
            }
            "91c1d-jobs-enhancing" => {
                // every style asked again; shot in Jobs once the first is in
                let all = increart_gui_core::update::running_styles(st).into_iter().map(|(k, _)| k).collect();
                vec![Msg::PeStyleDialog(None), Msg::PeStyleEnhance(all), Msg::ProjectsTab(ProjectsTab::Jobs)]
            }
            "91c2-rewrite-writing" => {
                // shot while the answer is written (91d's ask stops it)
                let mut f = st.generate.clone();
                f.sweep.on = false;
                f.prompt = "cute robot mascot sticker waving, like <image1>".into();
                vec![
                    Msg::PeStop(st.pe.jobs.iter().map(|j| j.id).collect()),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                    Msg::PeStyleDialog(None),
                    Msg::PeSuggestDialog(None),
                    Msg::Generate(f),
                    Msg::PeRewrite(increart_gui_core::PeTarget::Prompt),
                ]
            }
            "91d-rewrite-prompt" => {
                // PE's rewrite of a prompt using its reference image (the
                // tour waits for the answer)
                let mut f = st.generate.clone();
                f.sweep.on = false;
                f.prompt = "cute robot mascot sticker waving, like <image1>".into();
                vec![Msg::Generate(f), Msg::PeRewrite(increart_gui_core::PeTarget::Prompt)]
            }
            "91e-describe-image" => {
                let mut v = vec![Msg::PeTextClose(increart_gui_core::PeUse::Discard)];
                if let Some(img) = st.generate.refs().first() {
                    v.push(Msg::PeRewrite(increart_gui_core::PeTarget::Describe(img.clone())));
                }
                v
            }
            "91b1-sweep-random" | "91b2-sweep-random-again" => {
                // a random 6 of every Effortful Modify style; again with
                // another pick
                let mut f = st.generate.clone();
                f.sweep.random = 6;
                f.sweep.random_seed = if name == "91b1-sweep-random" { 1 } else { 99 };
                // the form first: the ticks change it after
                let mut v = vec![Msg::Generate(f)];
                if let Some(t) = st.prompt_presets.tasks.iter().position(|t| t.name.starts_with("Effortful Modify")) {
                    let n = st.prompt_presets.tasks[t].presets.len();
                    v.push(Msg::SweepTicks((0..n).map(|p| (t, p)).collect(), true));
                }
                v
            }
            "91b-sweep-blanks" => {
                // a few Effortful styles ticked, each with its blanks
                let mut v = vec![Msg::StylesClose(true)];
                // one of the project's images as the reference they need
                if let Some(o) = st.project_entry().and_then(|p| p.outputs.first()) {
                    v.push(Msg::AddRefs(vec![o.image.clone()]));
                }
                let effortful = st.prompt_presets.tasks.iter().position(|t| t.name.starts_with("Effortful"));
                if let Some(t) = effortful {
                    let p = st.prompt_presets.tasks[t].presets.len().min(12);
                    v.push(Msg::SweepClearTicks);
                    v.push(Msg::SweepTicks((0..p).map(|p| (t, p)).collect(), true));
                }
                v
            }
            "70-contact-sheet"
            | "92-check-sheet-arrows"
            | "92b-check-sheet-arrows-folded"
            | "93-check-sheet-arrows-click"
            | "94-check-sheet-arrows-slider" => {
                // the checks' positions are those of one row per line at the
                // default size (92b: Auto, the rows fold, two side by side)
                let rows = if name == "92b-check-sheet-arrows-folded" { SheetRows::Auto } else { SheetRows::Full };
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::Review),
                    Msg::SheetRows(rows),
                    Msg::SheetZoom { size: increart_gui_core::SHEET_CELL_DEFAULT, save: false },
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::ReviewSweep(p.project.id.clone(), "r02".into()));
                    v.push(Msg::SheetFilter(String::new()));
                    v.push(Msg::SheetCell(0, 2));
                }
                v
            }
            "98c-output-sources" => vec![
                Msg::ContinueWithoutServers,
                Msg::Tab(Tab::OutputLibrary),
                Msg::OutputSearchProjects(false),
                Msg::OutputSource(OutputSource::AllProjects),
            ],
            "98d-output-project-search" => vec![
                Msg::ContinueWithoutServers,
                Msg::Tab(Tab::OutputLibrary),
                Msg::OutputSearchProjects(true),
                Msg::OutputProjectSearch("fox".into()),
            ],
            // fox-cafe's outputs shown, so "by project" starts open (the
            // check then closes it before Show outputs)
            "98e-check-show-outputs-reveal" => {
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::OutputLibrary), Msg::OutputSearchProjects(false)];
                v.extend(fox_cafe(st).map(OutputSource::Project).map(Msg::OutputSource));
                v
            }
            // Show outputs from Projects with a project-name search that hides it
            "98f-show-outputs-reveal" => {
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::OutputLibrary),
                    Msg::OutputSource(OutputSource::AllProjects),
                    Msg::OutputSearchProjects(true),
                    Msg::OutputProjectSearch("zzz".into()),
                    Msg::Tab(Tab::Projects),
                ];
                v.extend(fox_cafe(st).map(OutputSource::Project).map(Msg::ShowOutputs));
                v
            }
            "98-setting-sweep" | "98b-model-sweep" => {
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Preview(None),
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                let mut f = st.generate.clone();
                f.prompt = "cute robot mascot sticker".into();
                f.seeds = 3;
                f.sweep.on = true;
                if name == "98-setting-sweep" {
                    f.sweep.vary = increart_core::prompts::SweepVary::Cfg;
                    f.cfg = 2.0;
                    f.sweep.values = increart_gui_core::update::sweep_defaults(f.sweep.vary, &f);
                } else {
                    f.sweep.vary = increart_core::prompts::SweepVary::Model;
                    f.sweep.models = st.config.recipes.iter().take(2).map(|r| r.name.clone()).collect();
                }
                v.push(Msg::Generate(f));
                v
            }
            "95-check-sheet-preview-arrows" => {
                let mut v = vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Review), Msg::SheetRows(SheetRows::Full)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::ReviewSweep(p.project.id.clone(), "r02".into()));
                    v.push(Msg::SheetFilter(String::new()));
                    v.push(Msg::SheetCell(0, 0));
                }
                v
            }
            "96-review-picked"
            | "96b-picked-menu"
            | "96c-sheet-band"
            | "97-tile-window"
            | "97b-check-tile-drag"
            | "97c-tile-menu"
            | "97d-tile-cell-menu"
            | "97e-tile-text-title"
            | "97h-tile-picker" => {
                // the drags' positions are those of one row per line at the
                // default size
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::Review),
                    Msg::SheetRows(SheetRows::Full),
                    Msg::SheetZoom { size: increart_gui_core::SHEET_CELL_DEFAULT, save: false },
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::ReviewSweep(p.project.id.clone(), "r02".into()));
                    let in_sweep = |o: &&increart_core::scan::OutputLink| {
                        st.index.run(&o.name.run).is_some_and(|(_, r)| r.sweep.is_some())
                    };
                    let mut picked: Vec<_> = p.outputs.iter().filter(in_sweep).map(|o| o.image.clone()).collect();
                    picked.sort_by(|a, b| a.id.cmp(&b.id));
                    picked.truncate(6);
                    if name != "96c-sheet-band" {
                        v.push(Msg::PickSet(picked));
                    }
                    if name.starts_with("97") {
                        v.push(Msg::OpenTile);
                        v.push(Msg::TileCaptions(increart_gui_core::CaptionFill::Style));
                        v.push(Msg::TileSelect(Some(0)));
                    }
                    if name == "97e-tile-text-title" {
                        v.push(Msg::TileAddText(0, 2));
                        v.push(Msg::TileText(6, "Pixel art and Watercolour\nkept for r03".into()));
                        v.push(Msg::TileTitle("Sticker styles".into(), "sweep r02 · seeds 0–2".into()));
                        v.push(Msg::TileCaptionFormat("#{n} {style} · seed {seed}".into()));
                    }
                    if name == "97h-tile-picker" {
                        v.push(Msg::TilePicker(Some((0, 2))));
                    }
                }
                v
            }
            "97f-project-runs" => {
                let mut v = vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectRuns(true)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v
            }
            "97g-output-by-project-runs" => {
                let mut v = vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::OutputLibrary), Msg::OutputSelect(None)];
                let stickers = st.index.projects.iter().find(|p| p.name() == "stickers");
                if let Some((r, _)) = stickers.and_then(|p| st.kept_runs(p).into_iter().next()) {
                    v.push(Msg::OutputSource(OutputSource::Run(r.id.clone())));
                }
                v
            }
            "97h-input-library-panel" => {
                let first = st.library_ideas().iter().find(|e| !e.idea.images.is_empty()).map(|e| e.idea.id.clone());
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::InputLibrary),
                    Msg::LibTab(increart_gui_core::LibTab::Grid),
                ];
                if let Some(id) = first {
                    v.push(Msg::LibSelect { id: id.clone(), additive: false });
                    v.push(Msg::LibInfo(Some(id)));
                }
                v
            }
            "98j-check-library-card-refs" => {
                let first = st.library_ideas().iter().find(|e| !e.idea.images.is_empty()).map(|e| e.idea.id.clone());
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::InputLibrary),
                    Msg::LibTab(increart_gui_core::LibTab::Grid),
                    Msg::LibFolder(None),
                ];
                if let Some(id) = first {
                    v.push(Msg::LibSelect { id, additive: false });
                }
                v
            }
            "98k-check-inbox-card-keep" => {
                vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::IdeaInbox), Msg::InboxTab(InboxTab::Grid)]
            }
            "97i-check-output-double-click" => vec![
                Msg::ContinueWithoutServers,
                Msg::Tab(Tab::OutputLibrary),
                Msg::OutputSource(OutputSource::AllProjects),
                Msg::OutputTab(OutputTab::Grid),
                Msg::OutputSelect(None),
            ],
            "97j-check-library-click" => {
                vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::InputLibrary),
                    Msg::LibTab(increart_gui_core::LibTab::Grid),
                    Msg::LibInfo(None),
                ]
            }
            "79-contact-sheet-filter-zoom" => {
                let mut v = vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Review), Msg::SheetRows(SheetRows::Full)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::ReviewSweep(p.project.id.clone(), "r02".into()));
                    v.push(Msg::SheetFilter("o".into()));
                    v.push(Msg::SheetZoom { size: 200, save: false });
                }
                v
            }
            "74-notification-settings" => {
                let mut n = st.config.notifications.clone();
                n.gpu_idle.on = true;
                n.review_backlog.on = true;
                n.review_backlog.sound = increart_core::notify::Sound::File("/home/me/sounds/ding.ogg".into());
                n.player = "mpv --really-quiet --volume={volume_percent} {file}".into();
                vec![Msg::ContinueWithoutServers, Msg::SetNotifications(n), Msg::Settings(true)]
            }
            "74b-timeout-settings" => vec![Msg::SettingsTab(increart_gui_core::state::SettingsTab::Timeouts)],
            // the review backlog over its threshold: the bell filled, its menu open
            "74c-bell-ringing" => {
                let mut n = st.config.notifications.clone();
                n.review_backlog.on = true;
                n.review_backlog.threshold = 1;
                n.show_message = true;
                vec![Msg::ContinueWithoutServers, Msg::Settings(false), Msg::SetNotifications(n)]
            }
            "75-promote-in-folder" => {
                let idea = st.index.ideas.iter().find(|e| matches!(e.loc, increart_core::IdeaLoc::Library(_)));
                let mut v = vec![Msg::ContinueWithoutServers, Msg::Settings(false), Msg::Tab(Tab::InputLibrary)];
                if let Some(e) = idea {
                    v.push(Msg::Promote(vec![e.idea.id.clone()]));
                    // step 2: in clients, a new folder
                    v.push(Msg::PromoteChoose(true));
                    v.push(Msg::PromoteParent("clients".into()));
                    v.push(Msg::PromoteNewFolder(Some("posters".into())));
                }
                v
            }
            "76-top-bar-wide" => vec![Msg::ContinueWithoutServers, Msg::CancelPromote, Msg::Tab(Tab::Review)],
            "71-job-info" => {
                // glass-fish's edit run pretends to generate: a reference and a start image
                let run = st.index.projects.iter().find(|p| p.name() == "glass-fish").and_then(|p| p.runs.first());
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Jobs)];
                if let Some(r) = run {
                    v.push(Msg::RunsQueued(vec![r.id.clone()]));
                    v.push(Msg::Engine(increart_core::engine::Event::Progress {
                        run: r.id.clone(),
                        rev: 0,
                        seed: 4,
                        server: 0,
                        step: 9,
                        total: 20,
                        preview: None,
                    }));
                    v.push(Msg::JobInfo(Some(r.id.clone())));
                }
                v
            }
            "80-stop-buttons" | "81-stop-finishing" => {
                // two runs pretend to generate; the second scene finishes one
                let runs: Vec<_> = st.index.projects.iter().flat_map(|p| p.runs.first()).take(2).collect();
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Jobs)];
                v.push(Msg::RunsQueued(runs.iter().map(|r| r.id.clone()).collect()));
                for r in &runs {
                    v.push(Msg::Engine(increart_core::engine::Event::Progress {
                        run: r.id.clone(),
                        rev: 0,
                        seed: r.all_seeds().next().map_or(0, |(_, s)| s),
                        server: 0,
                        step: 9,
                        total: 20,
                        preview: None,
                    }));
                }
                if name == "81-stop-finishing"
                    && let Some(r) = runs.first()
                {
                    v.push(Msg::CancelRun(r.id.clone(), increart_core::engine::Stop::Finish));
                    v.push(Msg::JobInfo(Some(r.id.clone())));
                }
                v
            }
            "72-review-sweep-button" => vec![
                Msg::JobInfo(None),
                Msg::Tab(Tab::Review),
                Msg::ReviewScope(None),
                Msg::ReviewView(ReviewView::Single),
                Msg::ReviewNewest,
            ],
            "73-promote" => {
                let ideas: Vec<_> = st.library_ideas().iter().take(2).map(|e| e.idea.id.clone()).collect();
                vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::IdeaInbox), Msg::Promote(ideas)]
            }
            "62-rename" => vec![Msg::Tab(Tab::Projects), Msg::StartRename(Tree::Projects, PathBuf::from("stickers"))],
            "61-duplicate-project" => vec![Msg::Tab(Tab::Projects), Msg::DuplicateProject(PathBuf::from("stickers"))],
            "55-library-open-wizard" => {
                vec![Msg::LibraryWizard(Some((LibraryWizard::Open, self.library.clone())))]
            }
            "02-server-dialog" => {
                vec![Msg::LibraryWizard(None), Msg::SetLibraryPath(self.library.clone()), Msg::OpenLibrary]
            }
            "02b-pe-settings" => {
                // Settings… on the first preset's chat server
                let at = st.config.server_presets.iter().enumerate().find_map(|(i, p)| {
                    p.servers.iter().position(increart_core::config::ServerConfig::is_chat).map(|j| (i, j))
                });
                match at {
                    Some((i, j)) => vec![Msg::EditPreset(i), Msg::PeEditorOpen(Some(j))],
                    None => vec![],
                }
            }
            "02c-koboldcpp-entry" => {
                // a KoboldCpp entry in the first preset, as the chat server too
                use increart_core::kobold::{KoboldChat, KoboldRun, KoboldServer};
                let first = st.config.server_presets.first().map(|p| p.servers.len()).unwrap_or(0);
                let run = KoboldRun::Start {
                    binary: increart_gui_core::kobold_binary(st),
                    config: String::new(),
                    host: "127.0.0.1".into(),
                    port: 5001,
                    device: Default::default(),
                    args: vec![],
                };
                let chat = KoboldChat {
                    model: "/models/koboldcpp/Qwen3-VL-8B-Instruct-Q8_0.gguf".into(),
                    mmproj: "/models/koboldcpp/mmproj-Qwen3-VL-8B-Instruct-F16.gguf".into(),
                    context: 8192,
                };
                let k = KoboldServer { name: format!("kcpp{first}"), run, chat: Some(chat), swap: false };
                vec![
                    Msg::PeEditorClose,
                    Msg::CancelEdit,
                    Msg::EditPreset(0),
                    Msg::AddKoboldServer,
                    Msg::EditServer(first, increart_core::config::ServerConfig::Kobold(k)),
                ]
            }
            // the preset editor's Image model gear
            "02d-model-presets-from-servers" => vec![Msg::OpenRecipes],
            "54-server-wizard" => {
                // KoboldCpp: two 24 GB cards and an integrated GPU (not ticked)
                use increart_core::tiers::Gpu;
                let gpu =
                    |number, name: &str, vram_gib, integrated| Gpu { number, name: name.into(), vram_gib, integrated };
                let gpus = vec![
                    gpu(0, "AMD Radeon Graphics (RADV RAPHAEL_MENDOCINO)", 15.6, true),
                    gpu(1, "AMD Radeon RX 7900 XTX (RADV NAVI31)", 23.98, false),
                    gpu(2, "AMD Radeon RX 7900 XTX (RADV NAVI31)", 23.98, false),
                ];
                vec![
                    Msg::CloseRecipes,
                    Msg::PeEditorClose,
                    Msg::CancelEdit,
                    Msg::ServerWizard(true),
                    Msg::WizardGpus(Ok(gpus), Some(60.0)),
                ]
            }
            "54a-server-wizard-choose" => {
                // presets chosen: the first card's bigger than its memory takes
                let Phase::Servers(ServerDialog { kobold: Some(k), .. }) = &st.phase else { return vec![] };
                let mut k = (**k).clone();
                k.choose = true;
                if let Some(Ok(list)) = &mut k.gpus {
                    list[1].preset = Some(32);
                }
                vec![Msg::EditKoboldWizard(k)]
            }
            "54c-server-wizard-pe-own-gpu" => {
                // prompt enhance on a card of its own, the other making images
                let Phase::Servers(ServerDialog { kobold: Some(k), .. }) = &st.phase else { return vec![] };
                let mut k = (**k).clone();
                k.choose = false;
                if let Some(Ok(list)) = &mut k.gpus {
                    list.iter_mut().for_each(|w| w.preset = None);
                }
                k.layout = increart_core::tiers::Layout::PeOwnCard;
                vec![Msg::EditKoboldWizard(k)]
            }
            "54d-server-wizard-pe-on-one" => {
                // images on both again, prompt enhance unticked on GPU 2 (by hand)
                let Phase::Servers(ServerDialog { kobold: Some(k), .. }) = &st.phase else { return vec![] };
                let mut k = (**k).clone();
                k.layout = increart_core::tiers::Layout::ImagePerCard;
                let mut first = k.clone();
                first.pe_gpus = vec![1];
                vec![Msg::EditKoboldWizard(k), Msg::EditKoboldWizard(first)]
            }
            "54b-server-wizard-sd-server" => {
                let mut c = st.config.sdserver.clone();
                c.binary = self.stand_in().to_string_lossy().into();
                vec![Msg::SetSdServer(c), Msg::WizardKobold(false)]
            }
            "03-idea-inbox-grid" => {
                // the presets' first chat server, for the prompt-enhance scenes
                let chat = st.config.chat_servers().first().map(|s| (*s).clone());
                vec![
                    Msg::PeEditorClose,
                    Msg::CancelEdit,
                    Msg::WithoutDialog(Some(increart_gui_core::Without { chat })),
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::IdeaInbox),
                    Msg::InboxTab(InboxTab::Grid),
                ]
            }
            "04-idea-inbox-new-idea" => vec![
                Msg::InboxTab(InboxTab::NewIdea),
                Msg::Draft(IdeaDraft {
                    title: "Otter tea party".into(),
                    text: "an otter floating on its back holding a teacup, watercolour".into(),
                    tags: "animals, watercolour".into(),
                    images: vec![],
                }),
            ],
            "05-input-library" => vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::InputLibrary), Msg::LibFolder(None)],
            "06-projects-grid" => vec![
                Msg::ContinueWithoutServers,
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Grid),
                Msg::SelectProjectFolder(Some(PathBuf::new())),
            ],
            // the details pane: the project with the most runs, then one of its runs
            "06b-projects-pane" => {
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Grid)];
                if let Some(p) = st.index.projects.iter().max_by_key(|p| p.runs.len()) {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v
            }
            "06c-projects-pane-run" => {
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Folder)];
                if let Some(p) = st.index.projects.iter().max_by_key(|p| p.runs.len())
                    && let Some(r) = st.latest_run(p)
                {
                    v.push(Msg::FvGo(p.dir.join("runs").join(r.id.as_str())));
                }
                v
            }
            "87-messages-popover" => vec![
                Msg::ContinueWithoutServers,
                Msg::Info("Copied 1 image".into()),
                Msg::Problem("Notification sound: no audio device".into()),
                Msg::Info("All GPUs idle for 5 min".into()),
            ],
            "86-image-title" => {
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::OutputLibrary),
                    Msg::OutputTab(increart_gui_core::OutputTab::Grid),
                ];
                if let Some(o) = st.kept_outputs().first() {
                    v.push(Msg::OutputSelect(Some(o.image.clone())));
                    v.push(Msg::TitleDialog(Some(o.image.clone())));
                    v.push(Msg::EditTitle("Harbour at dusk".into()));
                }
                v
            }
            // after 07: its refs are added, two more chosen from the outputs
            "85-ref-picker" => {
                let mut v = vec![
                    Msg::ContinueWithoutServers,
                    Msg::RefPicker(true),
                    Msg::RefPickerSource(increart_gui_core::RefSource::Outputs(
                        increart_gui_core::OutputSource::AllProjects,
                    )),
                ];
                let kept: Vec<ImageRef> = st.kept_outputs().iter().map(|o| o.image.clone()).collect();
                v.extend(kept.into_iter().skip(1).step_by(3).take(2).map(Msg::RefPickerToggle));
                v
            }
            "07-projects-generate" => {
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers").or(st.index.projects.first())
                {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                // One whole form (a `Generate` message replaces the form, so it
                // carries the refs and label too).
                let mut f = st.generate.clone();
                let refs: Vec<ImageRef> = st
                    .library_ideas()
                    .iter()
                    .flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input))
                    .take(2)
                    .collect();
                f.images = increart_gui_core::images_of(&refs, None);
                f.label = "r02".into();
                f.prompt = "the robot from <image1> holding the hut from <image2>, sticker style".into();
                v.push(Msg::Generate(f));
                v
            }
            "07b-generate-blanks" => {
                // a single run's prompt with blanks: the blanks table under it
                let mut f = st.generate.clone();
                f.sweep.on = false;
                f.prompt = "a {creature} sticker of <image1>, {colour} outline".into();
                f.sweep.slots.insert("creature".into(), "dragon".into());
                f.sweep.slots.remove("colour");
                vec![Msg::Generate(f)]
            }
            // too zoomed for the tab names: icons and counts
            "88-tabs-zoomed" => vec![Msg::ContinueWithoutServers, Msg::UiZoom { percent: 170, save: false }],
            "89-review-preview" | "90-preview-keys" => {
                let img = st.review_queue().first().map(|o| o.image.clone());
                vec![
                    Msg::ContinueWithoutServers,
                    Msg::UiZoom { percent: 100, save: false },
                    Msg::Tab(Tab::Review),
                    Msg::ReviewScope(None),
                    Msg::Preview(img),
                ]
            }
            "99-quit-with-unsaved-draft" => {
                let mut v = vec![
                    Msg::UiZoom { percent: 100, save: false },
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Generate),
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "glass-fish") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v.push(Msg::Generate(increart_gui_core::GenerateForm {
                    prompt: "edited right before quitting".into(),
                    label: "r02".into(),
                    ..st.generate.clone()
                }));
                v
            }
            "08-projects-jobs" => vec![Msg::ProjectsTab(ProjectsTab::Jobs)],
            "09-review" => vec![Msg::Tab(Tab::Review), Msg::ReviewScope(None)],
            "10-output-library" => vec![Msg::Tab(Tab::OutputLibrary)],
            "12-review-grid" => vec![Msg::RunsQueued(vec![]), Msg::Tab(Tab::Review), Msg::ReviewView(ReviewView::Grid)],
            "13-review-compare" => vec![Msg::ReviewView(ReviewView::Single), Msg::ReviewCursor(0), Msg::Compare],
            "13c-review-compare-with" => {
                let b = tour_with_refs(st);
                vec![Msg::Tab(Tab::Review), Msg::ReviewView(ReviewView::Single), Msg::CompareWith(b)]
            }
            "13d-review-compare-input" => {
                let b = tour_with_refs(st);
                let mut v = vec![Msg::Tab(Tab::Review), Msg::ReviewView(ReviewView::Single), Msg::CompareWith(b)];
                if let Some(img) = tour_input(st) {
                    v.push(Msg::RefPickerToggle(img));
                }
                v
            }
            "14b-output-library-compare" => {
                let mut v = vec![Msg::Tab(Tab::OutputLibrary), Msg::OutputTab(OutputTab::Grid)];
                let shown = st.output_source_images();
                if let Some(o) = shown.get(1).or(shown.first()) {
                    v.push(Msg::CompareWith(Some(o.image.clone())));
                    if let Some(img) = tour_input(st) {
                        v.push(Msg::RefPickerToggle(img));
                    }
                }
                v
            }
            "13b-review-follow" => {
                vec![Msg::ReviewView(ReviewView::Single), Msg::ReviewCursor(0), Msg::ReviewFollow(true)]
            }
            "17-prompt-history" => {
                // A few timed edits, so the history has entries (Tick sets the clock).
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                let fox = st.index.projects.iter().find(|p| p.name() == "fox-cafe").map(|p| p.project.id.clone());
                if let Some(id) = fox.clone() {
                    v.push(Msg::SelectProject(id));
                }
                let mut f = st.generate.clone();
                let mut t = 10_000.0;
                for text in [
                    "a red fox reading a newspaper",
                    "a red fox reading a newspaper in a sunlit cafe",
                    "a red fox reading a newspaper in a sunlit cafe, film photograph, 35mm",
                    "a red fox in a tweed jacket reading a newspaper in a sunlit cafe, film photograph, 35mm",
                ] {
                    v.push(Msg::Tick(t));
                    f.prompt = text.into();
                    if text.ends_with("35mm") {
                        f.steps += 10; // a settings change shows in the history too
                    }
                    v.push(Msg::Generate(f.clone()));
                    t += 120.0;
                    v.push(Msg::Tick(t));
                }
                if let Some(id) = fox {
                    v.push(Msg::OpenHistory(increart_core::HistoryKey::Project(id)));
                }
                v.push(Msg::HistorySelect(2));
                v
            }
            "18-edit-idea" => {
                let mut v = vec![Msg::Tab(Tab::IdeaInbox), Msg::InboxTab(InboxTab::Grid)];
                if let Some(e) = st.unreviewed_ideas().first() {
                    v.push(Msg::OpenIdea(e.idea.id.clone()));
                }
                v
            }
            "19-idea-inbox-import" => vec![Msg::Tab(Tab::IdeaInbox), Msg::InboxTab(InboxTab::Import)],
            "20-input-library-filters" => vec![
                Msg::Tab(Tab::InputLibrary),
                Msg::LibTab(increart_gui_core::LibTab::Grid),
                Msg::LibFilter(increart_gui_core::LibFilter { starred_only: true, ..Default::default() }),
            ],
            "21-resolutions" => vec![
                Msg::LibFilter(Default::default()),
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Generate),
                Msg::OpenResolutions,
            ],
            "21b-model-presets" => {
                vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate), Msg::OpenRecipes]
            }
            "22-output-library-lineage" => {
                let mut v = vec![
                    Msg::Tab(Tab::OutputLibrary),
                    Msg::OutputTab(OutputTab::Grid),
                    Msg::OutputSource(increart_gui_core::OutputSource::AllProjects),
                ];
                // an edited image (glass-fish), so the lineage has a parent and refs
                let img = st
                    .index
                    .projects
                    .iter()
                    .find(|p| p.name() == "glass-fish")
                    .and_then(|p| p.outputs.first())
                    .map(|o| o.image.clone());
                v.push(Msg::OutputSelect(img));
                v
            }
            "24-projects-grid-tile-actions" | "25-check-grid-click" => {
                let mut v = vec![
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Grid),
                    Msg::SelectProjectFolder(Some(PathBuf::new())),
                ];
                // (the tour library has no stickers: its first project)
                let projects = &st.index.projects;
                if let Some(p) = projects.iter().find(|p| p.name() == "stickers").or(projects.first()) {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v
            }
            "26-projects-folder" | "84-projects-folder-menu" => vec![
                Msg::ContinueWithoutServers,
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Folder),
                Msg::FvMode(increart_gui_core::FvMode::Icons),
                Msg::FvGo(PathBuf::from("projects")),
            ],
            "27-projects-folder-images" | "28-projects-folder-details" => {
                let mode = if name == "27-projects-folder-images" {
                    increart_gui_core::FvMode::Icons
                } else {
                    increart_gui_core::FvMode::Details
                };
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Folder), Msg::FvMode(mode)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    let dir = p.dir.join("unreviewed");
                    v.push(Msg::FvGo(if mode == increart_gui_core::FvMode::Icons { dir } else { p.dir.clone() }));
                }
                v
            }
            "29-projects-folder-project" | "30-projects-folder-runs" => {
                let mut v = vec![
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Folder),
                    Msg::FvMode(increart_gui_core::FvMode::Icons),
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "fox-cafe") {
                    v.push(Msg::FvGo(if name == "30-projects-folder-runs" {
                        p.dir.join("runs")
                    } else {
                        p.dir.clone()
                    }));
                }
                v
            }
            // a new folder, then type into its rename field (checks below)
            "31-check-rename-tree-grid" => {
                vec![
                    Msg::Tab(Tab::Projects),
                    Msg::ProjectsTab(ProjectsTab::Grid),
                    Msg::NewFolder(Tree::Projects, PathBuf::new()),
                ]
            }
            "32-check-rename-tree-folder" => vec![
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Folder),
                Msg::FvGo(PathBuf::from("projects")),
                Msg::NewFolder(Tree::Projects, PathBuf::new()),
            ],
            "33-check-rename-folder-view" => vec![
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Folder),
                Msg::FvGo(PathBuf::from("projects")),
                Msg::FvNewFolder,
            ],
            // the Jobs tab: no grid tiles, so the tree's entry is the drag source
            "34-check-drag-tree" => vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Jobs)],
            "37-check-width-rounding" => {
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.first() {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v
            }
            "36-check-grid-hover" => vec![
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Grid),
                Msg::SelectProjectFolder(Some(PathBuf::new())),
            ],
            "35-check-drag-folder-view" => vec![
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Folder),
                Msg::FvMode(increart_gui_core::FvMode::Icons),
                Msg::FvGo(PathBuf::from("projects")),
            ],
            "39-output-library-folder" | "40-output-library-smart-folder" => {
                let path =
                    if name == "39-output-library-folder" { "collections" } else { "collections/smart/by project" };
                vec![
                    Msg::ContinueWithoutServers,
                    Msg::Tab(Tab::OutputLibrary),
                    Msg::OutputTab(increart_gui_core::OutputTab::Folder),
                    Msg::FvMode(increart_gui_core::FvMode::Icons),
                    Msg::FvGo(PathBuf::from(path)),
                ]
            }
            "42-input-library-folder" => vec![
                Msg::Tab(Tab::InputLibrary),
                Msg::LibTab(increart_gui_core::LibTab::Folder),
                Msg::FvMode(increart_gui_core::FvMode::Icons),
                Msg::FvGo(PathBuf::from("input/library")),
            ],
            "43-idea-inbox-folder" => vec![
                Msg::Tab(Tab::IdeaInbox),
                Msg::InboxTab(InboxTab::Folder),
                Msg::FvMode(increart_gui_core::FvMode::Icons),
                Msg::FvGo(PathBuf::from("input/unreviewed")),
            ],
            "44-projects-folder-compact" | "41-check-rubber-band" => {
                let mode = if name == "41-check-rubber-band" {
                    increart_gui_core::FvMode::Icons
                } else {
                    increart_gui_core::FvMode::Compact
                };
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Folder), Msg::FvMode(mode)];
                v.push(Msg::FvGo(PathBuf::from(if name == "41-check-rubber-band" {
                    "projects"
                } else {
                    "projects/stickers/reviewed/good"
                })));
                v.push(Msg::FvClearSelection);
                v
            }
            // Show outputs on the project with the most kept images, a filter on
            "49-output-library-show-outputs" => {
                let p = st.index.projects.iter().max_by_key(|p| counts(p)[0] + counts(p)[1]);
                let f = increart_gui_core::OutputFilter { has_refs: true, ..Default::default() };
                let mut v = vec![Msg::OutputSelect(None), Msg::OutputFilter(f)];
                v.extend(p.map(|p| Msg::ShowOutputs(OutputSource::Project(p.project.id.clone()))));
                v
            }
            "50-output-library-show-outputs-empty" => {
                let p = st.index.projects.iter().find(|p| counts(p)[0] + counts(p)[1] == 0);
                vec![Msg::ShowOutputs(match p {
                    Some(p) => OutputSource::Project(p.project.id.clone()),
                    None => OutputSource::ProjectFolder("no such folder".into()),
                })]
            }
            // after 50: its "Show all, bad included"
            "51-output-library-show-outputs-any-rating" => {
                vec![Msg::OutputPreset(increart_gui_core::OutputPreset::All)]
            }
            // closes a dialog still open from scene 16 (when run right after it) first
            "52-server-dialog-edit" => {
                vec![Msg::CancelOpening, Msg::ContinueWithoutServers, Msg::OpenServerDialog, Msg::EditPreset(0)]
            }
            "48-check-copy-image" | "82-preview-zoom" | "83-preview-next" => {
                let img = st.kept_outputs().first().map(|o| o.image.clone());
                vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::OutputLibrary), Msg::Preview(img)]
            }
            "45-reference-preview" | "46-check-ref-double-click" => {
                let mut v =
                    vec![Msg::ContinueWithoutServers, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                // two library images as references (as in scene 07)
                let mut f = st.generate.clone();
                let refs: Vec<ImageRef> = st
                    .library_ideas()
                    .iter()
                    .flat_map(|e| e.idea.images.iter().cloned().map(ImageRef::input))
                    .take(2)
                    .collect();
                f.images = increart_gui_core::images_of(&refs, None);
                if name == "45-reference-preview" {
                    v.push(Msg::Preview(f.refs().first().cloned()));
                }
                v.push(Msg::Generate(f));
                v
            }
            "47-new-project-name" => {
                let img = st.kept_outputs().first().map(|o| o.image.clone());
                let mut v = vec![Msg::Tab(Tab::Review)];
                v.extend(img.map(|i| Msg::UseImage(i, increart_gui_core::UseTarget::DuplicateProject)));
                v
            }
            "38-scale-dialog" => {
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "stickers") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v.push(Msg::OpenScale(increart_gui_core::ScaleTarget::Generate));
                v
            }
            "23-export" => {
                let kept: Vec<_> = st.kept_outputs().iter().map(|o| o.image.clone()).collect();
                vec![Msg::OpenExport(kept, "all kept images".into())]
            }
            "16-servers-opening" => {
                // A Start server whose binary never answers: stays "starting".
                let mut c = st.config.sdserver.clone();
                c.binary = self.stand_in().to_string_lossy().into();
                vec![Msg::OpenServerDialog, Msg::SetSdServer(c), Msg::OpenPreset(0)]
            }
            "15-generate-img2img" => {
                // an output added to glass-fish, then made the start image
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "glass-fish") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                    if let Some(o) = p.outputs.first() {
                        // a clean form, so it's the first image
                        v.push(Msg::ClearGenerate);
                        v.push(Msg::UseImage(o.image.clone(), increart_gui_core::UseTarget::CurrentProject));
                        v.push(Msg::ImageRoles { index: 0, reference: true, start: true });
                    }
                }
                v
            }
            "15b-crop-mark" => {
                // an output in glass-fish's Generate, cropped with a circle and a scribble
                let o = st.index.projects.iter().find(|p| p.name() == "glass-fish").and_then(|p| p.outputs.first());
                crop_mark(st, "glass-fish", o)
            }
            "14-output-library-rejected" => {
                vec![Msg::Tab(Tab::OutputLibrary), Msg::OutputPreset(increart_gui_core::OutputPreset::Rejected)]
            }
            "11-jobs-popover" => {
                // Pretend the engine is working on the fox runs: two seeds part-sampled.
                let fox = st.index.projects.iter().find(|p| p.name() == "fox-cafe").or(st.index.projects.first());
                let runs: Vec<_> = fox.map(|p| p.runs.iter().map(|r| r.id.clone()).collect()).unwrap_or_default();
                let mut v = vec![Msg::Tab(Tab::Review), Msg::RunsQueued(runs.clone())];
                if let Some(run) = runs.last() {
                    for (seed, step, server) in [(6, 14, 0), (7, 5, 1)] {
                        v.push(Msg::Engine(increart_core::engine::Event::Progress {
                            run: run.clone(),
                            rev: 0,
                            seed,
                            server,
                            step,
                            total: 20,
                            preview: None,
                        }));
                    }
                }
                v
            }
            "R1-review-grid" => vec![
                Msg::ContinueWithoutServers,
                // the default tile sizes, whatever the config has
                Msg::GridZoom { grid: ZoomGrid::Review, size: ZoomGrid::Review.default_size(), save: false },
                Msg::GridZoom { grid: ZoomGrid::Output, size: ZoomGrid::Output.default_size(), save: false },
                Msg::Tab(Tab::Review),
                Msg::ReviewScope(None),
                Msg::ReviewView(ReviewView::Grid),
            ],
            "R2-review-compare" => vec![Msg::ReviewView(ReviewView::Single), Msg::ReviewCursor(0), Msg::Compare],
            "R3-generate" => {
                let mut v = vec![Msg::Compare, Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                // its draft: a style sweep with prompt enhance's suggestions
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "great-wave-cat") {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                }
                v
            }
            "R4-output-lineage" => {
                let mut v = vec![
                    Msg::Tab(Tab::OutputLibrary),
                    Msg::OutputTab(OutputTab::Grid),
                    Msg::OutputSource(OutputSource::AllProjects),
                ];
                let img = st
                    .index
                    .projects
                    .iter()
                    .find(|p| p.name() == "great-wave-cat")
                    .and_then(|p| p.outputs.iter().find(|o| o.verdict == Some(increart_core::Verdict::Good)))
                    .map(|o| o.image.clone());
                v.push(Msg::OutputSelect(img));
                v
            }
            "R5-input-library" => vec![Msg::Tab(Tab::InputLibrary), Msg::LibFolder(None)],
            "R6-projects-grid" => vec![
                Msg::Tab(Tab::Projects),
                Msg::ProjectsTab(ProjectsTab::Grid),
                Msg::SelectProjectFolder(Some(PathBuf::new())),
            ],
            "G01-server-wizard" => self.msgs("54-server-wizard", st),
            "G02-idea-inbox" => {
                let mut v = vec![Msg::ContinueWithoutServers];
                // the default card and tile sizes, whatever the config has
                for grid in [ZoomGrid::Review, ZoomGrid::Output, ZoomGrid::Library, ZoomGrid::Inbox] {
                    v.push(Msg::GridZoom { grid, size: grid.default_size(), save: false });
                }
                v.extend([Msg::Tab(Tab::IdeaInbox), Msg::InboxTab(InboxTab::Grid)]);
                v
            }
            "G03-projects-grid" => self.msgs("R6-projects-grid", st),
            "G04-generate-refs" | "G05-check-select-mika" | "G07-prompt-history" => {
                let project = if name == "G05-check-select-mika" { "mika-the-courier" } else { "olympus-mons" };
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == project) {
                    v.push(Msg::SelectProject(p.project.id.clone()));
                    if name == "G07-prompt-history" {
                        // step() selects its prompt before prompt enhance's rewrite
                        v.push(Msg::OpenHistory(increart_core::HistoryKey::Project(p.project.id.clone())));
                    }
                }
                v
            }
            "G06-setting-sweep" => {
                // mika-the-courier's draft (G05) as a CFG sweep
                let mut f = st.generate.clone();
                f.sweep.on = true;
                f.sweep.vary = increart_core::prompts::SweepVary::Cfg;
                f.sweep.values = increart_gui_core::update::sweep_defaults(f.sweep.vary, &f);
                vec![Msg::Generate(f)]
            }
            "G12-crop-mark" => {
                let good = st
                    .index
                    .projects
                    .iter()
                    .find(|p| p.name() == "olympus-mons")
                    .and_then(|p| p.outputs.iter().find(|o| o.verdict == Some(increart_core::Verdict::Good)));
                crop_mark(st, "olympus-mons", good)
            }
            "G08-jobs" => {
                // two projects' runs pretend to generate, a seed each part-sampled
                let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Jobs)];
                let runs: Vec<_> = ["great-wave-cat", "harbour-lanterns"]
                    .iter()
                    .filter_map(|n| st.index.projects.iter().find(|p| p.name() == *n))
                    .filter_map(|p| p.runs.last())
                    .collect();
                v.push(Msg::RunsQueued(runs.iter().map(|r| r.id.clone()).collect()));
                for (i, r) in runs.iter().enumerate() {
                    v.push(Msg::Engine(increart_core::engine::Event::Progress {
                        run: r.id.clone(),
                        rev: 0,
                        seed: r.all_seeds().next().map_or(0, |(_, s)| s),
                        server: i,
                        step: [14, 5][i % 2],
                        total: 20,
                        preview: None,
                    }));
                }
                v
            }
            "G09-contact-sheet" | "G10-tile-window" => {
                // moonwalk-styles' sweep: three styles, three seeds each
                let mut v = vec![
                    Msg::RunsQueued(vec![]),
                    Msg::Tab(Tab::Review),
                    Msg::SheetRows(SheetRows::Full),
                    Msg::SheetZoom { size: increart_gui_core::SHEET_CELL_DEFAULT, save: false },
                ];
                if let Some(p) = st.index.projects.iter().find(|p| p.name() == "moonwalk-styles") {
                    v.push(Msg::ReviewSweep(p.project.id.clone(), "r01".into()));
                    v.push(Msg::SheetFilter(String::new()));
                    if name == "G10-tile-window" {
                        let mut picked: Vec<_> = p.outputs.iter().map(|o| o.image.clone()).collect();
                        picked.sort_by(|a, b| a.id.cmp(&b.id));
                        picked.truncate(6);
                        v.extend([
                            Msg::PickSet(picked),
                            Msg::OpenTile,
                            Msg::TileCaptions(increart_gui_core::CaptionFill::Style),
                            Msg::TileTitle("Moonwalk, restyled".into(), "sweep r01 · two seeds per style".into()),
                            Msg::TileSelect(Some(0)),
                        ]);
                    }
                }
                v
            }
            "G11-export" => {
                let kept: Vec<_> = st.kept_outputs().iter().map(|o| o.image.clone()).collect();
                vec![Msg::CloseTile, Msg::Tab(Tab::OutputLibrary), Msg::OpenExport(kept, "all kept images".into())]
            }
            _ => vec![],
        }
    }

    /// The scene that shows the status bar's jobs popover open.
    /// The top-bar scenes: made-up servers (none are started) and a window
    /// size.
    pub fn top_bar(&self) -> Option<(Vec<increart_gui_core::ServerStatus>, [f32; 2])> {
        let size = match *self.scenes.get(self.scene)? {
            "76-top-bar-wide" => [1400.0, 900.0],
            "77-top-bar-medium" => [1180.0, 700.0],
            "78-top-bar-narrow" => crate::MIN_WINDOW,
            _ => return None,
        };
        let gpu = |device: &str, state: &str| increart_gui_core::ServerStatus {
            config: None,
            name: format!("gpu{}", &device[4..]),
            device: device.into(),
            state: state.into(),
            model: None,
            idle_since: None,
            worked: false,
        };
        Some((vec![gpu("ROCm0", "ready"), gpu("ROCm1", "warming up")], size))
    }

    pub fn messages_popover(&self) -> bool {
        self.scenes.get(self.scene) == Some(&"87-messages-popover")
    }

    pub fn bell_popover(&self) -> bool {
        self.scenes.get(self.scene) == Some(&"74c-bell-ringing")
    }

    pub fn preview_keys(&self) -> bool {
        self.scenes.get(self.scene) == Some(&"90-preview-keys")
    }

    pub fn jobs_popover(&self) -> bool {
        self.scenes.get(self.scene) == Some(&"11-jobs-popover")
    }

    /// Feeds the synthetic input queued by a check (eframe's raw input hook).
    pub fn raw_input(&mut self, raw: &mut egui::RawInput) {
        raw.events.append(&mut self.inject);
    }

    /// Scene 24, after its screenshot: click the selected tile's Generate
    /// button, then (a few frames later) check the app switched to that
    /// project's Generate form.
    fn click_check(&mut self, ctx: &egui::Context, st: &AppState) -> bool {
        if !self.clicked {
            let Some(r) = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("tour_generate_button"))) else {
                eprintln!("CHECK grid Generate button: FAILED (button not drawn)");
                return true;
            };
            let pos = r.center();
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.inject = vec![egui::Event::PointerMoved(pos), button(true), button(false)];
            self.clicked = true;
            return false;
        }
        let ok = st.tab == Tab::Projects && st.projects_tab == ProjectsTab::Generate;
        eprintln!("CHECK grid Generate button: {}", if ok { "ok" } else { "FAILED" });
        true
    }

    /// Click a button on the selected idea card (the library's Add as refs,
    /// the inbox's Keep), then check it did what it says.
    fn card_click_check(&mut self, name: &str, ctx: &egui::Context, st: &AppState) {
        let library = name == "98j-check-library-card-refs";
        match self.settled {
            5 if !self.clicked => {
                let Some(cover) = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("tour_idea_cover"))) else {
                    eprintln!("CHECK {name}: FAILED (no selected card drawn)");
                    self.clicked = true;
                    return;
                };
                // Add as refs is the second of two; Keep the first
                let pos = crate::widgets::cover_button_rect(cover, 2, usize::from(library)).center();
                self.card_before = if library {
                    (st.lib_selected.iter().next().cloned(), st.generate.refs().len())
                } else {
                    (st.inbox_queue().get(st.inbox_cursor).map(|e| e.idea.id.clone()), st.inbox_queue().len())
                };
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                self.inject = vec![egui::Event::PointerMoved(pos), button(true), button(false)];
                self.clicked = true;
            }
            14 if self.clicked => {
                let (id, n) = (self.card_before.0.clone(), self.card_before.1);
                let (ok, what) = if library {
                    let images = id.as_ref().and_then(|id| st.idea(id)).map_or(0, |e| e.idea.images.len());
                    let now = st.generate.refs().len();
                    (images > 0 && now == n + images, format!("refs {n} → {now}, its images {images}"))
                } else {
                    let kept = id
                        .as_ref()
                        .and_then(|id| st.idea(id))
                        .is_some_and(|e| matches!(e.loc, increart_core::IdeaLoc::Library(_)));
                    (kept && st.inbox_queue().len() + 1 == n, format!("inbox {n} → {}", st.inbox_queue().len()))
                };
                eprintln!("CHECK {name}: {} ({what})", if ok { "ok" } else { "FAILED" });
            }
            _ => {}
        }
    }

    pub fn step(&mut self, ctx: &egui::Context, st: &AppState, tex: &Textures, out: &mut Vec<Msg>) {
        ctx.request_repaint();
        let Some(name) = self.scenes.get(self.scene).copied() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        self.frame += 1;
        if self.frame == 1 {
            self.started = ctx.input(|i| i.time);
            egui::Popup::close_all(ctx); // menus/popovers left open by the previous scene
            out.extend([
                Msg::CloseHistory,
                Msg::CloseIdea,
                Msg::CloseResolutions,
                Msg::CloseRecipes,
                Msg::RefPicker(false),
                Msg::OutputCompare(None),
                Msg::CloseExport,
                Msg::CloseScale,
                Msg::Preview(None),
                Msg::CancelNewProject,
            ]);
            out.extend(self.msgs(name, st));
            if name.starts_with("54") || name.starts_with(['R', 'G']) || name == "02c-koboldcpp-entry" {
                // the virtual display's pointer sits mid-window: no tooltip over it
                self.inject = vec![egui::Event::PointerGone];
            }
            return;
        }
        if name == "99-quit-with-unsaved-draft" {
            // Close on the next frame, well inside the 2 s autosave delay:
            // only the flush on quit can save the edit.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let opening_ok = name == "16-servers-opening"; // shot while opening
        let waiting = st.busy > 0
            || (!matches!(name, "91c2-rewrite-writing" | "91c0-sweep-suggesting" | "91c1d-jobs-enhancing")
                && !st.pe.jobs.is_empty())
            || (name == "91c1d-jobs-enhancing"
                && !(st.pe.styles_waiting() > 0 && st.pe.style_batch > st.pe.styles_waiting()))
            // shot once the first answer is in, the rest still asked
            || (name == "91c0-sweep-suggesting"
                && !(st.pe.blanks_waiting() > 0 && st.pe.blank_batch > st.pe.blanks_waiting()))
            || (name == "91c2-rewrite-writing"
                && !st
                    .pe
                    .jobs
                    .iter()
                    .any(|j| matches!(&j.step, increart_gui_core::PeStep::Writing { pieces, .. } if *pieces > 12)))
            || tex.busy()
            || matches!(st.phase, Phase::Library(LibraryDialog { busy: true, .. }))
            || (!opening_ok && matches!(st.phase, Phase::Servers(ServerDialog { opening: true, .. })))
            || matches!(&st.phase, Phase::Servers(ServerDialog { wizard: true, devices: None, kobold: None, .. }))
            || matches!(&st.phase, Phase::Servers(ServerDialog { wizard: true, kobold: Some(k), .. }) if k.gpus.is_none());
        // Frames aren't capped on a virtual display: also wait real time.
        let young = ctx.input(|i| i.time) - self.started < 0.6;
        self.settled = if waiting || young { 0 } else { self.settled + 1 };
        if self.settled == 20 && !name.contains("-check-") {
            let path = self.dir.join(format!("{name}.png"));
            ctx.request_screenshot(move |img| {
                let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = image::save_buffer(
                    &path,
                    &raw,
                    img.size[0] as u32,
                    img.size[1] as u32,
                    image::ExtendedColorType::Rgba8,
                );
            });
        }
        if name == "G07-prompt-history" && self.settled == 5 {
            // once loaded (loading selects the newest): the entry before the
            // rewrite, compared with the current prompt
            out.push(Msg::HistorySelect(2));
        }
        if name == "98j-check-library-card-refs" || name == "98k-check-inbox-card-keep" {
            self.card_click_check(name, ctx, st);
        }
        if name == "25-check-grid-click" && self.settled == 5 && !self.clicked {
            self.click_check(ctx, st);
        }
        if name == "25-check-grid-click" && self.settled == 12 && self.clicked {
            self.click_check(ctx, st);
        }
        if name.starts_with("3") && name.contains("-check-rename") {
            // type "-ok<scene>" at the end of the name, then Enter
            let suffix = format!("-ok{}", &name[..2]);
            let key = |key, pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Default::default(),
            };
            match self.settled {
                5 => self.inject = vec![key(egui::Key::End, true), key(egui::Key::End, false)],
                7 => self.inject = vec![egui::Event::Text(suffix.clone())],
                9 => self.inject = vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
                28 => {
                    let ok = st.index.project_folders.iter().any(|f| f.to_string_lossy().ends_with(&suffix));
                    eprintln!("CHECK {name}: {}", if ok { "ok" } else { "FAILED" });
                }
                _ => {}
            }
        }
        if name.contains("-check-drag") {
            // drag the source onto the target over a few frames, then check
            let (src, dst) = match name {
                "34-check-drag-tree" => ("projects/stickers", "projects/experiments"),
                _ => ("projects/clients", "projects/experiments"),
            };
            let rect = |target, p: &str| {
                ctx.data(|d| d.get_temp::<egui::Rect>(crate::widgets::dnd_rect_id(target, std::path::Path::new(p))))
            };
            let (a, b) = match (rect(false, src), rect(true, dst)) {
                (Some(a), Some(b)) => (a.center(), b.center()),
                _ => {
                    if self.settled == 5 {
                        eprintln!("CHECK {name}: FAILED (source or target not drawn)");
                    }
                    (egui::Pos2::ZERO, egui::Pos2::ZERO)
                }
            };
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match self.settled {
                5 => self.inject = vec![egui::Event::PointerMoved(a), button(a, true)],
                6 => self.inject = vec![egui::Event::PointerMoved(a + egui::vec2(12.0, 6.0))],
                7 => self.inject = vec![egui::Event::PointerMoved(a.lerp(b, 0.5))],
                8 | 9 => self.inject = vec![egui::Event::PointerMoved(b)],
                // mid-drag: the target's outline and the "Move …" label
                10 if std::env::var_os("INCREART_TOUR_DRAG_SHOTS").is_some() => {
                    let path = self.dir.join(format!("{name}-mid-drag.png"));
                    ctx.request_screenshot(move |img| {
                        let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                        let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                        let _ = image::save_buffer(&path, &raw, w, h, image::ExtendedColorType::Rgba8);
                    });
                    self.inject = vec![egui::Event::PointerMoved(b)];
                }
                11 => self.inject = vec![button(b, false)],
                25 if std::env::var_os("INCREART_TOUR_DRAG_SHOTS").is_some() => {
                    let path = self.dir.join(format!("{name}-after.png"));
                    ctx.request_screenshot(move |img| {
                        let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                        let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                        let _ = image::save_buffer(&path, &raw, w, h, image::ExtendedColorType::Rgba8);
                    });
                }
                28 => {
                    let ok = match name {
                        "34-check-drag-tree" => st
                            .index
                            .projects
                            .iter()
                            .any(|p| p.dir == std::path::Path::new("projects/experiments/stickers")),
                        _ => st.index.project_folders.iter().any(|f| f == std::path::Path::new("experiments/clients")),
                    };
                    eprintln!("CHECK {name}: {}", if ok { "ok" } else { "FAILED" });
                }
                _ => {}
            }
        }
        if let Some((a, b)) = match name {
            // the contact sheet's first cell; the Tile window's first two tiles
            "96b-picked-menu" => Some((egui::pos2(478.0, 197.0), None)),
            "97c-tile-menu" => Some((egui::pos2(480.0, 265.0), None)),
            // the empty row below the tiles
            "97d-tile-cell-menu" => Some((egui::pos2(305.0, 640.0), None)),
            // empty space below the sheet up over two columns of three rows;
            // the first tile down into the empty row
            "96c-sheet-band" => Some((egui::pos2(1000.0, 700.0), Some(egui::pos2(600.0, 300.0)))),
            "97b-check-tile-drag" => Some((egui::pos2(305.0, 265.0), Some(egui::pos2(305.0, 640.0)))),
            _ => None,
        } {
            let button =
                |pos, b, pressed| egui::Event::PointerButton { pos, button: b, pressed, modifiers: Default::default() };
            let (l, r) = (egui::PointerButton::Primary, egui::PointerButton::Secondary);
            match (self.settled, b) {
                (5, None) if !self.clicked => self.inject = vec![egui::Event::PointerMoved(a), button(a, r, true)],
                (6, None) if !self.clicked => {
                    self.inject = vec![button(a, r, false)];
                    self.clicked = true;
                }
                (5, Some(_)) => self.inject = vec![egui::Event::PointerMoved(a), button(a, l, true)],
                (6, Some(_)) => self.inject = vec![egui::Event::PointerMoved(a + egui::vec2(-20.0, -20.0))],
                (7, Some(b)) => self.inject = vec![egui::Event::PointerMoved(a.lerp(b, 0.5))],
                (8 | 9, Some(b)) => self.inject = vec![egui::Event::PointerMoved(b)],
                (10, Some(b)) => self.inject = vec![button(b, l, false)],
                (20, Some(_)) => {
                    let ok = match name {
                        "96c-sheet-band" => st.picked.len() == 5,
                        _ => st.tile.as_ref().is_some_and(|t| (t.layout.tiles[0].col, t.layout.tiles[0].row) == (0, 2)),
                    };
                    eprintln!("CHECK {name}: {} ({} picked)", if ok { "ok" } else { "FAILED" }, st.picked.len());
                }
                _ => {}
            }
        }
        if name == "41-check-rubber-band" {
            // drag on empty space from the bottom right over every tile
            let (a, b) = (egui::pos2(1300.0, 700.0), egui::pos2(190.0, 100.0));
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match self.settled {
                5 => self.inject = vec![egui::Event::PointerMoved(a), button(a, true)],
                6 => self.inject = vec![egui::Event::PointerMoved(a - egui::vec2(20.0, 20.0))],
                7 => self.inject = vec![egui::Event::PointerMoved(a.lerp(b, 0.5))],
                8 | 9 => self.inject = vec![egui::Event::PointerMoved(b)],
                10 => self.inject = vec![button(b, false)],
                20 => {
                    let (n, all) = (st.folder_view.selected.len(), st.folder_view.shown().len());
                    eprintln!(
                        "CHECK {name}: {} ({n} of {all} selected)",
                        if n == all && all > 1 { "ok" } else { "FAILED" }
                    );
                }
                _ => {}
            }
        }
        if name == "84-projects-folder-menu" {
            // right-click the first entry: its menu has Show outputs
            let pos = Some(egui::pos2(FV_FIRST_ENTRY.0, FV_FIRST_ENTRY.1));
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: Default::default(),
            };
            match (self.settled, pos) {
                (5, Some(p)) if !self.clicked => self.inject = vec![egui::Event::PointerMoved(p), button(p, true)],
                (6, Some(p)) if !self.clicked => {
                    self.inject = vec![button(p, false)];
                    self.clicked = true;
                }
                _ => {}
            }
        }
        if name == "48-check-copy-image" {
            // right-click the preview, then its first item (Copy image)
            let c = ctx.content_rect().center();
            let button =
                |pos, b, pressed| egui::Event::PointerButton { pos, button: b, pressed, modifiers: Default::default() };
            let item = c + egui::vec2(40.0, 14.0);
            let (l, r) = (egui::PointerButton::Primary, egui::PointerButton::Secondary);
            match self.settled {
                5 if !self.clicked => self.inject = vec![egui::Event::PointerMoved(c), button(c, r, true)],
                6 if !self.clicked => self.inject = vec![button(c, r, false)],
                9 if !self.clicked => self.inject = vec![egui::Event::PointerMoved(item), button(item, l, true)],
                10 if !self.clicked => {
                    self.inject = vec![button(item, l, false)];
                    self.clicked = true;
                }
                20 => {
                    let ok = st.messages.iter().any(|m| m.text == "Image copied to the clipboard");
                    eprintln!(
                        "CHECK {name}: {} (preview still open: {})",
                        if ok { "ok" } else { "FAILED" },
                        st.preview.is_some()
                    );
                }
                _ => {}
            }
        }
        // zoomed in three steps (+), the view on the image's middle
        // or one image on (→)
        let step = match name {
            "82-preview-zoom" if (5..8).contains(&self.settled) => Some(egui::Key::Plus),
            "83-preview-next" if self.settled == 5 => Some(egui::Key::ArrowRight),
            "92-check-sheet-arrows" | "92b-check-sheet-arrows-folded" if self.settled == 5 => {
                Some(egui::Key::ArrowDown)
            }
            "92b-check-sheet-arrows-folded" if self.settled == 8 => Some(egui::Key::ArrowRight),
            "93-check-sheet-arrows-click" | "94-check-sheet-arrows-slider" if self.settled == 10 => {
                Some(egui::Key::ArrowDown)
            }
            _ => None,
        };
        if let Some(key) = step {
            self.inject = vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }];
        }
        if name == "92-check-sheet-arrows" && self.settled == 12 {
            let ok = st.sheet_cell == (1, 2);
            eprintln!("CHECK {name}: {} (cell {:?})", if ok { "ok" } else { "FAILED" }, st.sheet_cell);
        }
        // two rows side by side: ↓ is two rows on, → past a row's last seed
        // the next row's first
        if name == "92b-check-sheet-arrows-folded" && self.settled == 12 {
            let ok = st.sheet_bands == 2 && st.sheet_cell == (3, 0);
            eprintln!(
                "CHECK {name}: {} ({} bands, cell {:?})",
                if ok { "ok" } else { "FAILED" },
                st.sheet_bands,
                st.sheet_cell
            );
        }
        // in the large preview opened from the sheet, ↓ goes a style down
        // (the same seed) and ↑ back
        if name == "95-check-sheet-preview-arrows" {
            use increart_gui_core::SheetCell;
            let cell = |r: usize| match st.sheet()?.rows.get(r)?.cells.first()? {
                SheetCell::Image(o) => Some(o.image.clone()),
                _ => None,
            };
            let key = |key| egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            };
            match self.settled {
                3 => out.push(Msg::Preview(cell(0))),
                6 => self.inject = vec![key(egui::Key::ArrowDown)],
                10 => {
                    let ok = st.preview.is_some() && st.preview == cell(1) && st.sheet_cell == (1, 0);
                    eprintln!("CHECK {name} ↓: {} (cell {:?})", if ok { "ok" } else { "FAILED" }, st.sheet_cell);
                    self.inject = vec![key(egui::Key::ArrowUp)];
                }
                14 => {
                    let ok = st.preview.is_some() && st.preview == cell(0) && st.sheet_cell == (0, 0);
                    eprintln!("CHECK {name} ↑: {} (cell {:?})", if ok { "ok" } else { "FAILED" }, st.sheet_cell);
                }
                _ => {}
            }
        }
        // the same after a click on a thumbnail (0, 0) or the zoom slider
        if let Some((at, want)) = match name {
            "93-check-sheet-arrows-click" => Some((egui::pos2(478.0, 172.0), (1, 0))),
            "94-check-sheet-arrows-slider" => Some((egui::pos2(545.0, 69.0), (1, 2))),
            _ => None,
        } {
            let click = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match self.settled {
                3 => self.inject = vec![egui::Event::PointerMoved(at)],
                4 => self.inject = vec![click(true)],
                5 => self.inject = vec![click(false)],
                9 => eprintln!(
                    "{name}: before the key, cell {:?}, focus {:?}",
                    st.sheet_cell,
                    ctx.memory(|m| m.focused())
                ),
                16 => {
                    let ok = st.sheet_cell == want;
                    eprintln!("CHECK {name}: {} (cell {:?})", if ok { "ok" } else { "FAILED" }, st.sheet_cell);
                }
                _ => {}
            }
        }
        if name == "46-check-ref-double-click" {
            let at = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("tour_first_ref"))).map(|r| r.center());
            let click = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match (self.settled, at) {
                (5, None) => eprintln!("CHECK {name}: FAILED (reference not drawn)"),
                // like a hand: one event per frame, once (the counter starts
                // over while the large image loads)
                (5, Some(p)) if !self.clicked => self.inject = vec![egui::Event::PointerMoved(p), click(p, true)],
                (6, Some(p)) if !self.clicked => self.inject = vec![click(p, false)],
                (7, Some(p)) if !self.clicked => self.inject = vec![click(p, true)],
                (8, Some(p)) if !self.clicked => {
                    self.inject = vec![click(p, false)];
                    self.clicked = true;
                }
                (12, _) => {
                    let ok = st.preview.is_some() && st.preview == st.generate.refs().first().cloned();
                    eprintln!("CHECK {name}: {}", if ok { "ok" } else { "FAILED" });
                    if !ok {
                        eprintln!(
                            "  preview {:?}, first ref {:?}, clicked at {at:?}",
                            st.preview,
                            st.generate.refs().first()
                        );
                    }
                }
                // Esc closes it
                (14, _) => {
                    self.inject = vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    }]
                }
                (20, _) => {
                    eprintln!("CHECK {name} closes on Esc: {}", if st.preview.is_none() { "ok" } else { "FAILED" })
                }
                _ => {}
            }
        }
        if name == "37-check-width-rounding" {
            // type 1000 into the width field: it must become 992
            let key = |key, pressed, modifiers| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            };
            let field = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique("tour_width_field")));
            let click = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            match (self.settled, field) {
                (5, Some(r)) => {
                    self.inject =
                        vec![egui::Event::PointerMoved(r.center()), click(r.center(), true), click(r.center(), false)]
                }
                (7, _) => {
                    let ctrl = egui::Modifiers::COMMAND;
                    self.inject = vec![key(egui::Key::A, true, ctrl), key(egui::Key::A, false, ctrl)]
                }
                (8, _) => self.inject = vec![egui::Event::Text("1000".into())],
                (10, _) => self.inject = vec![key(egui::Key::Enter, true, Default::default())],
                (11, _) => self.inject = vec![key(egui::Key::Enter, false, Default::default())],
                (5, None) => eprintln!("CHECK {name}: FAILED (width field not drawn)"),
                (20, _) => {
                    let w = st.generate.width;
                    eprintln!("CHECK {name}: {} (width {w})", if w == 992 { "ok" } else { "FAILED" });
                }
                _ => {}
            }
        }
        if name == "36-check-grid-hover" {
            // hover a tile that isn't selected: no tile may move
            let tiles = || -> Vec<Option<egui::Rect>> {
                st.index
                    .projects
                    .iter()
                    .map(|p| ctx.data(|d| d.get_temp::<egui::Rect>(crate::widgets::dnd_rect_id(false, &p.dir))))
                    .collect()
            };
            match self.settled {
                4 => self.before = tiles(),
                5 => {
                    let unselected = st.index.projects.iter().position(|p| st.project.as_ref() != Some(&p.project.id));
                    if let Some(Some(r)) = unselected.map(|i| self.before.get(i).copied().flatten()) {
                        self.inject = vec![egui::Event::PointerMoved(r.center())];
                    }
                }
                8..=14 => {
                    let now = tiles();
                    if now != self.before {
                        eprintln!("CHECK {name}: FAILED (tiles moved on hover: {:?} -> {:?})", self.before, now);
                        self.settled = 15;
                    }
                }
                15 => eprintln!("CHECK {name}: ok"),
                _ => {}
            }
        }
        if name == "97i-check-output-double-click" || name == "97j-check-library-click" {
            let double = name == "97i-check-output-double-click";
            let key = if double { "tour_first_output" } else { "tour_first_idea" };
            let at = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::unique(key))).map(|r| r.center());
            let click = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            // like a hand, one event a frame; the click restarts the scene's
            // clock, so the check comes after the double-click delay
            let last = if double { 8 } else { 6 };
            match (self.settled, at) {
                (5, None) => eprintln!("CHECK {name}: FAILED (nothing drawn to click)"),
                (5, Some(p)) if !self.clicked => self.inject = vec![egui::Event::PointerMoved(p), click(p, true)],
                (6..=8, Some(p)) if !self.clicked && self.settled <= last => {
                    self.inject = vec![click(p, self.settled == 7)];
                    if self.settled == last {
                        self.clicked = true;
                        self.started = ctx.input(|i| i.time);
                    }
                }
                (12, _) if self.clicked => {
                    let ok = if double {
                        st.preview.is_some() && st.output_selected.is_none()
                    } else {
                        let first = st.library_ideas().first().map(|e| e.idea.id.clone());
                        first.is_some() && st.lib_info == first
                    };
                    eprintln!(
                        "CHECK {name}: {} (preview {}, output panel {}, library panel {})",
                        if ok { "ok" } else { "FAILED" },
                        st.preview.is_some(),
                        st.output_selected.is_some(),
                        st.lib_info.is_some()
                    );
                }
                _ => {}
            }
        }
        if name == "98e-check-show-outputs-reveal" {
            self.reveal_check(ctx, st, out);
        }
        if self.settled == 30 {
            (self.scene, self.frame, self.settled, self.clicked, self.stage) = (self.scene + 1, 0, 0, false, 0);
        }
    }

    /// As a user would: open "experiments" (off fox-cafe's way), close "by
    /// project", then Show outputs on fox-cafe with a project-name search
    /// that hides it. The search goes, "by project" opens with only the way
    /// to fox-cafe open (experiments closed again), and fox-cafe's row is
    /// selected and in view.
    fn reveal_check(&mut self, ctx: &egui::Context, st: &AppState, out: &mut Vec<Msg>) {
        let name = "98e-check-show-outputs-reveal";
        let rect = |key: egui::Id| ctx.data(|d| d.get_temp::<egui::Rect>(key));
        let header = rect(egui::Id::unique("tour_by_project_header"));
        let experiments = rect(egui::Id::unique(("tour_folder_toggle", std::path::Path::new("experiments"))));
        let now = ctx.cumulative_pass_nr();
        // whether glass-fish (inside experiments) was drawn lately
        let fish = ctx
            .data(|d| d.get_temp::<u64>(egui::Id::unique(("tour_project_row", "glass-fish".to_string()))))
            .is_some_and(|pass| pass + 3 >= now);
        let click = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        // after each step the clock restarts, so the folders finish animating
        let next = |t: &mut Tour| (t.stage, t.started) = (t.stage + 1, ctx.input(|i| i.time));
        let target = match self.stage {
            0 => experiments,
            1 => header,
            _ => None,
        };
        match (self.stage, self.settled, target) {
            (0 | 1, 5, None) => eprintln!("CHECK {name}: FAILED (nothing to click at step {})", self.stage),
            (0 | 1, 5, Some(r)) => self.inject = vec![egui::Event::PointerMoved(r.center()), click(r.center(), true)],
            (0 | 1, 6, Some(r)) => {
                self.inject = vec![click(r.center(), false)];
                next(self);
            }
            (1, 4, _) => self.clicked = fish, // experiments opened
            (2, 5, _) => {
                let seen =
                    ctx.data(|d| d.get_temp::<(egui::Rect, egui::Rect, u64)>(egui::Id::unique("tour_selected_source")));
                let closed = seen.is_some_and(|(_, _, pass)| pass + 3 < now);
                self.clicked &= closed;
                let mut v = vec![
                    Msg::OutputSource(OutputSource::AllProjects),
                    Msg::OutputSearchProjects(true),
                    Msg::OutputProjectSearch("zzz".into()),
                    Msg::Tab(Tab::Projects),
                ];
                v.extend(fox_cafe(st).map(OutputSource::Project).map(Msg::ShowOutputs));
                out.extend(v);
                next(self);
            }
            (3, 5, _) => {
                let seen =
                    ctx.data(|d| d.get_temp::<(egui::Rect, egui::Rect, u64)>(egui::Id::unique("tour_selected_source")));
                let visible = seen.is_some_and(|(r, clip, pass)| pass + 2 >= now && clip.contains_rect(r));
                let fox = fox_cafe(st).is_some_and(|id| st.output_source == OutputSource::Project(id));
                let set_up = self.clicked;
                let ok = set_up && visible && !fish && fox && st.output_project_search.is_empty() && !st.output_reveal;
                eprintln!(
                    "CHECK {name}: {} (opened experiments and closed by project {set_up}, row visible {visible}, \
                     experiments closed again {}, fox-cafe {fox}, search {:?}, reveal {})",
                    if ok { "ok" } else { "FAILED" },
                    !fish,
                    st.output_project_search,
                    st.output_reveal
                );
            }
            _ => {}
        }
    }
}

/// An input library image with an idea title, for the compare scenes.
/// Scene 15b and G07: `output` made the start image of `project`'s Generate,
/// cropped, with a circle and a scribble.
fn crop_mark(st: &AppState, project: &str, output: Option<&increart_core::scan::OutputLink>) -> Vec<Msg> {
    let mut v = vec![Msg::Tab(Tab::Projects), Msg::ProjectsTab(ProjectsTab::Generate)];
    if let (Some(p), Some(root), Some(o)) =
        (st.index.projects.iter().find(|p| p.name() == project), st.library.as_ref(), output)
    {
        use increart_core::model::{ImageEdits, Mark, Shape};
        let (w, h) = image::image_dimensions(o.image.path(root)).unwrap_or((1024, 1024));
        let (w, h) = (w as f32, h as f32);
        let red = increart_gui_core::MarkTools::default().color;
        let width = w.max(h) * increart_gui_core::MarkTools::default().width / 1000.0;
        let scribble = (0..24)
            .map(|i| {
                let t = i as f32 / 23.0;
                [w * (0.15 + 0.3 * t), h * (0.75 + 0.04 * (t * 12.0).sin())]
            })
            .collect();
        v.push(Msg::SelectProject(p.project.id.clone()));
        v.push(Msg::ClearGenerate);
        v.push(Msg::UseImage(o.image.clone(), increart_gui_core::UseTarget::CurrentProject));
        v.push(Msg::OpenMarkup(o.image.clone(), Some(0)));
        v.push(Msg::MarkupEdit(ImageEdits {
            crop: Some([(w * 0.05) as u32, (h * 0.1) as u32, (w * 0.85) as u32, (h * 0.8) as u32]),
            marks: vec![
                Mark { shape: Shape::Ellipse([w * 0.35, h * 0.3], [w * 0.65, h * 0.6]), color: red, width },
                Mark { shape: Shape::Pencil(scribble), color: [255, 210, 0, 191], width },
            ],
        }));
    }
    v
}

fn tour_input(st: &AppState) -> Option<increart_core::ImageRef> {
    let mut ideas: Vec<_> = st
        .index
        .ideas
        .iter()
        .filter(|e| matches!(e.loc, increart_core::IdeaLoc::Library(_)) && !e.idea.images.is_empty())
        .collect();
    ideas.sort_by_key(|e| e.idea.title.is_empty());
    ideas.first().map(|e| increart_core::ImageRef::input(e.idea.images[0].clone()))
}

/// An image waiting for review that was made from references, if any.
fn tour_with_refs(st: &AppState) -> Option<increart_core::ImageRef> {
    st.review_queue().iter().map(|o| o.image.clone()).find(|i| !st.image_inputs(i).is_empty())
}

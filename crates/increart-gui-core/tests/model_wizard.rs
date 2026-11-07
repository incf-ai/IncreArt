//! The **Models** window: folders found, presets picked, what's fetched
//! where; Your models alongside.

use increart_core::config::{Config, catalogue};
use increart_core::modelsetup::FoundFolder;
use increart_gui_core::*;

fn state(dirs: Vec<String>) -> AppState {
    let mut config = Config::default();
    config.normalize();
    config.recipes = vec![];
    config.programs.model_dirs = dirs;
    AppState::new(config, None)
}

#[test]
fn the_folder_found_folders_and_picked_presets_become_the_setup() {
    let tmp = tempfile::tempdir().unwrap();
    let mine = tmp.path().join("mine").to_string_lossy().into_owned();
    let comfy = tmp.path().join("ComfyUI/models");
    let mut s = state(vec![]);
    let fx = update(&mut s, Msg::OpenModelWizard);
    // it looks for folders, the GPUs and the downloads' sizes
    assert!(fx.iter().any(|f| matches!(f, Effect::FindModelFolders { .. })));
    assert!(fx.iter().any(|f| matches!(f, Effect::DetectGpus(_))));
    let Some(Effect::DownloadSizes(urls)) = fx.iter().find(|f| matches!(f, Effect::DownloadSizes(_))) else {
        panic!("{fx:?}")
    };
    assert!(urls.len() > 20 && urls.iter().all(|u| u.starts_with("https://")));
    assert!(s.model_wizard.as_ref().unwrap().dir.ends_with("models"));
    // a folder of the first preset's files (each slot's first choice), found
    let first = &catalogue()[0];
    for f in first.files.values() {
        let p = comfy.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"1234").unwrap();
    }
    let path = comfy.to_string_lossy().into_owned();
    update(&mut s, Msg::ModelFoldersFound(vec![FoundFolder { path: path.clone(), files: 3, bytes: 12 }]));
    let w = s.model_wizard.clone().unwrap();
    assert!(w.add.contains(&path), "found folders ticked");
    // its files there: nothing to fetch for it; the second needs downloads
    let dirs = w.folders(&[]);
    let needs = PresetNeeds::of(first, &dirs, &w.sizes, None);
    assert!(needs.downloads.is_empty() && needs.total_bytes == Some(12) && !needs.unavailable, "{needs:?}");
    assert_eq!(needs.fits(1.0), Some(true));
    let second = &catalogue()[1];
    let needs = PresetNeeds::of(second, &dirs, &w.sizes, None);
    assert!(!needs.downloads.is_empty() && needs.download_bytes.is_none(), "sizes not known yet");
    for d in &needs.downloads {
        update(&mut s, Msg::DownloadSize(d.url.clone(), Some(1_000)));
    }
    let w = s.model_wizard.clone().unwrap();
    assert_eq!(
        PresetNeeds::of(second, &dirs, &w.sizes, None).download_bytes,
        Some(1_000 * needs.downloads.len() as u64)
    );
    // picked, in my folder
    let mut w = s.model_wizard.clone().unwrap();
    w.dir = mine.clone();
    w.picked = [first.name.clone(), second.name.clone()].into();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    // Your models open in the same window, a new one started there
    update(&mut s, Msg::OpenRecipes);
    let mut e = s.recipe_editor.clone().unwrap();
    assert!(!e.changed(&s.config.recipes));
    e.add_new();
    let mine_row = &mut e.rows.last_mut().unwrap().preset;
    mine_row.name = "Mine".into();
    mine_row.files.insert("diffusion_model".into(), "mine.safetensors".into());
    update(&mut s, Msg::EditRecipes(e));
    let fx = update(&mut s, Msg::ModelWizardGo);
    // the installed ones are in the editor too, so saving keeps them
    let e = s.recipe_editor.clone().unwrap();
    let names: Vec<&str> = e.rows.iter().map(|r| r.preset.name.as_str()).collect();
    assert_eq!(names, ["Mine", first.name.as_str(), second.name.as_str()]);
    assert!(e.changed(&s.config.recipes));
    let mut e2 = e.clone();
    e2.select_named(&second.name);
    assert_eq!(e2.selected, 2);
    assert_eq!(s.config.programs.model_dirs, vec![mine.clone(), path]);
    assert!(std::path::Path::new(&mine).is_dir());
    let names: Vec<&str> = s.config.recipes.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, [first.name.as_str(), second.name.as_str()]);
    let Some(Effect::DownloadModels { dir, files, .. }) =
        fx.iter().find(|f| matches!(f, Effect::DownloadModels { .. }))
    else {
        panic!("{fx:?}")
    };
    assert_eq!(dir, &mine);
    assert_eq!(files.len(), needs.downloads.len(), "only the second's files");
    assert!(s.preset_download.is_some());
    update(&mut s, Msg::SaveRecipes);
    let names: Vec<&str> = s.config.recipes.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Mine", first.name.as_str(), second.name.as_str()]);
    // again while it downloads: refused
    update(&mut s, Msg::ModelWizardGo);
    assert!(s.messages.iter().any(|m| m.error && m.text.contains("under way")));
    // the window closed: the download goes on, the Models half keeps its state
    update(&mut s, Msg::CloseModelWizard);
    assert!(s.model_wizard.as_ref().is_some_and(|w| !w.editing) && s.preset_download.is_some());
}

#[test]
fn the_folders_in_use_arent_offered_and_the_first_is_the_default() {
    let mut s = state(vec!["/m/a".into(), "/m/b".into()]);
    update(&mut s, Msg::OpenModelWizard);
    assert_eq!(s.model_wizard.as_ref().unwrap().dir, "/m/a");
    let found = |p: &str| FoundFolder { path: p.into(), files: 1, bytes: 1 };
    update(&mut s, Msg::ModelFoldersFound(vec![found("/m/b"), found("/x")]));
    let w = s.model_wizard.as_ref().unwrap();
    assert_eq!(w.found.as_ref().unwrap(), &vec![found("/x")]);
    // an existing folder isn't moved
    assert_eq!(w.folders(&s.config.programs.model_dirs), vec!["/m/a", "/m/b", "/x"]);
}

#[test]
fn sizes_are_asked_once_a_session() {
    let mut s = state(vec![]);
    let fx = update(&mut s, Msg::OpenModelWizard);
    let Some(Effect::DownloadSizes(urls)) = fx.into_iter().find(|f| matches!(f, Effect::DownloadSizes(_))) else {
        panic!()
    };
    update(&mut s, Msg::DownloadSize(urls[0].clone(), Some(5)));
    // the window closed and the Models half shown again: nothing asked again
    update(&mut s, Msg::CloseModelWizard);
    assert!(update(&mut s, Msg::OpenModelWizard).is_empty());
    // a new session's half (the state gone): only what isn't known
    s.model_wizard = None;
    let fx = update(&mut s, Msg::OpenModelWizard);
    let Some(Effect::DownloadSizes(again)) = fx.into_iter().find(|f| matches!(f, Effect::DownloadSizes(_))) else {
        panic!()
    };
    assert_eq!(again.len(), urls.len() - 1);
    assert_eq!(s.model_wizard.as_ref().unwrap().sizes[&urls[0]], Some(5));
}

#[test]
fn the_models_are_listed_as_yours_installed_and_not_and_filtered() {
    use increart_core::config::RecipePreset;
    let catalogue = increart_core::config::catalogue();
    let mine = RecipePreset { name: "My own".into(), recipe: "z-image-turbo".into(), ..Default::default() };
    let recipes = vec![mine, catalogue[1].clone()];
    let sizes = std::collections::BTreeMap::new();
    let lists = ModelLists::of(&recipes, &catalogue, &[], &sizes, None);
    let names = |v: &[(&RecipePreset, PresetNeeds)]| v.iter().map(|(p, _)| p.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&lists.custom), ["My own"]);
    assert_eq!(names(&lists.installed), [catalogue[1].name.clone()]);
    assert_eq!(lists.available.len(), catalogue.len() - 1);
    assert!(!names(&lists.available).contains(&catalogue[1].name));
    // search: every word, in the name, recipe or note
    let mut w = ModelWizard { search: "qwen EDIT".into(), ..Default::default() };
    let shown = names(&w.shown(lists.available.clone(), &[]));
    assert!(!shown.is_empty() && shown.iter().all(|n| n.contains("Qwen-Image Edit")), "{shown:?}");
    assert!(w.filtering());
    // kind
    w.search.clear();
    w.kind = ModelKind::Lora;
    let shown = w.shown(lists.available.clone(), &[]);
    assert!(!shown.is_empty() && shown.iter().all(|(p, _)| !p.loras.is_empty()));
    w.kind = ModelKind::Edit;
    assert!(w.shown(lists.available.clone(), &[]).iter().any(|(p, _)| p.name == "FLUX.1 Kontext"));
    // fits a ticked GPU: sizes not known shown; none ticked: every size
    w.kind = ModelKind::All;
    assert_eq!(w.shown(lists.available.clone(), &[24.0]).len(), lists.available.len());
    let mut sized = lists.available.clone();
    sized[0].1.total_bytes = Some(30 << 30);
    sized[1].1.total_bytes = Some(10 << 30);
    let fits = w.shown(sized.clone(), &[24.0]);
    assert!(
        !fits.iter().any(|(p, _)| p.name == sized[0].0.name) && fits.iter().any(|(p, _)| p.name == sized[1].0.name)
    );
    assert!(w.shown(sized.clone(), &[24.0, 48.0]).iter().any(|(p, _)| p.name == sized[0].0.name));
    assert_eq!(w.shown(sized, &[]).len(), lists.available.len());
    use increart_core::tiers::Gpu;
    let gpu = |number, vram_gib| Gpu { number, name: String::new(), vram_gib, integrated: false };
    assert!(!w.filtering());
    w.gpus_off.insert(1);
    assert_eq!(w.ticked_vram(&[gpu(0, 24.0), gpu(1, 48.0)]), [24.0]);
    assert!(w.filtering());
    // a heading clicked: by it; again: the other way; a third time: the list's order
    w.sort.click(ModelColumn::Name);
    let by_name = names(&w.shown(lists.available.clone(), &[]));
    let mut sorted = by_name.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(by_name, sorted);
    w.sort.click(ModelColumn::Name);
    sorted.reverse();
    assert_eq!(names(&w.shown(lists.available.clone(), &[])), sorted);
    w.sort.click(ModelColumn::Name);
    assert_eq!(w.sort, ModelSort::default());
    assert_eq!(names(&w.shown(lists.available.clone(), &[])), names(&lists.available));
    // size: sizes not known last either way
    let mut rows = lists.available.clone();
    rows[3].1.total_bytes = Some(9);
    rows[5].1.total_bytes = Some(20);
    w.sort.click(ModelColumn::Size);
    let small = w.shown(rows.clone(), &[]);
    assert_eq!((small[0].1.total_bytes, small[1].1.total_bytes, small[2].1.total_bytes), (Some(9), Some(20), None));
    w.sort.click(ModelColumn::Size);
    let big = w.shown(rows.clone(), &[]);
    assert_eq!((big[0].1.total_bytes, big[1].1.total_bytes, big[2].1.total_bytes), (Some(20), Some(9), None));
    // another column: by it, ascending
    w.sort.click(ModelColumn::Fits);
    assert_eq!(w.sort, ModelSort { column: Some(ModelColumn::Fits), descending: false });
    // fits the most GPUs first: 9 bytes fits both, then a size not known
    let fits = w.shown(rows, &[0.000001, 24.0]);
    assert_eq!(fits[0].1.total_bytes, Some(9));
    assert_eq!(fits.last().unwrap().1.total_bytes, None);
}

#[test]
fn files_kept_elsewhere_are_linked_not_downloaded() {
    use increart_core::modelsetup::FileIndex;
    let tmp = tempfile::tempdir().unwrap();
    let mine = tmp.path().join("mine").to_string_lossy().into_owned();
    // another tool's folder, its own layout: the second built-in's files
    // loose in it, one of them the wrong size
    let other = tmp.path().join("other/stuff");
    std::fs::create_dir_all(&other).unwrap();
    let p = &catalogue()[1];
    let mut s = state(vec![]);
    update(&mut s, Msg::OpenModelWizard);
    let wants: Vec<_> = increart_gui_core::missing_downloads(p, &[]);
    assert!(wants.len() >= 2, "{wants:?}");
    for (i, d) in wants.iter().enumerate() {
        let name = std::path::Path::new(&d.file).file_name().unwrap();
        std::fs::write(other.join(name), if i == 0 { b"12" as &[u8] } else { b"1234" }).unwrap();
        update(&mut s, Msg::DownloadSize(d.url.clone(), Some(4)));
    }
    // looked for once the folders are found; the answer kept
    update(&mut s, Msg::ModelFoldersFound(vec![]));
    let mut w = s.model_wizard.clone().unwrap();
    w.dir = mine.clone();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    let fx = update(&mut s, Msg::IndexModelFiles);
    let Some(Effect::IndexModelFiles(dirs)) = fx.first() else { panic!("{fx:?}") };
    assert!(dirs.contains(&std::path::PathBuf::from(&mine)));
    assert!(update(&mut s, Msg::IndexModelFiles).is_empty(), "not twice at once");
    let mut dirs = dirs.clone();
    dirs.push(tmp.path().join("other"));
    let index = FileIndex::build(&dirs);
    update(&mut s, Msg::ModelFilesIndexed(dirs, Box::new(index)));
    let w = s.model_wizard.clone().unwrap();
    let needs = PresetNeeds::of(p, &w.folders(&[]), &w.sizes, w.index.as_deref());
    // the right size linked, the wrong one downloaded
    assert_eq!(needs.links.len(), wants.len() - 1, "{needs:?}");
    assert_eq!(needs.downloads.len(), 1);
    assert_eq!(needs.downloads[0].file, wants[0].file);
    // installed: linked into the download folder, only the other fetched
    let mut w = w;
    w.picked = [p.name.clone()].into();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    let fx = update(&mut s, Msg::ModelWizardGo);
    for d in &wants[1..] {
        let at = std::path::Path::new(&mine).join(&d.file);
        assert!(std::fs::symlink_metadata(&at).unwrap().file_type().is_symlink(), "{at:?}");
        assert_eq!(std::fs::read(&at).unwrap(), b"1234");
    }
    let Some(Effect::DownloadModels { files, .. }) = fx.iter().find(|f| matches!(f, Effect::DownloadModels { .. }))
    else {
        panic!("{fx:?}")
    };
    assert_eq!(files.len(), 1);
    assert!(s.messages.iter().any(|m| m.text.contains("linked from where they were")));
    // looked for again (the files changed)
    assert!(s.model_wizard.as_ref().unwrap().index_of.is_empty());
}

#[test]
fn link_whats_here_links_every_built_in_models_files_found() {
    use increart_core::modelsetup::FileIndex;
    let tmp = tempfile::tempdir().unwrap();
    let mine = tmp.path().join("mine").to_string_lossy().into_owned();
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let p = &catalogue()[2];
    let wants = increart_gui_core::missing_downloads(p, &[]);
    for d in &wants {
        std::fs::write(other.join(std::path::Path::new(&d.file).file_name().unwrap()), b"x").unwrap();
    }
    let mut s = state(vec![]);
    update(&mut s, Msg::OpenModelWizard);
    update(&mut s, Msg::ModelFoldersFound(vec![]));
    let mut w = s.model_wizard.clone().unwrap();
    w.dir = mine.clone();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    // sizes not known: by name
    update(&mut s, Msg::ModelFilesIndexed(vec![other.clone()], Box::new(FileIndex::build(&[other]))));
    let (links, whole) = s.model_wizard.as_ref().unwrap().links_here(&[]);
    assert!(links.len() >= wants.len() && whole >= 1, "{links:?}");
    update(&mut s, Msg::ModelWizardLinkHere);
    for d in &wants {
        assert!(std::path::Path::new(&mine).join(&d.file).is_file(), "{}", d.file);
    }
    // the download folder made a model folder; nothing installed
    assert_eq!(s.config.programs.model_dirs, vec![mine]);
    assert!(s.config.recipes.is_empty());
    assert!(s.messages.iter().any(|m| m.text.contains("linked into")), "{:?}", s.messages);
}

/// As reported: a config from before (the default model with sd-server's
/// int8 files, nothing to download) where the KoboldCpp wizard put its
/// files in koboldcpp/.
#[test]
fn an_older_copy_of_a_built_in_says_so_and_install_updates_it() {
    use increart_core::modelsetup::FileIndex;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_string_lossy().into_owned();
    let builtin = catalogue().into_iter().find(|p| p.name == "Qwen-Image 2.1").unwrap();
    // the wizard's files: the built-in's VAE and text encoder among them
    let kcpp = tmp.path().join("koboldcpp");
    std::fs::create_dir_all(&kcpp).unwrap();
    for f in [&builtin.files["vae"], &builtin.files["llm"]] {
        std::fs::write(kcpp.join(std::path::Path::new(f).file_name().unwrap()), b"12345").unwrap();
    }
    let mut old = increart_core::config::builtin_recipes()[0].clone();
    (old.alternatives, old.downloads) = Default::default();
    let mut s = state(vec![dir.clone()]);
    s.config.recipes = vec![old.clone()];
    update(&mut s, Msg::OpenModelWizard);
    update(&mut s, Msg::ModelFoldersFound(vec![]));
    let index = FileIndex::build(&[tmp.path().to_path_buf()]);
    update(&mut s, Msg::ModelFilesIndexed(vec![], Box::new(index)));
    let w = s.model_wizard.clone().unwrap();
    let cat = catalogue();
    let lists = ModelLists::of(&s.config.recipes, &cat, std::slice::from_ref(&dir), &w.sizes, w.index.as_deref());
    let (_, needs) = &lists.installed[0];
    // two files missing, the VAE found elsewhere; no size (so no fit)
    assert!(needs.update && needs.unavailable, "{needs:?}");
    assert_eq!((needs.missing, needs.links.len()), (2, 1));
    assert_eq!(needs.total_bytes, None);
    assert_eq!(needs.fits(24.0), None);
    // installed again: replaced by the built-in, under the same name
    let mut w = w;
    w.dir = dir.clone();
    w.picked = [builtin.name.clone()].into();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    update(&mut s, Msg::ModelWizardGo);
    assert_eq!(s.config.recipes.len(), 1);
    assert_eq!(s.config.recipes[0], builtin);
    assert!(s.messages.iter().any(|m| m.text.contains("1 model updated")), "{:?}", s.messages);
    // its VAE and text encoder linked from koboldcpp/, not downloaded
    assert!(tmp.path().join(&builtin.files["vae"]).is_file());
    assert!(tmp.path().join(&builtin.files["llm"]).is_file());
}

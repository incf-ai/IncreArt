//! **Set up models**: folders found, presets picked, what's fetched where.

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
    let needs = PresetNeeds::of(first, &dirs, &w.sizes);
    assert!(needs.downloads.is_empty() && needs.total_bytes == Some(12) && !needs.unavailable, "{needs:?}");
    assert_eq!(needs.fits(1.0), Some(true));
    let second = &catalogue()[1];
    let needs = PresetNeeds::of(second, &dirs, &w.sizes);
    assert!(!needs.downloads.is_empty() && needs.download_bytes.is_none(), "sizes not known yet");
    for d in &needs.downloads {
        update(&mut s, Msg::DownloadSize(d.url.clone(), Some(1_000)));
    }
    let w = s.model_wizard.clone().unwrap();
    assert_eq!(PresetNeeds::of(second, &dirs, &w.sizes).download_bytes, Some(1_000 * needs.downloads.len() as u64));
    // picked, in my folder
    let mut w = s.model_wizard.clone().unwrap();
    w.dir = mine.clone();
    w.picked = [first.name.clone(), second.name.clone()].into();
    update(&mut s, Msg::EditModelWizard(Box::new(w)));
    let fx = update(&mut s, Msg::ModelWizardGo);
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
    let lists = ModelLists::of(&recipes, &catalogue, &[], &sizes);
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
    // fits a GPU: sizes not known fit none
    w.kind = ModelKind::All;
    w.fits_only = true;
    assert!(w.shown(lists.available.clone(), &[24.0]).is_empty());
    // order: by name; largest first, sizes not known last
    w.fits_only = false;
    w.sort = ModelSort::Name;
    let by_name = names(&w.shown(lists.available.clone(), &[]));
    let mut sorted = by_name.clone();
    sorted.sort_by_key(|n| n.to_lowercase());
    assert_eq!(by_name, sorted);
    let mut rows = lists.available.clone();
    rows[3].1.total_bytes = Some(9);
    rows[5].1.total_bytes = Some(20);
    w.sort = ModelSort::LargeFirst;
    let big = w.shown(rows.clone(), &[]);
    assert_eq!((big[0].0.name.as_str(), big[1].0.name.as_str()), (rows[5].0.name.as_str(), rows[3].0.name.as_str()));
    w.sort = ModelSort::SmallFirst;
    assert_eq!(w.shown(rows, &[])[0].1.total_bytes, Some(9));
}

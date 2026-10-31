//! The core against the generated sample library (tests/common/sample.rs,
//! written without the core): every file parses,
//! re-serializes byte for byte, and the scan matches what the generator
//! wrote.

mod common;

use std::fs;

use increart_core::config::Config;
use increart_core::model::*;
use increart_core::scan::{IDEA_FILE, PROJECT_FILE, RUN_FILE};
use increart_core::{Error, IdeaLoc, Library, Store, Verdict, fsutil, png, rsnfmt};

fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    common::sample_copy(&dir.path().join("lib"));
    dir
}

#[test]
fn opens_and_scans() {
    let tmp = sample();
    let root = tmp.path().join("lib");
    let mut lib = Library::open(&root).unwrap();
    // the usage counts are checked after opening (by the GUI, in the background)
    lib.check_all_usage().unwrap();
    assert_eq!(lib.link_mode(), LinkMode::Symlink);
    assert!(lib.problems().is_empty(), "{:?}", lib.problems());
    upgraded(&lib);

    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    assert_eq!(ix.ideas.len(), 12);
    let count = |pred: fn(&IdeaLoc) -> bool| ix.ideas.iter().filter(|e| pred(&e.loc)).count();
    assert_eq!(count(|l| *l == IdeaLoc::Unreviewed), 5);
    assert_eq!(count(|l| *l == IdeaLoc::Discarded), 1);
    assert_eq!(count(|l| matches!(l, IdeaLoc::Library(_))), 6);
    let papercut = ix.ideas.iter().find(|e| e.idea.title == "Ref: paper cut style").unwrap();
    assert_eq!(papercut.loc, IdeaLoc::Library("styles/paper".into()));
    assert_eq!(ix.library_folders, ["places", "styles", "styles/paper"].map(std::path::PathBuf::from));
    assert_eq!(ix.project_folders, ["clients", "clients/acme", "experiments"].map(std::path::PathBuf::from));

    assert_eq!(ix.projects.len(), 3);
    let fox = ix.projects.iter().find(|p| p.name() == "fox-cafe").unwrap();
    assert_eq!(fox.runs.len(), 2);
    assert_eq!(fox.runs[1].seed_count(), 32);
    assert_eq!(fox.outputs.len(), 14);
    let good = fox.outputs.iter().filter(|o| o.verdict == Some(Verdict::Good)).count();
    assert_eq!(good, 2);
    assert!(fox.outputs.iter().all(|o| o.image.store == Store::Output));

    let fish = ix.projects.iter().find(|p| p.name() == "glass-fish").unwrap();
    let Mode::Edit { init: Some(init), strength } = &fish.runs[0].mode else { panic!("edit run") };
    assert_eq!(*strength, 0.75);
    assert_eq!(init.store, Store::Input);
    assert_eq!(ix.inbox_drop.len(), 2);

    let from_fox = ix.ideas.iter().find(|e| matches!(e.idea.source, IdeaSource::FromOutput { .. })).unwrap();
    let IdeaSource::FromOutput { image } = &from_fox.idea.source else { unreachable!() };
    assert!(lib.image_path(image).is_file());
}

#[test]
fn every_chunk_parses() {
    let tmp = sample();
    let lib = Library::open(&tmp.path().join("lib")).unwrap();
    for id in lib.store_ids(Store::Input).unwrap() {
        let src = lib.source_info(&id).unwrap().expect("increart:source");
        assert_eq!(src.version, 1);
        assert!(src.imported.is_some());
    }
    let outputs = lib.store_ids(Store::Output).unwrap();
    assert_eq!(outputs.len(), 35);
    for id in &outputs {
        let recipe = lib.recipe(id).unwrap().expect("increart:recipe");
        assert_eq!(recipe.model.files.len(), 3);
        let path = lib.image_path(&increart_core::ImageRef::output(id.clone()));
        let bytes = fs::read(&path).unwrap();
        assert_eq!(png::text(&bytes, png::KEY_ID).unwrap().as_deref(), Some(id.as_str()));
        assert!(png::text(&bytes, "parameters").unwrap().is_some());
    }
}

/// The generator writes the same RSN the core writes, so a file read and
/// written back is unchanged.
#[test]
fn rsn_files_roundtrip_byte_for_byte() {
    let tmp = sample();
    let root = tmp.path().join("lib");
    let mut checked = 0;
    for p in fsutil::walk(&root).unwrap() {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let text = match name.as_str() {
            IDEA_FILE | PROJECT_FILE | RUN_FILE | "library.rsn" | "config.rsn" => fs::read_to_string(&p).unwrap(),
            _ => continue,
        };
        // library.rsn and config.rsn start with a comment line.
        let body: String = text.lines().filter(|l| !l.starts_with("//")).map(|l| format!("{l}\n")).collect();
        let again = match name.as_str() {
            IDEA_FILE => rsnfmt::to_string(&rsnfmt::from_str::<Idea>(&body).unwrap()),
            PROJECT_FILE => rsnfmt::to_string(&rsnfmt::from_str::<Project>(&body).unwrap()),
            RUN_FILE => rsnfmt::to_string(&rsnfmt::from_str::<Run>(&body).unwrap()),
            "library.rsn" => rsnfmt::to_string(&rsnfmt::from_str::<LibraryMeta>(&body).unwrap()),
            _ => rsnfmt::to_string(&rsnfmt::from_str::<Config>(&body).unwrap()),
        }
        .unwrap();
        assert_eq!(format!("{again}\n"), body, "{}", p.display());
        checked += 1;
    }
    assert_eq!(checked, 12 + 3 + 8 + 2);

    let lib = Library::open(&root).unwrap();
    for id in lib.store_ids(Store::Output).unwrap() {
        let path = lib.image_path(&increart_core::ImageRef::output(id.clone()));
        let text = png::text(&fs::read(&path).unwrap(), png::KEY_RECIPE).unwrap().unwrap();
        let recipe: Recipe = rsnfmt::from_str(&text).unwrap();
        assert_eq!(rsnfmt::to_string(&recipe).unwrap(), text);
    }
}

#[test]
fn sample_config_loads() {
    let tmp = sample();
    let (config, warnings) = Config::load(&tmp.path().join("lib/config/config.rsn")).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(config.resolutions.len(), 6);
    assert_eq!(config.default_resolution().unwrap().name, "Square");
}

#[test]
fn second_instance_is_refused() {
    let tmp = sample();
    let root = tmp.path().join("lib");
    let _lib = Library::open(&root).unwrap();
    assert!(matches!(Library::open(&root), Err(Error::Locked(_))));
}

#[test]
fn pixel_index_is_built_and_checked() {
    let tmp = sample();
    let root = tmp.path().join("lib");
    {
        let lib = Library::open(&root).unwrap();
        let first = &lib.store_ids(Store::Input).unwrap()[0];
        let path = lib.image_path(&increart_core::ImageRef::input(first.clone()));
        let hash = png::text(&fs::read(path).unwrap(), png::KEY_PIXELS).unwrap().unwrap();
        assert_eq!(lib.find_pixels(Store::Input, &hash), Some(first));
    }
    let cache: PixelIndexFile = rsnfmt::read(&root.join("cache/pixel-index.rsn")).unwrap();
    assert_eq!(cache.input.len() + cache.output.len(), 45);

    // A store file whose increart:pixels lies is reported on (re)indexing.
    let victim = root.join("output_images").join(cache.output.values().next().unwrap().to_string() + ".png");
    let bytes = fs::read(&victim).unwrap();
    let forged = png::set_chunks(&bytes, &[png::Chunk::text(png::KEY_PIXELS, "0000")]).unwrap();
    fs::write(&victim, forged).unwrap();
    fs::remove_file(root.join("cache/pixel-index.rsn")).unwrap();
    let lib = Library::open(&root).unwrap();
    assert_eq!(lib.problems().len(), 1, "{:?}", lib.problems());
    assert!(lib.problems()[0].contains("increart:pixels 0000"));
}

/// The generator writes no `increart:blake3` and no recipe `inputs`: opening
/// adds them (once), leaving the pixels and the other chunks alone.
fn upgraded(lib: &Library) {
    let root = lib.root();
    let meta: LibraryMeta = rsnfmt::read(&root.join("library.rsn")).unwrap();
    assert_eq!(meta.metadata, METADATA_VERSION);
    let mut with_inputs = 0;
    for store in [Store::Input, Store::Output] {
        for id in lib.store_ids(store).unwrap() {
            let r = ImageRef { id: id.clone(), store };
            let bytes = fs::read(lib.image_path(&r)).unwrap();
            let px = increart_core::import::decode_pixels(&bytes).unwrap();
            assert_eq!(png::text(&bytes, png::KEY_BLAKE3).unwrap().unwrap(), px.blake3(), "{id}");
            assert_eq!(png::text(&bytes, png::KEY_PIXELS).unwrap().unwrap(), px.hash(), "{id}");
            let usage = png::text(&bytes, png::KEY_USAGE).unwrap().unwrap();
            assert!(png::UsageCounts::parse(&usage).is_some(), "{id}: {usage}");
            if store == Store::Output {
                let recipe = lib.recipe(&id).unwrap().unwrap();
                assert_eq!(
                    recipe.inputs.len(),
                    recipe.refs.len() + usize::from(matches!(recipe.mode, Mode::Edit { init: Some(_), .. })),
                    "{id}"
                );
                for i in &recipe.inputs {
                    assert_eq!(i.blake3, lib.blake3_of(&i.image).unwrap());
                    assert!(root.join(&i.file).is_file(), "{}", i.file);
                }
                with_inputs += usize::from(!recipe.inputs.is_empty());
            }
        }
    }
    assert!(with_inputs > 0, "the sample has outputs made from other images");
}

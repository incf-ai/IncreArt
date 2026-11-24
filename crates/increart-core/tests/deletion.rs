//! Deleting (DESIGN.md, Deleting for good): folders go to a Discarded first,
//! restorable; what's deleted from there, and Clean up, is overwritten and
//! deleted, never sent to a system trash. Each library test runs in the
//! link mode `INCREART_LINK_MODE` (or the probe) picks and in hardlink and
//! copy mode, so the store images links share data with stay whole.
#![allow(clippy::disallowed_methods)] // fixtures on disk: no overlay here

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgb, RgbImage};
use increart_core::model::*;
use increart_core::{Error, Id, IdeaLoc, ImageRef, Library, NewIdea, NewRun, Store, Tree, Verdict, fsutil, link, png};

// ------------------------------------------------------------------ fixtures

fn sdserver_png(seed: u8) -> Vec<u8> {
    let img = RgbImage::from_fn(64, 64, |x, y| Rgb([(x * 4) as u8 ^ seed, (y * 4) as u8, seed]));
    let mut plain = Vec::new();
    DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut plain), image::ImageFormat::Png).unwrap();
    png::set_chunks(&plain, &[png::Chunk::text("parameters", &format!("a fox\nSeed: {seed}"))]).unwrap()
}

fn new_run(seeds: u64) -> NewRun {
    let files = BTreeMap::from([(
        "diffusion_model".to_string(),
        ModelFile { id: "diffusion_models/q.safetensors".into(), sha256: "ab".repeat(32) },
    )]);
    NewRun {
        sweep: None,
        label: "r01".into(),
        prompt: "a fox".into(),
        negative_prompt: String::new(),
        alpha: false,
        width: 64,
        height: 64,
        sample: Sample {
            method: "euler".into(),
            scheduler: "simple".into(),
            steps: 20,
            cfg: 6.0,
            flow_shift: None,
            custom_sigmas: vec![],
        },
        mode: Mode::Generate,
        refs: vec![],
        model: ModelSelection { recipe: "qwen-image-21".into(), files, loras: vec![] },
        source_idea: None,
        first_seed: 0,
        seeds,
        queued_anyway: false,
    }
}

fn recipe(run: &Run, seed: u64) -> Recipe {
    Recipe {
        prompt_original: run.prompt.clone(),
        prompt_sent: run.prompt.clone(),
        negative_prompt: String::new(),
        width: run.width,
        height: run.height,
        seed,
        alpha: false,
        sample: run.sample.clone(),
        mode: run.mode.clone(),
        refs: vec![],
        parent: None,
        source_idea: None,
        rewrite: None,
        model: run.model.clone(),
        sdcpp: "fd21ecf".into(),
        increart: env!("CARGO_PKG_VERSION").into(),
        device: "ROCm0".into(),
        elapsed_s: 1.0,
        queued_s: 0.0,
        step_s: 0.0,
        inputs: vec![],
    }
}

/// `None`: the mode `INCREART_LINK_MODE` or the probe picks (CI runs every
/// OS, and Windows in hardlink and copy mode, through it).
const MODES: [Option<LinkMode>; 3] = [None, Some(LinkMode::Hardlink), Some(LinkMode::Copy)];

fn setup(mode: Option<LinkMode>) -> (tempfile::TempDir, Library) {
    common::init();
    let tmp = tempfile::tempdir().unwrap();
    let lib = Library::create(&tmp.path().join("lib"), mode).unwrap();
    (tmp, lib)
}

/// A project (in `folder`, of the project tree) with `n` generated images;
/// returns its ID and the images' IDs and links.
fn project_with(lib: &mut Library, folder: &Path, name: &str, n: u64) -> (Id, Vec<(Id, PathBuf)>) {
    let idea = lib.new_idea(NewIdea::text(name, "a fox")).unwrap();
    let (project, _) = lib.create_project(folder, Some(name), std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(n)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let outs = (0..n)
        .map(|seed| lib.write_output(&run, 0, seed, &sdserver_png(seed as u8 + 1), &recipe(&def, seed)).unwrap())
        .collect();
    (project, outs)
}

/// Every store file's bytes: deleting links and entries must leave them be.
fn stores(lib: &Library) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for dir in ["input_images", "output_images"] {
        for p in fsutil::walk(&lib.path(dir)).unwrap() {
            if p.is_file() {
                out.insert(p.clone(), fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// The image a link resolves to, checking it reaches the store file.
fn resolves(lib: &Library, abs: &Path) -> ImageRef {
    let r = link::read_ref(abs, Store::Output).unwrap().unwrap_or_else(|| panic!("{} is not a link", abs.display()));
    assert_eq!(fs::read(abs).unwrap(), fs::read(lib.image_path(&r)).unwrap(), "{}", abs.display());
    r
}

fn clean(lib: &Library) {
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    let report = common::verify(lib.root());
    assert!(report.trim_end().ends_with("problems: none"), "{report}");
}

fn ids(images: &[ImageRef]) -> Vec<Id> {
    let mut v: Vec<_> = images.iter().map(|r| r.id.clone()).collect();
    v.sort();
    v
}

// ------------------------------------------------------------------ overwriting

#[test]
fn overwriting_replaces_every_byte_and_keeps_the_length() {
    common::init();
    let d = tempfile::tempdir().unwrap();
    // more than one buffer's worth, not a whole number of them
    for len in [0usize, 1, 4096, 200_001] {
        let p = d.path().join(format!("{len}.png"));
        fs::write(&p, vec![7u8; len]).unwrap();
        assert!(fsutil::overwrite(&p), "{len}");
        let after = fs::read(&p).unwrap();
        assert_eq!(after.len(), len);
        // random bytes: about 1 in 256 is a 7 by chance
        assert!(after.iter().filter(|b| **b == 7).count() <= len / 64 + 4, "{len}");
    }
}

#[test]
fn overwriting_leaves_data_another_name_shares() {
    common::init();
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store.png");
    fs::write(&store, b"pixels").unwrap();
    let hard = d.path().join("hard.png");
    fs::hard_link(&store, &hard).unwrap();
    assert!(!fsutil::overwrite(&hard), "a hard link shares the store's data");
    assert!(!fsutil::overwrite(&store), "the store has two names too");
    assert_eq!(fs::read(&store).unwrap(), b"pixels");
    // a symlink is never followed
    let soft = d.path().join("soft.png");
    if link::symlink(&store, &soft).is_ok() {
        assert!(!fsutil::overwrite(&soft));
        assert_eq!(fs::read(&store).unwrap(), b"pixels");
    }
    // a folder, or nothing there, isn't a file to overwrite
    assert!(!fsutil::overwrite(d.path()));
    assert!(!fsutil::overwrite(&d.path().join("missing")));
    // once the other name is gone, the data is the one name's
    fs::remove_file(&hard).unwrap();
    assert!(fsutil::overwrite(&store));
    assert_ne!(fs::read(&store).unwrap(), b"pixels");
}

#[test]
fn shredding_deletes_files_folders_and_links_but_not_what_they_share() {
    common::init();
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("store.png");
    fs::write(&store, vec![9u8; 70_000]).unwrap();
    let dir = d.path().join("project");
    fs::create_dir_all(dir.join("reviewed/good")).unwrap();
    fs::hard_link(&store, dir.join("reviewed/good/hard.png")).unwrap();
    let soft = link::symlink(&store, &dir.join("reviewed/soft.png")).is_ok();
    fs::copy(&store, dir.join("copy.png")).unwrap();
    fs::write(dir.join("run.rsn"), b"prompt").unwrap();
    fs::create_dir(dir.join("empty")).unwrap();
    fsutil::shred(&dir).unwrap();
    assert!(!dir.exists());
    assert_eq!(fs::read(&store).unwrap(), vec![9u8; 70_000], "symlink {soft}");
    // one file
    let one = d.path().join("one.rsn");
    fs::write(&one, b"x").unwrap();
    fsutil::shred(&one).unwrap();
    assert!(!one.exists());
    // a symlink alone: removed, its target kept
    let alone = d.path().join("alone.png");
    if link::symlink(&store, &alone).is_ok() {
        fsutil::shred(&alone).unwrap();
        assert!(fs::symlink_metadata(&alone).is_err());
        assert_eq!(fs::read(&store).unwrap().len(), 70_000);
    }
    // nothing there
    assert!(fsutil::shred(&d.path().join("missing")).is_err());
}

/// The overwrite lands in the file itself, as a handle open before the
/// delete still sees (Unix: deleting a file another handle holds keeps its
/// data until that handle closes).
#[cfg(unix)]
#[test]
fn a_shredded_file_holds_random_bytes_when_deleted() {
    use std::io::Read;
    common::init();
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("a.png");
    fs::write(&p, vec![7u8; 200_000]).unwrap();
    let mut held = fs::File::open(&p).unwrap();
    fsutil::shred(&p).unwrap();
    assert!(!p.exists());
    let mut after = vec![];
    held.read_to_end(&mut after).unwrap();
    assert_eq!(after.len(), 200_000);
    assert!(after.iter().filter(|b| **b == 7).count() < 10_000);
}

// ------------------------------------------------------------------ collections

#[test]
fn collection_folders_are_discarded_restored_and_deleted_for_good() {
    for mode in MODES {
        collection_folders(mode);
    }
}

fn collection_folders(mode: Option<LinkMode>) {
    let (_tmp, mut lib) = setup(mode);
    let (_, outs) = project_with(&mut lib, Path::new(""), "fox", 3);
    let before = stores(&lib);
    // a/b/best holding two images, one of them in a subfolder too
    lib.create_folder(Tree::Collections, Path::new(""), Some("a")).unwrap();
    lib.create_folder(Tree::Collections, Path::new("a"), Some("b")).unwrap();
    lib.create_folder(Tree::Collections, Path::new("a/b"), Some("best")).unwrap();
    lib.create_folder(Tree::Collections, Path::new("a/b/best"), Some("inner")).unwrap();
    lib.add_to_collection(Path::new("a/b/best"), &outs[0].0).unwrap();
    lib.add_to_collection(Path::new("a/b/best/inner"), &outs[1].0).unwrap();
    let ctx = format!("{:?}", lib.link_mode());

    // to Discarded, whole, noting where it was
    assert_eq!(lib.delete_node(Tree::Collections, Path::new("a/b/best")).unwrap(), vec![]);
    assert!(!lib.path("collections/a/b/best").exists(), "{ctx}");
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{ctx}: {:?}", ix.problems);
    assert!(ix.collection_links.is_empty() && ix.collection_folders == [Path::new("a"), Path::new("a/b")], "{ctx}");
    let [d] = &ix.discarded_collections[..] else { panic!("{ctx}: {:?}", ix.discarded_collections) };
    assert_eq!(d.dir, Path::new("discarded_collections/best"), "{ctx}");
    assert_eq!((d.discarded.name.as_str(), d.discarded.from.as_path()), ("best", Path::new("a/b")), "{ctx}");
    assert_eq!((ids(&d.images), d.folders), (vec![outs[0].0.clone(), outs[1].0.clone()], 1), "{ctx}");
    assert!(ix.discarded_input_collections.is_empty());
    for p in fsutil::walk(&lib.path(&d.dir)).unwrap() {
        if p.extension().is_some_and(|e| e == "png") {
            resolves(&lib, &p);
        }
    }
    // its images are kept through Clean up while it's there, though their
    // project's links are gone
    for (_, l) in &outs[..2] {
        let gone = lib.delete_output_link(l).unwrap();
        lib.delete_discarded_output(&gone).unwrap();
    }
    assert_eq!(ids(&lib.unreferenced().unwrap()), Vec::<Id>::new(), "{ctx}");
    clean(&lib);

    // a second folder of the same name: numbered in Discarded
    lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
    lib.add_to_collection(Path::new("best"), &outs[2].0).unwrap();
    lib.delete_node(Tree::Collections, Path::new("best")).unwrap();
    let ix = lib.scan().unwrap();
    let dirs: Vec<_> = ix.discarded_collections.iter().map(|d| d.dir.clone()).collect();
    assert_eq!(dirs.len(), 2, "{ctx}");
    assert!(dirs.contains(&PathBuf::from("discarded_collections/best 2")), "{ctx}: {dirs:?}");
    let second = PathBuf::from("discarded_collections/best 2");

    // back where it was, its folders made again; a name taken: numbered
    lib.delete_node(Tree::Collections, Path::new("a")).unwrap();
    lib.delete_discarded_folder(Tree::Collections, Path::new("discarded_collections/a")).unwrap();
    assert!(!lib.path("collections/a").exists() && !lib.path("discarded_collections/a").exists(), "{ctx}");
    assert_eq!(
        lib.restore_folder(Tree::Collections, Path::new("discarded_collections/best")).unwrap(),
        Path::new("a/b/best"),
        "{ctx}"
    );
    assert!(!lib.path("collections/a/b/best").join(increart_core::scan::DISCARDED_FILE).exists());
    lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
    assert_eq!(lib.restore_folder(Tree::Collections, &second).unwrap(), Path::new("best 2"), "{ctx}");
    let ix = lib.scan().unwrap();
    assert!(ix.discarded_collections.is_empty() && ix.problems.is_empty(), "{ctx}: {:?}", ix.problems);
    let mut back: Vec<_> = ix.collection_links.iter().map(|l| (l.folder.clone(), l.image.id.clone())).collect();
    back.sort();
    let mut expect = vec![
        (PathBuf::from("a/b/best"), outs[0].0.clone()),
        (PathBuf::from("a/b/best/inner"), outs[1].0.clone()),
        (PathBuf::from("best 2"), outs[2].0.clone()),
    ];
    expect.sort();
    assert_eq!(back, expect, "{ctx}");
    for l in &ix.collection_links {
        resolves(&lib, &lib.path(&l.path));
    }
    clean(&lib);

    // for good: the links go, the store images stay until Clean up
    lib.delete_node(Tree::Collections, Path::new("a")).unwrap();
    lib.delete_discarded_folder(Tree::Collections, Path::new("discarded_collections/a")).unwrap();
    assert!(!lib.path("discarded_collections/a").exists(), "{ctx}");
    assert_eq!(stores(&lib), before, "{ctx}: store images untouched");
    assert_eq!(ids(&lib.unreferenced().unwrap()), vec![outs[0].0.clone(), outs[1].0.clone()], "{ctx}");
    let mut cleaned = lib.clean_up().unwrap();
    cleaned.sort();
    assert_eq!(ids(&cleaned), vec![outs[0].0.clone(), outs[1].0.clone()], "{ctx}");
    assert!(!lib.image_path(&ImageRef::output(outs[0].0.clone())).exists(), "{ctx}");
    let kept = lib.image_path(&ImageRef::output(outs[2].0.clone()));
    assert_eq!(fs::read(&kept).unwrap(), before[&kept], "{ctx}: an image still collected stays whole");
    clean(&lib);
}

#[test]
fn a_collection_link_deleted_leaves_its_image() {
    for mode in MODES {
        let (_tmp, mut lib) = setup(mode);
        let (_, outs) = project_with(&mut lib, Path::new(""), "fox", 1);
        let before = stores(&lib);
        lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
        let at = lib.add_to_collection(Path::new("best"), &outs[0].0).unwrap();
        let rel = at.strip_prefix("collections").unwrap_or(&at).to_path_buf();
        lib.delete_node(Tree::Collections, &rel).unwrap();
        assert!(!lib.path(&at).exists() && lib.path("collections/best").is_dir());
        assert_eq!(stores(&lib), before, "{:?}", lib.link_mode());
        resolves(&lib, &lib.path(&outs[0].1));
        assert!(lib.scan().unwrap().discarded_collections.is_empty(), "an entry isn't kept");
        clean(&lib);
    }
}

#[test]
fn input_collection_folders_are_discarded_restored_and_deleted_for_good() {
    let (_tmp, mut lib) = setup(None);
    let mut ideas = vec![];
    for t in ["Hut", "Lake"] {
        let id = lib.new_idea(NewIdea { loc: IdeaLoc::Library, ..NewIdea::text(t, "") }).unwrap();
        ideas.push(id);
    }
    lib.create_folder(Tree::InputCollections, Path::new(""), Some("places")).unwrap();
    lib.create_folder(Tree::InputCollections, Path::new("places"), Some("alpine")).unwrap();
    lib.collect_ideas(Path::new("places"), &ideas[..1]).unwrap();
    lib.collect_ideas(Path::new("places/alpine"), &ideas[1..]).unwrap();

    lib.delete_node(Tree::InputCollections, Path::new("places/alpine")).unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    assert!(ix.idea_collections(&ideas[1]).is_empty(), "out of the collections");
    assert_eq!(ix.input_collection_folders, [Path::new("places")]);
    let [d] = &ix.discarded_input_collections[..] else { panic!("{:?}", ix.discarded_input_collections) };
    assert_eq!(d.dir, Path::new("input/discarded_collections/alpine"));
    assert_eq!((d.discarded.from.as_path(), d.ideas.as_slice(), d.folders), (Path::new("places"), &ideas[1..], 0));
    assert!(ix.discarded_collections.is_empty());
    // the wrong tree, or a path that isn't a discarded folder: refused
    assert!(lib.restore_folder(Tree::Collections, &d.dir).is_err());
    assert!(matches!(lib.delete_discarded_folder(Tree::Projects, &d.dir), Err(Error::Refused(_))));
    for bad in ["input/discarded_collections", "input/discarded_collections/../collections", "places/alpine"] {
        assert!(matches!(lib.delete_discarded_folder(Tree::InputCollections, Path::new(bad)), Err(Error::NotFound(_))));
    }
    let dir = d.dir.clone();

    // back, its parent gone and made again
    lib.delete_node(Tree::InputCollections, Path::new("places")).unwrap();
    lib.delete_discarded_folder(Tree::InputCollections, Path::new("input/discarded_collections/places")).unwrap();
    assert_eq!(lib.restore_folder(Tree::InputCollections, &dir).unwrap(), Path::new("places/alpine"));
    let ix = lib.scan().unwrap();
    assert_eq!(ix.idea_collections(&ideas[1]), [Path::new("places/alpine")]);
    assert!(ix.idea_collections(&ideas[0]).is_empty(), "its entry went with the folder deleted for good");

    // for good: the ideas stay in the input library
    lib.delete_node(Tree::InputCollections, Path::new("places")).unwrap();
    lib.delete_discarded_folder(Tree::InputCollections, Path::new("input/discarded_collections/places")).unwrap();
    for id in &ideas {
        assert_eq!(lib.find_idea(id).unwrap().0, IdeaLoc::Library);
    }
    let ix = lib.scan().unwrap();
    assert!(ix.discarded_input_collections.is_empty() && ix.idea_links.is_empty());
    clean(&lib);
}

// ------------------------------------------------------------------ projects

#[test]
fn project_folders_send_their_projects_to_discarded() {
    for mode in MODES {
        project_folders(mode);
    }
}

fn project_folders(mode: Option<LinkMode>) {
    let (_tmp, mut lib) = setup(mode);
    let ctx = format!("{mode:?}");
    lib.create_folder(Tree::Projects, Path::new(""), Some("clients")).unwrap();
    lib.create_folder(Tree::Projects, Path::new("clients"), Some("acme")).unwrap();
    let (p1, o1) = project_with(&mut lib, Path::new("clients/acme"), "logo", 2);
    let (p2, _) = project_with(&mut lib, Path::new("clients"), "poster", 1);
    let before = stores(&lib);
    // a file of the user's in the folder
    fs::write(lib.path("projects/clients/notes.txt"), b"call back").unwrap();

    lib.delete_node(Tree::Projects, Path::new("clients")).unwrap();
    assert!(!lib.path("projects/clients").exists(), "{ctx}: the emptied folders go, the stray file with them");
    let ix = lib.scan().unwrap();
    assert!(ix.projects.is_empty() && ix.project_folders.is_empty(), "{ctx}");
    let mut from: Vec<_> =
        ix.discarded_projects.iter().map(|d| (d.entry.project.id.clone(), d.discarded.from.clone())).collect();
    from.sort();
    let mut expect = vec![(p1.clone(), PathBuf::from("clients/acme")), (p2.clone(), PathBuf::from("clients"))];
    expect.sort();
    assert_eq!(from, expect, "{ctx}");
    assert!(lib.unreferenced().unwrap().is_empty(), "{ctx}: kept for a restore");
    clean(&lib);

    // back in their folders, made again
    assert_eq!(lib.restore_project(&p1).unwrap(), Path::new("clients/acme/logo"), "{ctx}");
    let ix = lib.scan().unwrap();
    let p = ix.project(&p1).unwrap();
    assert_eq!(p.outputs.len(), 2, "{ctx}");
    for o in &p.outputs {
        resolves(&lib, &lib.path(&o.path));
    }

    // for good: the folder goes, the store images stay until Clean up
    lib.delete_discarded_project(&p2).unwrap();
    lib.delete_node(Tree::Projects, Path::new("clients/acme/logo")).unwrap();
    lib.delete_discarded_project(&p1).unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.discarded_projects.is_empty(), "{ctx}");
    assert!(fsutil::walk(&lib.path("discarded_projects")).unwrap().is_empty(), "{ctx}");
    assert_eq!(stores(&lib), before, "{ctx}: store images untouched");
    assert!(matches!(lib.delete_discarded_project(&p1), Err(Error::NotFound(_))));
    let gone = lib.unreferenced().unwrap();
    assert_eq!(gone.len(), 3, "{ctx}");
    lib.clean_up().unwrap();
    for (id, _) in &o1 {
        assert!(!lib.image_path(&ImageRef::output(id.clone())).exists(), "{ctx}");
    }
    assert!(lib.scan().unwrap().problems.is_empty());
    clean(&lib);
}

#[test]
fn deleted_ideas_and_outputs_leave_the_store_whole() {
    for mode in MODES {
        let (_tmp, mut lib) = setup(mode);
        let ctx = format!("{mode:?}");
        let (project, outs) = project_with(&mut lib, Path::new(""), "fox", 2);
        let before = stores(&lib);
        let bad = lib.review_output(&outs[0].1, Some(Verdict::Bad)).unwrap();
        let gone = lib.delete_output_link(&bad).unwrap();
        lib.delete_discarded_output(&gone).unwrap();
        assert!(!lib.path(&gone).exists(), "{ctx}");
        assert_eq!(stores(&lib), before, "{ctx}");
        resolves(&lib, &lib.path(&outs[1].1));
        let idea = lib.new_idea(NewIdea::text("Junk", "")).unwrap();
        lib.review_idea(&idea, Verdict::Bad).unwrap();
        lib.delete_discarded_idea(&idea).unwrap();
        assert!(lib.find_idea(&idea).is_err(), "{ctx}");
        assert_eq!(lib.scan().unwrap().project(&project).unwrap().outputs.len(), 1);
        assert_eq!(ids(&lib.unreferenced().unwrap()), vec![outs[0].0.clone()], "{ctx}");
        lib.clean_up().unwrap();
        assert!(!lib.image_path(&ImageRef::output(outs[0].0.clone())).exists(), "{ctx}");
        resolves(&lib, &lib.path(&outs[1].1));
        clean(&lib);
    }
}

// ------------------------------------------------------------------ ephemeral

/// Every file below `dir` with its bytes (symlinks: their targets).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for p in fsutil::walk(dir).unwrap() {
        let m = fs::symlink_metadata(&p).unwrap();
        let rel = p.strip_prefix(dir).unwrap().to_path_buf();
        if m.file_type().is_symlink() {
            out.insert(rel, fs::read_link(&p).unwrap().into_os_string().into_encoded_bytes());
        } else if m.is_file() {
            out.insert(rel, fs::read(&p).unwrap());
        } else {
            out.insert(rel, vec![]);
        }
    }
    out
}

/// An ephemeral library over one on disk deletes in RAM: the disk's files
/// keep their bytes, its folders and projects included.
#[test]
fn an_ephemeral_library_deletes_nothing_on_disk() {
    common::init();
    let tmp = tempfile::tempdir().unwrap();
    let disk = tmp.path().join("lib");
    {
        let mut lib = Library::create(&disk, None).unwrap();
        project_with(&mut lib, Path::new(""), "fox", 2);
        lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
        let out = lib.scan().unwrap().projects[0].outputs[0].image.id.clone();
        lib.add_to_collection(Path::new("best"), &out).unwrap();
        lib.create_folder(Tree::InputCollections, Path::new(""), Some("places")).unwrap();
    }
    let before = snapshot(&disk);

    let mut lib = Library::open_ephemeral(Some(&disk), false).unwrap();
    let project = lib.scan().unwrap().projects[0].project.id.clone();
    lib.delete_node(Tree::Projects, Path::new("fox")).unwrap();
    lib.delete_discarded_project(&project).unwrap();
    lib.delete_node(Tree::Collections, Path::new("best")).unwrap();
    lib.delete_discarded_folder(Tree::Collections, Path::new("discarded_collections/best")).unwrap();
    lib.delete_node(Tree::InputCollections, Path::new("places")).unwrap();
    let back = lib.restore_folder(Tree::InputCollections, Path::new("input/discarded_collections/places")).unwrap();
    assert_eq!(back, Path::new("places"));
    let ix = lib.scan().unwrap();
    assert!(ix.projects.is_empty() && ix.discarded_projects.is_empty() && ix.discarded_collections.is_empty());
    assert_eq!(lib.clean_up().unwrap().len(), 2);
    drop(lib);
    assert_eq!(snapshot(&disk), before);
}

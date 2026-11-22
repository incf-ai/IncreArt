//! Ephemeral libraries (DESIGN.md, Ephemeral mode): wholly in RAM, a
//! library on disk read and never written.
#![allow(clippy::disallowed_methods)] // fixtures on disk: no overlay here

mod common;

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgb, RgbImage};
use increart_core::model::*;
use increart_core::{Error, IdeaLoc, Library, NewIdea, NewRun, Store, Tree, Verdict, png, vfs};

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
        prompt: "a private fox".into(),
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

/// Every file below `dir` with its bytes (symlinks: their targets).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for p in increart_core::fsutil::walk(dir).unwrap() {
        let m = std::fs::symlink_metadata(&p).unwrap();
        let rel = p.strip_prefix(dir).unwrap().to_path_buf();
        if m.file_type().is_symlink() {
            out.insert(rel, std::fs::read_link(&p).unwrap().into_os_string().into_encoded_bytes());
        } else if m.is_file() {
            out.insert(rel, std::fs::read(&p).unwrap());
        } else {
            out.insert(rel, vec![]);
        }
    }
    out
}

/// Generates `n` outputs in a new project and run; returns the run's ID and
/// the outputs' links.
fn generate(lib: &mut Library, n: u64) -> (increart_core::id::Id, Vec<PathBuf>) {
    let idea = lib.new_idea(NewIdea { loc: IdeaLoc::Unreviewed, ..NewIdea::text("A fox", "a private fox") }).unwrap();
    lib.review_idea(&idea, Verdict::Good).unwrap();
    let (project, _) = lib.create_project(Path::new(""), Some("private"), std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(n)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let links = (0..n)
        .map(|seed| lib.write_output(&run, 0, seed, &sdserver_png(seed as u8), &recipe(&def, seed)).unwrap().1)
        .collect();
    (run, links)
}

#[test]
fn a_library_on_disk_is_read_and_never_written() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = tmp.path().join("lib");
    common::sample_copy(&disk);
    // as an instance leaves it: opened once (the lock file there)
    drop(Library::open(&disk).unwrap());
    let before = snapshot(&disk);

    let mut lib = Library::open_ephemeral(Some(&disk), false).unwrap();
    let root = lib.root().to_path_buf();
    assert!(vfs::in_ram(&root) && !root.starts_with(&disk));
    assert_eq!(lib.ram_lower().as_deref(), Some(disk.as_path()));
    let disk_projects = lib.scan().unwrap().projects.len();
    assert!(disk_projects > 0, "the library's projects are there");

    // an instance can't open it meanwhile, nor a second ephemeral session
    // be refused (shared locks)
    assert!(matches!(Library::open(&disk), Err(Error::Locked(_))));
    let second = Library::open_ephemeral(Some(&disk), false).unwrap();
    drop(second);

    // every kind of change
    let (run, links) = generate(&mut lib, 3);
    lib.review_output(&links[0], Some(Verdict::Good)).unwrap();
    lib.delete_output_link(&links[1]).unwrap();
    let first = lib.scan().unwrap().projects[0].dir.clone();
    lib.rename_node(Tree::Projects, first.strip_prefix("projects").unwrap(), "renamed").unwrap();
    lib.create_folder(Tree::Collections, Path::new(""), Some("faves")).unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    assert_eq!(ix.projects.len(), disk_projects + 1);
    assert!(ix.projects.iter().any(|p| p.runs.iter().any(|r| r.id == run)));
    assert!(lib.ram_used().unwrap() > 0);
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);

    // reopened in RAM: what was changed is there while the session lasts
    drop(lib);
    assert!(!vfs::in_ram(&root), "unmounted with the library");
    assert_eq!(snapshot(&disk), before, "the library on disk as it was");
    assert!(!std::fs::exists(&root).unwrap_or(false));
    // and it opens as before
    let lib = Library::open(&disk).unwrap();
    assert_eq!(lib.scan().unwrap().projects.len(), disk_projects);
}

#[test]
fn a_new_ephemeral_library_frees_its_oldest_images() {
    common::init();
    let mut lib = Library::open_ephemeral(None, false).unwrap();
    assert_eq!(lib.ram_lower(), None);
    let (run, links) = generate(&mut lib, 6);
    let ix = lib.scan().unwrap();
    let outs: Vec<_> = ix.projects[0].outputs.iter().map(|o| o.image.clone()).collect();
    assert_eq!(outs.len(), 6);
    // a collection link to the oldest, which goes too
    lib.create_folder(Tree::Collections, Path::new(""), Some("faves")).unwrap();
    lib.add_to_collection(Path::new("faves"), &outs[0].id).unwrap();
    for l in &links[..2] {
        increart_core::thumbs::ensure(
            lib.root(),
            &increart_core::link::read_ref(&lib.path(l), Store::Output).unwrap().unwrap(),
        )
        .unwrap();
    }

    let used = lib.ram_used().unwrap();
    assert!(lib.free_ram(used).unwrap().is_empty(), "within the budget: nothing");
    let gone = lib.free_ram(used - 1).unwrap();
    assert!(!gone.is_empty());
    let mut oldest: Vec<_> = outs.clone();
    oldest.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(gone[..], oldest[..gone.len()], "the oldest first");
    assert!(lib.ram_used().unwrap() < used);

    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    assert_eq!(ix.projects[0].outputs.len(), 6 - gone.len());
    assert!(ix.collection_links.iter().all(|c| c.image != gone[0]), "its collection link gone");
    let (_, r) = lib.find_run(&run).unwrap();
    let deleted: u64 = r.deleted.iter().map(|d| d.count).sum();
    assert_eq!(deleted as usize, gone.len(), "recorded as deleted: not generated again");
    assert!(!vfs::exists(increart_core::thumbs::path(lib.root(), &gone[0])));
    assert_eq!(lib.pending_seeds(&run).unwrap(), vec![]);

    // everything: as much as can go
    let all = lib.free_ram(0).unwrap();
    assert_eq!(gone.len() + all.len(), 6);
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
}

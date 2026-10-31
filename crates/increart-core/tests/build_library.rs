//! Building libraries entirely through the core API, then checking them with
//! the independent verifier (tests/common/verify.rs) and a rescan.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use chrono::TimeZone;
use image::{DynamicImage, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};
use increart_core::model::*;
use increart_core::{
    Disposal, Error, IdeaLoc, ImageRef, Library, NewIdea, NewRun, Original, Store, Tree, Verdict, container, fsutil,
    import, link, png,
};

fn t0() -> increart_core::id::Timestamp {
    chrono::Utc.with_ymd_and_hms(2026, 10, 24, 12, 0, 0).unwrap()
}

fn picture(w: u32, h: u32, seed: u8) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| Rgb([(x * 4) as u8 ^ seed, (y * 4) as u8, seed]))
}

fn png_bytes(img: DynamicImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn jpeg_bytes(img: RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

#[cfg(feature = "avif")]
fn avif_bytes(img: RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Avif).unwrap();
    out
}

/// A PNG with an `eXIf` block saying "rotate 90" (orientation 6).
fn rotated_png() -> Vec<u8> {
    let mut exif = b"MM\0\x2a\0\0\0\x08\0\x01".to_vec();
    exif.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
    exif.extend([0, 0, 0, 0]);
    let plain = png_bytes(DynamicImage::ImageRgb8(picture(32, 16, 1)));
    png::set_chunks(&plain, &[png::Chunk::exif(exif)]).unwrap()
}

/// A PNG tagged with a Display P3 ICC profile.
fn p3_png() -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = image::codecs::png::PngEncoder::new(&mut out);
    enc.set_icc_profile(moxcms::ColorProfile::new_display_p3().encode().unwrap()).unwrap();
    let img = picture(16, 16, 2);
    enc.write_image(img.as_raw(), 16, 16, image::ExtendedColorType::Rgb8).unwrap();
    out
}

/// What sd-server returns: a PNG with its own `parameters` chunk.
fn sdserver_png(seed: u8) -> Vec<u8> {
    let plain = png_bytes(DynamicImage::ImageRgb8(picture(64, 64, 100 + seed)));
    png::set_chunks(&plain, &[png::Chunk::text("parameters", &format!("a fox\nSeed: {seed}"))]).unwrap()
}

fn sample() -> Sample {
    Sample {
        method: "euler".into(),
        scheduler: "simple".into(),
        steps: 20,
        cfg: 6.0,
        flow_shift: Some(3.0),
        custom_sigmas: vec![],
    }
}

fn model() -> ModelSelection {
    let files = BTreeMap::from([(
        "diffusion_model".to_string(),
        ModelFile { id: "diffusion_models/q.safetensors".into(), sha256: "ab".repeat(32) },
    )]);
    ModelSelection { recipe: "qwen-image-21".into(), files, loras: vec![] }
}

fn new_run(refs: Vec<ImageRef>, mode: Mode, seeds: u64) -> NewRun {
    NewRun {
        sweep: None,
        label: "r01".into(),
        prompt: "a fox in a café".into(),
        negative_prompt: String::new(),
        alpha: false,
        width: 64,
        height: 64,
        sample: sample(),
        mode,
        refs,
        model: model(),
        source_idea: None,
        first_seed: 0,
        seeds,
    }
}

fn recipe(run: &Run, seed: u64, parent: Option<ImageRef>) -> Recipe {
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
        refs: run.refs.clone(),
        parent,
        source_idea: run.source_idea.clone(),
        rewrite: None,
        model: run.model.clone(),
        sdcpp: "fd21ecf".into(),
        increart: env!("CARGO_PKG_VERSION").into(),
        device: "ROCm0".into(),
        elapsed_s: 33.4,
        queued_s: 0.0,
        step_s: 0.0,
        inputs: vec![],
    }
}

fn original(name: &str) -> Original {
    Original {
        name: Some(name.into()),
        path: Some(format!("/home/user/Downloads/{name}")),
        mtime: Some(t0()),
        ..Default::default()
    }
}

fn setup(mode: Option<LinkMode>) -> (tempfile::TempDir, Library) {
    common::init();
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = Library::create(&tmp.path().join("lib"), mode).unwrap();
    lib.set_clock(t0);
    lib.set_disposal(Disposal::Directory(tmp.path().join("trash")));
    (tmp, lib)
}

/// The image a link resolves to, checking it really reaches the store file.
fn resolves(lib: &Library, rel: &Path, store: Store) -> ImageRef {
    let abs = lib.path(rel);
    let r = link::read_ref(&abs, store).unwrap().unwrap_or_else(|| panic!("{} is not a link", rel.display()));
    assert_eq!(fs::read(&abs).unwrap(), fs::read(lib.image_path(&r)).unwrap(), "{}", rel.display());
    r
}

#[test]
fn whole_library_through_the_api() {
    // symlinks whatever INCREART_LINK_MODE says: this test reads them as such
    let (tmp, mut lib) = setup(Some(LinkMode::Symlink));
    let root = lib.root().to_path_buf();
    assert_eq!(lib.link_mode(), LinkMode::Symlink);

    // ---- import
    let jpeg = jpeg_bytes(picture(40, 30, 7));
    let photo = lib.import_bytes(&jpeg, &original("photo.jpg")).unwrap();
    assert!(!photo.reused);
    let again = lib.import_bytes(&jpeg, &original("copy-of-photo.jpg")).unwrap();
    assert_eq!((again.image.clone(), again.reused), (photo.image.clone(), true));
    let photo = photo.image;
    let src = lib.source_info(&photo).unwrap().unwrap();
    assert_eq!((src.original_format.as_str(), src.original_name.as_deref()), ("jpeg", Some("photo.jpg")));
    assert_eq!(src.conversions, ["jpeg -> png"]);
    assert_eq!(src.original_sha256, increart_core::pixel::sha256_hex(&jpeg));

    let rotated = lib.import_bytes(&rotated_png(), &original("hut.png")).unwrap().image;
    let rot_file = fs::read(lib.image_path(&ImageRef::input(rotated.clone()))).unwrap();
    let rot_img = image::load_from_memory(&rot_file).unwrap();
    assert_eq!((rot_img.width(), rot_img.height()), (16, 32));
    assert!(png::read_chunks(&rot_file).unwrap().iter().any(|c| &c.kind == b"eXIf"), "EXIF kept");
    assert_eq!(lib.source_info(&rotated).unwrap().unwrap().conversions, ["exif-orientation: rotate 90"]);

    let p3 = lib.import_bytes(&p3_png(), &original("fish.png")).unwrap().image;
    let notes = lib.source_info(&p3).unwrap().unwrap().conversions;
    assert!(notes.len() == 1 && notes[0].starts_with("icc: ") && notes[0].ends_with(" -> sRGB"), "{notes:?}");

    let rgba = RgbaImage::from_fn(8, 8, |x, _| Rgba([200, 10, 10, (x * 30) as u8]));
    let sticker = lib.import_bytes(&png_bytes(DynamicImage::ImageRgba8(rgba)), &original("sticker.png")).unwrap().image;
    let st = image::load_from_memory(&fs::read(lib.image_path(&ImageRef::input(sticker.clone()))).unwrap()).unwrap();
    assert!(st.color().has_alpha());

    // ---- ideas and input review
    let fox = lib.new_idea(NewIdea { images: vec![photo.clone()], ..NewIdea::text("Fox", "a fox in a café") }).unwrap();
    let hut = lib
        .new_idea(NewIdea {
            images: vec![rotated.clone(), p3.clone()],
            source: IdeaSource::DroppedFile { original_path: "/home/user/Pictures/hut.png".into() },
            ..NewIdea::text("Hut", "")
        })
        .unwrap();
    let junk = lib.new_idea(NewIdea { images: vec![sticker.clone()], ..NewIdea::text("Junk", "meh") }).unwrap();
    assert_eq!(fox.as_str(), "2026.10.24-12.00-0000");
    assert_eq!(hut.as_str(), "2026.10.24-12.00-0001");
    let hut_dir = lib.find_idea(&hut).unwrap().1;
    assert!(hut_dir.join("1-hut.png").exists() && hut_dir.join("2-fish.png").exists());

    lib.review_idea(&fox, Verdict::Good).unwrap();
    lib.review_idea(&hut, Verdict::Neutral).unwrap();
    lib.review_idea(&junk, Verdict::Bad).unwrap();
    assert!(lib.load_idea(&fox).unwrap().starred);
    assert!(!lib.load_idea(&hut).unwrap().starred);
    assert_eq!(lib.find_idea(&junk).unwrap().0, IdeaLoc::Discarded);

    // ---- input library folders
    let places = lib.create_folder(Tree::InputLibrary, Path::new(""), Some("places")).unwrap();
    assert!(matches!(
        lib.create_folder(Tree::InputLibrary, Path::new(""), Some("Places")),
        Err(Error::InvalidName { .. })
    ));
    let deep = lib.create_folder(Tree::InputLibrary, &places, None).unwrap();
    assert_eq!(deep, Path::new("places/new folder"));
    let moved = lib.move_node(Tree::InputLibrary, Path::new(hut.as_str()), &deep).unwrap();
    assert_eq!(lib.find_idea(&hut).unwrap().0, IdeaLoc::Library(deep.clone()));
    resolves(&lib, &Path::new("input/library").join(&moved).join("1-hut.png"), Store::Input);
    let renamed = lib.rename_node(Tree::InputLibrary, &deep, "alpine").unwrap();
    assert_eq!(renamed, Path::new("places/alpine"));
    assert!(matches!(
        lib.move_node(Tree::InputLibrary, Path::new("places"), Path::new("places/alpine")),
        Err(Error::Refused(_))
    ));
    let copy = lib.copy_node(Tree::InputLibrary, Path::new("places"), Path::new("")).unwrap();
    assert_eq!(copy, Path::new("places 2"));
    let ix = lib.scan().unwrap();
    assert_eq!(ix.ideas.iter().filter(|e| e.idea.title == "Hut").count(), 2);

    // ---- projects, runs, outputs
    lib.create_folder(Tree::Projects, Path::new(""), Some("clients")).unwrap();
    let (project, project_path) =
        lib.create_project(Path::new("clients"), Some("fox"), std::slice::from_ref(&fox)).unwrap();
    assert_eq!(project_path, Path::new("clients/fox"));
    assert_eq!(lib.load_idea(&fox).unwrap().projects, std::slice::from_ref(&project));
    assert!(matches!(lib.create_project(Path::new("clients/fox"), None, &[]), Err(Error::Refused(_))));
    let run = lib.create_run(&project, new_run(vec![ImageRef::input(photo.clone())], Mode::Generate, 4)).unwrap();
    let (_, run_def) = lib.find_run(&run).unwrap();
    // what's left to generate, by the scan as by the disk
    let by_scan = |lib: &Library| {
        let ix = lib.scan().unwrap();
        let p = ix.projects.iter().find(|p| p.project.id == project).unwrap();
        p.pending_seeds(p.runs.iter().find(|r| r.id == run).unwrap()).collect::<Vec<_>>()
    };
    assert_eq!(by_scan(&lib), lib.pending_seeds(&run).unwrap());
    assert_eq!(by_scan(&lib).len(), 4);
    let mut outs = Vec::new();
    for seed in 0..4 {
        let (id, link_path) =
            lib.write_output(&run, 0, seed, &sdserver_png(seed as u8), &recipe(&run_def, seed, None)).unwrap();
        assert_eq!(link_path.file_name().unwrap().to_string_lossy(), format!("{run}_r0_s{seed:04}.png"));
        let bytes = fs::read(lib.image_path(&ImageRef::output(id.clone()))).unwrap();
        assert!(png::text(&bytes, "parameters").unwrap().unwrap().starts_with("a fox"), "parameters kept");
        assert_eq!(lib.recipe(&id).unwrap().unwrap().seed, seed);
        outs.push((id, link_path));
        assert_eq!(by_scan(&lib), lib.pending_seeds(&run).unwrap());
    }
    assert!(by_scan(&lib).is_empty());
    // the output records its inputs and both carry a BLAKE3 pixel hash
    let photo_ref = ImageRef::input(photo.clone());
    let photo_bytes = fs::read(lib.image_path(&photo_ref)).unwrap();
    let photo_blake3 = png::text(&photo_bytes, png::KEY_BLAKE3).unwrap().unwrap();
    assert_eq!(photo_blake3, increart_core::import::decode_pixels(&photo_bytes).unwrap().blake3());
    let inputs = lib.recipe(&outs[0].0).unwrap().unwrap().inputs;
    assert_eq!(inputs.len(), 1);
    assert_eq!((inputs[0].role, &inputs[0].image), (InputRole::Ref(1), &photo_ref));
    assert_eq!(inputs[0].blake3, photo_blake3);
    assert_eq!(inputs[0].file, format!("input_images/{photo}.png"));
    assert!(inputs[0].original_name.is_some());
    let out_bytes = fs::read(lib.image_path(&ImageRef::output(outs[0].0.clone()))).unwrap();
    assert_eq!(
        png::text(&out_bytes, png::KEY_BLAKE3).unwrap().unwrap(),
        increart_core::import::decode_pixels(&out_bytes).unwrap().blake3()
    );
    let good = lib.review_output(&outs[0].1, Some(Verdict::Good)).unwrap();
    lib.review_output(&outs[1].1, Some(Verdict::Bad)).unwrap();
    let neutral = lib.review_output(&outs[2].1, Some(Verdict::Neutral)).unwrap();
    lib.review_output(&neutral, None).unwrap();
    assert_eq!(resolves(&lib, &good, Store::Output).id, outs[0].0);

    assert_eq!(lib.add_seeds(&run, 2, false).unwrap(), (0, Seeds { first: 4, count: 2 }));
    assert_eq!(lib.add_seeds(&run, 2, true).unwrap(), (1, Seeds { first: 6, count: 2 }));
    assert_eq!(lib.find_run(&run).unwrap().1.next_seed(), 8);

    let edit_mode = Mode::Edit { init: Some(ImageRef::output(outs[0].0.clone())), strength: 0.6 };
    let edit_run = lib.create_run(&project, new_run(vec![], edit_mode, 1)).unwrap();
    let (_, edit_def) = lib.find_run(&edit_run).unwrap();
    let (edited, _) = lib
        .write_output(
            &edit_run,
            0,
            0,
            &sdserver_png(9),
            &recipe(&edit_def, 0, Some(ImageRef::output(outs[0].0.clone()))),
        )
        .unwrap();
    let inputs = lib.recipe(&edited).unwrap().unwrap().inputs;
    assert_eq!(
        inputs.iter().map(|i| (i.role, i.image.clone())).collect::<Vec<_>>(),
        [(InputRole::Start, ImageRef::output(outs[0].0.clone()))]
    );
    assert_eq!(inputs[0].original_name, None);
    lib.set_cover(&project, Some(ImageRef::output(outs[0].0.clone()))).unwrap();

    // ---- collections, output → input
    lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
    let in_coll = lib.add_to_collection(Path::new("best"), &outs[0].0).unwrap();
    assert_eq!(lib.add_to_collection(Path::new("best"), &outs[0].0).unwrap(), in_coll);
    let from_out = lib.idea_from_output(&outs[1].0, true, IdeaLoc::Unreviewed).unwrap();
    let idea = lib.load_idea(&from_out).unwrap();
    assert_eq!(idea.text, "a fox in a café");
    assert_eq!(idea.source, IdeaSource::FromOutput { image: ImageRef::output(outs[1].0.clone()) });

    // ---- project tree: copy (setup only), move with a clashing name
    let copied = lib.copy_node(Tree::Projects, Path::new("clients/fox"), Path::new("")).unwrap();
    assert_eq!(copied, Path::new("fox"));
    let ix = lib.scan().unwrap();
    let copy = ix.projects.iter().find(|p| p.dir == Path::new("projects/fox")).unwrap();
    assert_ne!(copy.project.id, project);
    assert_eq!(copy.runs.len(), 2);
    assert!(copy.runs.iter().all(|r| r.revisions.is_empty()) && copy.outputs.is_empty());
    let moved = lib.move_node(Tree::Projects, Path::new("clients/fox"), Path::new("")).unwrap();
    assert_eq!(moved, Path::new("fox 2"));
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    let p = ix.project(&project).unwrap();
    assert_eq!(p.outputs.len(), 5);
    for o in &p.outputs {
        resolves(&lib, &o.path, Store::Output);
    }
    assert_eq!(lib.find_project(&project).unwrap(), root.join("projects/fox 2"));

    let report = common::verify(&root);
    assert!(report.trim_end().ends_with("problems: none"), "{report}");

    // ---- discard (restorable), delete for good + clean up
    let copy_id = copy.project.id.clone();
    lib.delete_node(Tree::Projects, Path::new("fox")).unwrap();
    assert!(root.join("discarded_projects/fox/project.rsn").exists());
    lib.delete_discarded_project(&copy_id).unwrap();
    assert!(tmp.path().join("trash/fox").join("project.rsn").exists());
    assert!(lib.unreferenced().unwrap().is_empty());
    // a discarded project keeps its images through Clean up, and comes back
    // where it was, its links working
    lib.delete_node(Tree::Projects, Path::new("fox 2")).unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.project(&project).is_none());
    let d = &ix.discarded_projects[0];
    assert_eq!((d.discarded.name.as_str(), d.discarded.from.as_path()), ("fox 2", Path::new("")));
    let left = lib.unreferenced().unwrap();
    assert!(left.is_empty(), "kept for a restore: {left:?}");
    assert_eq!(lib.restore_project(&project).unwrap(), Path::new("fox 2"));
    let ix = lib.scan().unwrap();
    assert!(ix.discarded_projects.is_empty() && !root.join("projects/fox 2/discarded.rsn").exists());
    for o in &ix.project(&project).unwrap().outputs {
        resolves(&lib, &o.path, Store::Output);
    }
    lib.delete_discarded_idea(&junk).unwrap();
    lib.delete_node(Tree::Projects, Path::new("fox 2")).unwrap();
    lib.delete_discarded_project(&project).unwrap();
    let mut gone = lib.unreferenced().unwrap();
    gone.sort();
    let mut expect = vec![
        ImageRef::input(sticker.clone()),
        ImageRef::output(outs[2].0.clone()),
        ImageRef::output(outs[3].0.clone()),
        ImageRef::output(edited.clone()),
    ];
    expect.sort();
    // outs[0]: in a collection; outs[1]: an idea's FromOutput source; the
    // photo: an idea image and a ref in outs[0]'s recipe.
    assert_eq!(gone, expect);
    let mut cleaned = lib.clean_up().unwrap();
    cleaned.sort();
    assert_eq!(cleaned, gone);
    assert!(!lib.image_path(&ImageRef::output(edited)).exists());
    assert!(tmp.path().join("trash").join(format!("{sticker}.png")).exists());
    let report = common::verify(&root);
    assert!(report.trim_end().ends_with("problems: none"), "{report}");

    // ---- reopen: counters continue, lock released
    drop(lib);
    let mut lib = Library::open(&root).unwrap();
    assert!(lib.problems().is_empty(), "{:?}", lib.problems());
    lib.set_clock(t0);
    let next = lib.new_idea(NewIdea::text("after reopen", "")).unwrap();
    let ix = lib.scan().unwrap();
    assert_eq!(ix.ideas.iter().filter(|e| e.idea.id == next).count(), 1);
    assert!(ix.ideas.iter().all(|e| e.idea.id <= next));

    // ---- the whole-library check agrees, and the library moves between
    // OSes: no path in its files or images uses `\` or names this machine
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(report.store_images > 0 && report.links > 0, "{}", report.summary());
    let home = root.to_string_lossy().into_owned();
    for f in increart_core::fsutil::walk(&root).unwrap() {
        // the lock: Windows locks are mandatory, so it can't be read while held
        let skip = [root.join("cache"), root.join("thumbs"), root.join(increart_core::library::LOCK_FILE)];
        if skip.iter().any(|s| f.starts_with(s)) || !f.is_file() {
            continue;
        }
        let text = String::from_utf8_lossy(&fs::read(&f).unwrap()).into_owned();
        for dir in ["_images\\", "projects\\", "input\\", "collections\\"] {
            assert!(!text.contains(dir), "{} names a path with `\\`", f.display());
        }
        assert!(!text.contains(&home), "{} names the library's own path", f.display());
    }
}

/// Converts every store image to `to` the way the GUI does: encode, then
/// install.
fn convert(lib: &mut Library, to: StoreFormat) {
    let c = lib.conversion(to).unwrap();
    assert!(!c.images.is_empty());
    let done: Vec<_> = c.images.iter().filter_map(|r| c.encode(r).unwrap()).collect();
    assert_eq!(lib.install(done).unwrap(), []);
    assert!(lib.conversion(to).unwrap().images.is_empty());
}

/// A link's image, checking it has the store file's pixels (a copy's usage
/// counts may lag, so not its bytes).
fn same_pixels(lib: &Library, rel: &Path, store: Store) -> ImageRef {
    let abs = lib.path(rel);
    let r = link::read_ref(&abs, store).unwrap().unwrap_or_else(|| panic!("{} is not a link", rel.display()));
    let px = |p: &Path| import::decode_pixels(&fs::read(p).unwrap()).unwrap().hash();
    assert_eq!(px(&abs), px(&lib.image_path(&r)), "{}", rel.display());
    r
}

#[test]
fn webp_store_and_conversion_both_ways() {
    for mode in [LinkMode::Symlink, LinkMode::Hardlink, LinkMode::Copy] {
        let (_tmp, mut lib) = setup(Some(mode));
        let root = lib.root().to_path_buf();
        let format_number = || increart_core::rsnfmt::read::<LibraryMeta>(&root.join("library.rsn")).unwrap().format;
        // a PNG library first, then WebP for new images: a mixed one
        let photo =
            ImageRef::input(lib.import_bytes(&jpeg_bytes(picture(20, 20, 3)), &original("a.jpg")).unwrap().image);
        lib.set_store_format(StoreFormat::Webp).unwrap();
        assert_eq!(format_number(), LIBRARY_FORMAT_WEBP);
        let rgba = RgbaImage::from_fn(8, 8, |x, y| Rgba([200, (y * 9) as u8, 10, (x * 30) as u8]));
        let sticker = lib.import_bytes(&png_bytes(DynamicImage::ImageRgba8(rgba)), &original("s.png")).unwrap().image;
        let sticker = ImageRef::input(sticker);
        assert_eq!(lib.format_of(&photo), StoreFormat::Png);
        assert_eq!(lib.format_of(&sticker), StoreFormat::Webp);
        let idea = lib
            .new_idea(NewIdea { images: vec![photo.id.clone(), sticker.id.clone()], ..NewIdea::text("A", "") })
            .unwrap();
        let idea_dir = lib.find_idea(&idea).unwrap().1;
        assert!(idea_dir.join("1-a.png").exists() && idea_dir.join("2-s.webp").exists());
        let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
        let run = lib.create_run(&project, new_run(vec![sticker.clone()], Mode::Generate, 2)).unwrap();
        assert_eq!(lib.usage_counts(&sticker).unwrap(), counts(1, 0), "patched in a WebP file");
        let (_, def) = lib.find_run(&run).unwrap();
        let (out, at) = lib.write_output(&run, 0, 0, &sdserver_png(1), &recipe(&def, 0, None)).unwrap();
        let out = ImageRef::output(out);
        assert_eq!(at.extension().unwrap(), "webp");
        // sd-server's chunk travels with ours; servers get PNG
        let bytes = fs::read(lib.image_path(&out)).unwrap();
        assert_eq!(container::format_of(&bytes), Some(StoreFormat::Webp));
        assert!(container::text(&bytes, "parameters").unwrap().is_some());
        assert_eq!(lib.recipe(&out.id).unwrap().unwrap().seed, 0);
        assert!(lib.read_verified(&sticker).unwrap().starts_with(png::SIGNATURE));
        let reviewed = lib.review_output(&at, Some(Verdict::Good)).unwrap();
        lib.create_folder(Tree::Collections, Path::new(""), Some("best")).unwrap();
        let in_coll = lib.add_to_collection(Path::new("best"), &out.id).unwrap();
        assert_eq!(in_coll.extension().unwrap(), "webp");

        let hashes: Vec<String> = [&photo, &sticker, &out]
            .iter()
            .map(|r| import::decode_pixels(&fs::read(lib.image_path(r)).unwrap()).unwrap().hash())
            .collect();
        let check = |lib: &Library, ext: &str| {
            let ix = lib.scan().unwrap();
            assert!(ix.problems.is_empty(), "{mode:?} {:?}", ix.problems);
            assert!(lib.unreferenced().unwrap().is_empty());
            for (r, h) in [&photo, &sticker, &out].iter().zip(&hashes) {
                let bytes = fs::read(lib.image_path(r)).unwrap();
                assert_eq!(&import::decode_pixels(&bytes).unwrap().hash(), h);
                assert_eq!(container::text(&bytes, png::KEY_PIXELS).unwrap().as_ref(), Some(h));
                assert_eq!(container::text(&bytes, png::KEY_ID).unwrap().as_deref(), Some(r.id.as_str()));
            }
            assert_eq!(lib.usage_counts(&sticker).unwrap(), counts(1, 0));
            assert_eq!(lib.source_info(&photo.id).unwrap().unwrap().original_name.as_deref(), Some("a.jpg"));
            let p = ix.project(&project).unwrap();
            assert_eq!(p.outputs.len(), 1);
            assert_eq!(p.outputs[0].path.extension().unwrap(), ext);
            assert_eq!(same_pixels(lib, &p.outputs[0].path, Store::Output), out);
            assert_eq!(ix.collection_links.len(), 1);
            assert_eq!(ix.collection_links[0].path.extension().unwrap(), ext);
            same_pixels(lib, &ix.collection_links[0].path, Store::Output);
            let dir = &ix.idea(&idea).unwrap().dir;
            assert_eq!(same_pixels(lib, &dir.join(format!("1-a.{ext}")), Store::Input), photo);
            assert_eq!(same_pixels(lib, &dir.join(format!("2-s.{ext}")), Store::Input), sticker);
            for r in [&photo, &sticker, &out] {
                assert!(lib.image_path(r).extension().unwrap() == ext);
            }
        };
        assert!(reviewed.ends_with(format!("{}", at.file_name().unwrap().to_string_lossy())));

        convert(&mut lib, StoreFormat::Webp);
        check(&lib, "webp");
        assert_eq!(lib.store_counts().unwrap().png.0, 0);
        // the verifier follows symlinks only
        if mode == LinkMode::Symlink {
            let report = common::verify(&root);
            assert!(report.trim_end().ends_with("3 WebP pixel hashes left unchecked; problems: none"), "{report}");
        }

        lib.set_store_format(StoreFormat::Png).unwrap();
        assert_eq!(format_number(), LIBRARY_FORMAT_WEBP, "WebP files remain");
        convert(&mut lib, StoreFormat::Png);
        check(&lib, "png");
        assert_eq!(format_number(), LIBRARY_FORMAT);
        if mode == LinkMode::Symlink {
            let report = common::verify(&root);
            assert!(report.trim_end().ends_with(" refs; problems: none"), "{report}");
        }

        // reopens with the setting
        lib.set_store_format(StoreFormat::Webp).unwrap();
        drop(lib);
        let lib = Library::open(&root).unwrap();
        assert_eq!(lib.store_format(), StoreFormat::Webp);
        assert!(lib.problems().is_empty(), "{:?}", lib.problems());
    }
}

#[cfg(feature = "avif")]
#[test]
fn avif_imports_as_png() {
    let (_tmp, mut lib) = setup(None);
    let avif =
        avif_bytes(RgbaImage::from_fn(12, 8, |x, y| Rgba([(x * 20) as u8, (y * 30) as u8, 90, 255 - (x * 20) as u8])));
    let still = lib.import_bytes(&avif, &original("still.avif")).unwrap().image;
    let src = lib.source_info(&still).unwrap().unwrap();
    assert_eq!((src.original_format.as_str(), &src.conversions[..]), ("avif", &["avif -> png".to_string()][..]));
    let av = image::load_from_memory(&fs::read(lib.image_path(&ImageRef::input(still.clone()))).unwrap()).unwrap();
    assert_eq!((av.width(), av.height(), av.color().has_alpha()), (12, 8, true));
}

/// A library deep enough that its files' paths pass Windows' old 260
/// character limit, in the link mode this file system gets.
#[test]
fn long_paths() {
    common::init();
    let tmp = tempfile::tempdir().unwrap();
    let mut root = tmp.path().to_path_buf();
    while root.as_os_str().len() < 240 {
        root.push("a folder name long enough to need a few of them");
    }
    let mut lib = Library::create(&root, None).unwrap();
    lib.set_clock(t0);
    lib.set_disposal(Disposal::Directory(tmp.path().join("trash")));
    let photo = lib.import_bytes(&jpeg_bytes(picture(20, 20, 3)), &original("a photo with a long name.jpg")).unwrap();
    let idea = lib.new_idea(NewIdea { images: vec![photo.image], ..NewIdea::text("A", "") }).unwrap();
    lib.review_idea(&idea, Verdict::Good).unwrap();
    let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 1)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let (_, at) = lib.write_output(&run, 0, 0, &sdserver_png(1), &recipe(&def, 0, None)).unwrap();
    assert!(lib.path(&at).as_os_str().len() > 300, "{}", lib.path(&at).display());
    lib.review_output(&at, Some(Verdict::Good)).unwrap();
    lib.create_folder(Tree::Projects, Path::new(""), Some("a folder for the project")).unwrap();
    lib.move_node(Tree::Projects, Path::new("new project"), Path::new("a folder for the project")).unwrap();
    drop(lib);

    let lib = Library::open(&root).unwrap();
    assert!(lib.problems().is_empty(), "{:?}", lib.problems());
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!((report.projects, report.outputs), (1, 1), "{}", report.summary());
}

#[test]
fn copy_mode_library() {
    let (_tmp, mut lib) = setup(Some(LinkMode::Copy));
    let photo = lib.import_bytes(&jpeg_bytes(picture(20, 20, 3)), &original("a.jpg")).unwrap().image;
    let idea = lib.new_idea(NewIdea { images: vec![photo.clone()], ..NewIdea::text("A", "") }).unwrap();
    let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 1)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let (out, at) = lib.write_output(&run, 0, 0, &sdserver_png(1), &recipe(&def, 0, None)).unwrap();
    assert!(!fsutil::is_symlink(&lib.path(&at)));
    let reviewed = lib.review_output(&at, Some(Verdict::Good)).unwrap();
    lib.create_folder(Tree::Projects, Path::new(""), Some("deep")).unwrap();
    lib.create_folder(Tree::Projects, Path::new("deep"), Some("er")).unwrap();
    let moved = lib.move_node(Tree::Projects, Path::new("new project"), Path::new("deep/er")).unwrap();
    assert_eq!(moved, Path::new("deep/er/new project"));

    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    let p = ix.project(&project).unwrap();
    assert_eq!(p.outputs[0].image, ImageRef::output(out));
    assert_eq!(p.outputs[0].verdict, Some(Verdict::Good));
    assert!(reviewed.ends_with(p.outputs[0].path.file_name().unwrap()));
    let idea_dir = &ix.idea(&idea).unwrap().dir;
    resolves(&lib, &idea_dir.join("1-a.png"), Store::Input);
    assert!(lib.unreferenced().unwrap().is_empty());
}

/// A seed generated once (it has a timing) isn't generated again when its
/// link is gone: deleted, or lost unzipping (a tool that skips symlinks).
#[test]
fn a_generated_seed_without_its_link_isnt_pending() {
    let (_tmp, mut lib) = setup(None);
    let idea = lib.new_idea(NewIdea::text("A", "")).unwrap();
    let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 3)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let (_, deleted) = lib.write_output(&run, 0, 0, &sdserver_png(1), &recipe(&def, 0, None)).unwrap();
    let (_, lost) = lib.write_output(&run, 0, 1, &sdserver_png(2), &recipe(&def, 1, None)).unwrap();
    lib.delete_output_link(&deleted).unwrap();
    fs::remove_file(lib.path(&lost)).unwrap();
    assert_eq!(lib.pending_seeds(&run).unwrap(), vec![(0, 2)]);
    let ix = lib.scan().unwrap();
    let p = ix.project(&project).unwrap();
    assert_eq!(p.pending_seeds(&p.runs[0]).collect::<Vec<_>>(), vec![(0, 2)]);

    // the deleted one is recorded; the lost one is a problem
    assert_eq!(p.missing_outputs(&p.runs[0]).collect::<Vec<_>>(), vec![(0, 1)]);
    let report = increart_core::check::check(&lib).unwrap();
    assert_eq!(report.pending, 1);
    assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
    assert!(
        report.problems[0].contains("new project: 1 generated image(s) have no output link"),
        "{:?}",
        report.problems
    );
    assert!(report.summary().contains(", 1 seeds to generate: 1 problem(s)"), "{}", report.summary());

    // a library from before deletions were recorded: what's gone is taken
    // as deleted
    let root = lib.root().to_path_buf();
    drop(lib);
    let meta_path = root.join("library.rsn");
    let mut meta: LibraryMeta = increart_core::rsnfmt::read(&meta_path).unwrap();
    meta.metadata = 1;
    increart_core::rsnfmt::write(&meta_path, &meta).unwrap();
    let lib = Library::open(&root).unwrap();
    let report = increart_core::check::check(&lib).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(lib.find_run(&run).unwrap().1.deleted, vec![Seeds { first: 0, count: 2 }]);
    assert_eq!(lib.pending_seeds(&run).unwrap(), vec![(0, 2)]);
}

/// A library whose `library.rsn` names no link mode (the example library,
/// from git or a zip) probes one on open and saves it: copies still
/// resolve, symlink stand-ins (git's, where it can't make symlinks) become
/// links, and new links are made the probed way.
#[test]
fn a_library_without_a_link_mode_probes_one() {
    let (tmp, mut lib) = setup(Some(LinkMode::Copy));
    let photo = lib.import_bytes(&jpeg_bytes(picture(20, 20, 3)), &original("a.jpg")).unwrap().image;
    let idea = lib.new_idea(NewIdea { images: vec![photo.clone()], ..NewIdea::text("A", "") }).unwrap();
    let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
    let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 3)).unwrap();
    let (_, def) = lib.find_run(&run).unwrap();
    let (_, copied) = lib.write_output(&run, 0, 0, &sdserver_png(1), &recipe(&def, 0, None)).unwrap();
    let (out, stood_in) = lib.write_output(&run, 0, 2, &sdserver_png(3), &recipe(&def, 2, None)).unwrap();
    let idea_link = lib.path(lib.scan().unwrap().idea(&idea).unwrap().dir.join("1-a.png"));
    let depth = |p: &Path| p.strip_prefix(lib.root()).unwrap().components().count() - 1;
    let up = |p: &Path| "../".repeat(depth(p));
    fs::write(lib.path(&stood_in), format!("{}output_images/{out}.png", up(&lib.path(&stood_in)))).unwrap();
    fs::write(&idea_link, format!("{}input_images/{photo}.png", up(&idea_link))).unwrap();
    // not stand-ins: a path to no image, and one that doesn't reach the store
    let bogus = lib.path(&stood_in).with_file_name("x.png");
    fs::write(&bogus, "../../../output_images/2026.01.01-00.00-0000.png").unwrap();
    let short = lib.path(&stood_in).with_file_name("y.png");
    fs::write(&short, format!("../output_images/{out}.png")).unwrap();
    drop(lib);
    let meta_path = tmp.path().join("lib/library.rsn");
    let body = fs::read_to_string(&meta_path).unwrap();
    fs::write(&meta_path, body.replace("link_mode: Copy,", "")).unwrap();
    assert!(!fs::read_to_string(&meta_path).unwrap().contains("link_mode"));

    let mut lib = Library::open(&tmp.path().join("lib")).unwrap();
    let probed = link::probe(lib.root()).unwrap();
    assert_eq!(lib.link_mode(), probed);
    let saved: LibraryMeta = increart_core::rsnfmt::read(&meta_path).unwrap();
    assert_eq!(saved.link_mode, Some(probed));
    let (_, def) = lib.find_run(&run).unwrap();
    let (_, linked) = lib.write_output(&run, 0, 1, &sdserver_png(2), &recipe(&def, 1, None)).unwrap();
    assert_eq!(fsutil::is_symlink(&lib.path(&linked)), probed == LinkMode::Symlink);
    resolves(&lib, &copied, Store::Output);
    resolves(&lib, &linked, Store::Output);
    assert_eq!(resolves(&lib, &stood_in, Store::Output), ImageRef::output(out));
    assert_eq!(fsutil::is_symlink(&lib.path(&stood_in)), probed == LinkMode::Symlink);
    assert_eq!(resolves(&lib, &idea_link, Store::Input), ImageRef::input(photo));
    assert!(fs::read(&bogus).unwrap().starts_with(b"../") && fs::read(&short).unwrap().starts_with(b"../"));
    fs::remove_file(&bogus).unwrap();
    fs::remove_file(&short).unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.problems.is_empty(), "{:?}", ix.problems);
    assert_eq!(ix.project(&project).unwrap().outputs.len(), 3);
}

#[test]
fn inbox_drop_becomes_ideas() {
    let (_tmp, mut lib) = setup(None);
    let drop = lib.path("input/inbox_drop");
    fs::write(drop.join("otter.txt"), "otter with a teacup\n").unwrap();
    fs::write(drop.join("IMG_1.png"), png_bytes(DynamicImage::ImageRgb8(picture(8, 8, 5)))).unwrap();
    fs::write(drop.join("notes.pdf"), b"%PDF-1.4").unwrap();
    let ideas = lib.import_inbox_drop().unwrap();
    assert_eq!(ideas.len(), 2);
    let ix = lib.scan().unwrap();
    let texts: Vec<_> = ix.ideas.iter().map(|e| (e.idea.text.as_str(), e.idea.images.len(), &e.loc)).collect();
    assert!(texts.contains(&("otter with a teacup", 0, &IdeaLoc::Unreviewed)));
    assert!(texts.contains(&("", 1, &IdeaLoc::Unreviewed)));
    assert!(ix.ideas.iter().all(|e| e.idea.source == IdeaSource::InboxFolder));
    assert_eq!(ix.inbox_drop, [PathBuf::from("input/inbox_drop/notes.pdf")]);
    assert!(drop.join("imported/otter.txt").exists() && drop.join("imported/IMG_1.png").exists());
    let src =
        lib.source_info(&ix.ideas.iter().find(|e| !e.idea.images.is_empty()).unwrap().idea.images[0]).unwrap().unwrap();
    assert!(Path::new(&src.original_path.unwrap()).ends_with("input/inbox_drop/IMG_1.png"));
}

#[test]
fn names_and_refusals() {
    let (_tmp, mut lib) = setup(None);
    for bad in ["", "  ", "a/b", "a\\b", ".hidden"] {
        assert!(
            matches!(lib.create_folder(Tree::Projects, Path::new(""), Some(bad)), Err(Error::InvalidName { .. })),
            "{bad:?}"
        );
    }
    lib.create_folder(Tree::Projects, Path::new(""), Some("Alpha")).unwrap();
    lib.create_folder(Tree::Projects, Path::new(""), Some("beta")).unwrap();
    assert!(matches!(lib.rename_node(Tree::Projects, Path::new("beta"), "ALPHA"), Err(Error::InvalidName { .. })));
    assert_eq!(lib.rename_node(Tree::Projects, Path::new("Alpha"), "alpha").unwrap(), Path::new("alpha"));
    assert_eq!(lib.create_folder(Tree::Projects, Path::new(""), None).unwrap(), Path::new("new folder"));
    assert_eq!(lib.create_folder(Tree::Projects, Path::new(""), None).unwrap(), Path::new("new folder 2"));
    let (_, p) = lib.create_project(Path::new("alpha"), None, &[]).unwrap();
    assert!(matches!(lib.move_node(Tree::Projects, Path::new("beta"), &p), Err(Error::Refused(_))));
    assert!(matches!(lib.move_node(Tree::Projects, Path::new(".."), Path::new("")), Err(Error::Refused(_))));
    assert_eq!(lib.move_node(Tree::Projects, &p, Path::new("alpha")).unwrap(), p, "same place: no-op");
    let idea = lib.new_idea(NewIdea::text("x", "")).unwrap();
    lib.review_idea(&idea, Verdict::Neutral).unwrap();
    assert!(matches!(lib.rename_node(Tree::InputLibrary, Path::new(idea.as_str()), "y"), Err(Error::Refused(_))));
    assert!(matches!(lib.delete_discarded_idea(&idea), Err(Error::Refused(_))));
    let discarded = lib.delete_node(Tree::InputLibrary, Path::new(idea.as_str())).unwrap();
    assert_eq!(discarded, std::slice::from_ref(&idea));
    assert_eq!(lib.find_idea(&idea).unwrap().0, IdeaLoc::Discarded);
    assert!(matches!(Library::create(lib.root(), None), Err(Error::Refused(_))), "not empty");
}

#[test]
fn drafts_are_saved_copied_and_protect_their_images() {
    let (_tmp, mut lib) = setup(None);
    let a = lib.import_bytes(&jpeg_bytes(picture(16, 16, 1)), &original("a.jpg")).unwrap().image;
    let b = lib.import_bytes(&jpeg_bytes(picture(16, 16, 2)), &original("b.jpg")).unwrap().image;
    let (project, path) = lib.create_project(Path::new(""), Some("p"), &[]).unwrap();
    let draft = GenerateDraft {
        sweep: Default::default(),
        label: "r01".into(),
        prompt: "the fox from <image1>".into(),
        negative_prompt: String::new(),
        width: 1344,
        height: 896,
        preset: "Qwen-Image 2.1".into(),
        steps: 20,
        cfg: 2.0,
        flow_shift: None,
        seeds: 4,
        first_seed: 0,
        alpha: true,
        refs: vec![ImageRef::input(a.clone())],
        init: None,
        images: vec![],
        strength: 0.75,
        source_idea: None,
    };
    lib.save_generate_draft(&project, &draft).unwrap();
    let idea_draft =
        IdeaDraftFile { title: "t".into(), text: "x".into(), tags: String::new(), images: vec![b.clone()] };
    lib.save_idea_draft(&idea_draft).unwrap();

    let ix = lib.scan().unwrap();
    assert_eq!(ix.project(&project).unwrap().draft.as_ref(), Some(&draft));
    assert_eq!(ix.idea_draft.as_ref(), Some(&idea_draft));
    assert!(lib.unreferenced().unwrap().is_empty(), "draft refs and draft images are kept");

    let copy = lib.copy_node(Tree::Projects, &path, Path::new("")).unwrap();
    let ix = lib.scan().unwrap();
    let copied = ix.projects.iter().find(|p| p.dir == Path::new("projects").join(&copy)).unwrap();
    assert_eq!(copied.draft.as_ref(), Some(&draft), "a copied project keeps its draft");

    lib.clear_generate_draft(&project).unwrap();
    lib.clear_generate_draft(&project).unwrap();
    lib.clear_idea_draft().unwrap();
    let ix = lib.scan().unwrap();
    assert!(ix.project(&project).unwrap().draft.is_none() && ix.idea_draft.is_none());
    assert_eq!(lib.unreferenced().unwrap(), [ImageRef::input(b)], "only the copy's draft still holds image a");
}

#[test]
fn smart_is_reserved_at_the_top_of_collections() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = increart_core::Library::create(&tmp.path().join("lib"), None).unwrap();
    let c = increart_core::Tree::Collections;
    let refused = |r: increart_core::error::Result<PathBuf>| {
        let e = r.unwrap_err().to_string();
        assert!(e.contains("reserved"), "{e}");
    };
    refused(lib.create_folder(c, Path::new(""), Some("Smart")));
    // fine below the top, and in other trees
    lib.create_folder(c, Path::new(""), Some("work")).unwrap();
    lib.create_folder(c, Path::new("work"), Some("smart")).unwrap();
    lib.create_folder(increart_core::Tree::Projects, Path::new(""), Some("smart")).unwrap();
    refused(lib.move_node(c, Path::new("work/smart"), Path::new("")));
    refused(lib.copy_node(c, Path::new("work/smart"), Path::new("")));
    refused(lib.rename_node(c, Path::new("work"), "SMART"));
    assert!(!tmp.path().join("lib/collections/smart").exists() && !tmp.path().join("lib/collections/SMART").exists());
}

fn usage(lib: &Library, r: &ImageRef) -> png::UsageCounts {
    lib.usage_counts(r).unwrap()
}

fn counts(refs: u64, starts: u64) -> png::UsageCounts {
    png::UsageCounts { refs, starts }
}

#[test]
fn usage_counts_live_in_the_images() {
    let (tmp, mut lib) = setup(None);
    let mut import = |seed| {
        let bytes = png_bytes(DynamicImage::ImageRgb8(picture(8, 8, seed)));
        ImageRef { id: lib.import_bytes(&bytes, &original("a.png")).unwrap().image, store: Store::Input }
    };
    let (a, b) = (import(1), import(2));
    assert_eq!(usage(&lib, &a), counts(0, 0), "imports start at zero");
    let len = fs::metadata(lib.image_path(&a)).unwrap().len();

    let (project, _) = lib.create_project(Path::new(""), Some("p"), &[]).unwrap();
    let edit = Mode::Edit { init: Some(b.clone()), strength: 0.5 };
    // a run naming a twice counts once
    lib.create_run(&project, new_run(vec![a.clone(), a.clone(), b.clone()], edit.clone(), 2)).unwrap();
    assert_eq!(usage(&lib, &a), counts(1, 0));
    assert_eq!(usage(&lib, &b), counts(1, 1));
    assert_eq!(fs::metadata(lib.image_path(&a)).unwrap().len(), len, "patched in place");

    // a run without seeds counts when it first generates
    let later = lib.create_run(&project, new_run(vec![a.clone()], Mode::Generate, 0)).unwrap();
    assert_eq!(usage(&lib, &a), counts(1, 0));
    lib.add_seeds(&later, 2, false).unwrap();
    lib.add_seeds(&later, 2, true).unwrap();
    assert_eq!(usage(&lib, &a), counts(2, 0));

    // ever used: deleting a run doesn't lower them, and a higher count is kept
    let root = lib.root().to_path_buf();
    let path_a = lib.image_path(&a);
    let path_b = lib.image_path(&b);
    let (at, chunk, ok) = png::find_header_chunk(&path_a, png::KEY_USAGE).unwrap().unwrap();
    assert!(ok);
    assert_eq!(chunk.data, b"increart:usage\0ref=000000002 start=000000000");
    png::patch_chunk(&path_a, at, &counts(7, 0).chunk()).unwrap();
    // a torn write: new data, old CRC
    let (at_b, _, _) = png::find_header_chunk(&path_b, png::KEY_USAGE).unwrap().unwrap();
    let mut bytes = fs::read(&path_b).unwrap();
    let data = at_b as usize + 8 + "increart:usage\0ref=".len();
    bytes[data..data + 9].copy_from_slice(b"000000000");
    fs::write(&path_b, &bytes).unwrap();
    assert!(!png::find_header_chunk(&path_b, png::KEY_USAGE).unwrap().unwrap().2);
    drop(lib);

    let mut lib = Library::open(&root).unwrap();
    let torn = !png::find_header_chunk(&path_b, png::KEY_USAGE).unwrap().unwrap().2;
    assert!(torn, "open doesn't check (the GUI does, in the background)");
    // the GUI's lock-free read: only the torn one needs the lock
    let check = lib.usage_check().unwrap();
    assert!(check.covers(&a), "a higher count covers the runs");
    assert!(!check.covers(&b), "torn");
    lib.check_all_usage().unwrap();
    assert!(lib.usage_check().unwrap().covers(&b));
    assert_eq!(usage(&lib, &a), counts(7, 0));
    assert_eq!(usage(&lib, &b), counts(1, 1), "repaired from the runs");
    assert!(png::find_header_chunk(&path_b, png::KEY_USAGE).unwrap().unwrap().2);
    drop(tmp);
}

#[test]
fn imports_record_the_original_files_times_and_mode() {
    let (tmp, mut lib) = setup(None);
    let file = tmp.path().join("fox.png");
    fs::write(&file, png_bytes(DynamicImage::ImageRgb8(picture(8, 8, 3)))).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let id = lib.import_file(&file).unwrap().image;
    let bytes = fs::read(lib.image_path(&ImageRef { id, store: Store::Input })).unwrap();
    let source: SourceInfo = serde_json::from_str(&png::text(&bytes, png::KEY_SOURCE).unwrap().unwrap()).unwrap();
    assert!(source.original_mtime.is_some() && source.original_atime.is_some());
    // the inode change time, mode and owner are Unix's only
    if cfg!(unix) {
        assert_eq!(source.original_mode, Some(0o640));
        assert!(source.original_ctime.is_some() && source.original_uid.is_some() && source.original_gid.is_some());
    }
}

#[test]
fn transparency_is_read_from_the_pixels() {
    let (tmp, mut lib) = setup(None);
    // Qwen's "opaque" RGBA (alpha 253) vs a cut-out with a clear background
    let rgba = |clear: bool| {
        png_bytes(DynamicImage::ImageRgba8(RgbaImage::from_fn(8, 8, |x, _| {
            Rgba([x as u8, 0, 0, if clear && x < 4 { 0 } else { 253 }])
        })))
    };
    let (project, _) = lib.create_project(Path::new(""), Some("p"), &[]).unwrap();
    let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 2)).unwrap();
    let def = lib.find_run(&run).unwrap().1;
    let (opaque, _) = lib.write_output(&run, 0, 0, &rgba(false), &recipe(&def, 0, None)).unwrap();
    let (clear, _) = lib.write_output(&run, 0, 1, &rgba(true), &recipe(&def, 1, None)).unwrap();
    let input = lib.import_bytes(&rgba(true), &original("cut.png")).unwrap().image;
    let expect = [ImageRef::input(input), ImageRef::output(clear)];
    let ix = lib.scan().unwrap();
    assert_eq!(ix.transparent.iter().cloned().collect::<Vec<_>>(), expect);
    assert!(!ix.transparent(&ImageRef::output(opaque)));

    // an index from before the transparent set is rebuilt on open
    let root = lib.root().to_path_buf();
    drop(lib);
    let index = root.join(increart_core::library::PIXEL_INDEX);
    let mut old: PixelIndexFile = increart_core::rsnfmt::read(&index).unwrap();
    (old.version, old.transparent) = (1, Default::default());
    increart_core::rsnfmt::write(&index, &old).unwrap();
    let lib = Library::open(&root).unwrap();
    assert_eq!(lib.scan().unwrap().transparent.into_iter().collect::<Vec<_>>(), expect);
    drop(tmp);
}

/// A library exported to each archive format and unpacked again opens with
/// every link and image, and its links the kind this disk takes; a hard-link
/// or copy library goes in with each image once.
#[test]
fn libraries_go_to_archives_and_back() {
    use increart_core::archive::{self, ArchiveFormat};
    for mode in [LinkMode::Symlink, LinkMode::Hardlink, LinkMode::Copy] {
        let (tmp, mut lib) = setup(Some(mode));
        let photo = lib.import_bytes(&jpeg_bytes(picture(20, 20, 3)), &original("a.jpg")).unwrap().image;
        let idea = lib.new_idea(NewIdea { images: vec![photo], ..NewIdea::text("A", "") }).unwrap();
        let (project, _) = lib.create_project(Path::new(""), None, std::slice::from_ref(&idea)).unwrap();
        let run = lib.create_run(&project, new_run(vec![], Mode::Generate, 2)).unwrap();
        let (_, def) = lib.find_run(&run).unwrap();
        lib.write_output(&run, 0, 0, &sdserver_png(0), &recipe(&def, 0, None)).unwrap();
        let (_, rel) = lib.write_output(&run, 0, 1, &sdserver_png(1), &recipe(&def, 1, None)).unwrap();
        lib.review_output(&rel, Some(Verdict::Good)).unwrap();
        let store_bytes: u64 = ["input_images", "output_images"]
            .iter()
            .flat_map(|d| fs::read_dir(lib.path(d)).unwrap().flatten())
            .map(|e| e.metadata().unwrap().len())
            .sum();
        let before = increart_core::check::check(&lib).unwrap();
        assert!(before.problems.is_empty(), "{:?}", before.problems);

        for format in [ArchiveFormat::Zip, ArchiveFormat::TarGz] {
            let dest = tmp.path().join(format!("lib.{}", format.ext()));
            let size = archive::export(lib.root(), &dest, format, &mut |_, _| true).unwrap();
            // each image once (and a little for the records)
            assert!(size < store_bytes + 64 * 1024, "{mode:?} {format:?}: {size} for {store_bytes}");
            let into = tmp.path().join(format!("out-{}", format.ext()));
            fs::create_dir_all(&into).unwrap();
            let root = archive::unpack(&dest, &into, &mut |_, _| true).unwrap();
            assert_eq!(root, into.join("lib"));
            assert!(!fs::read_to_string(root.join("library.rsn")).unwrap().contains("link_mode"));
            assert!(!root.join("cache").exists() && !root.join("library.lock").exists());
            let back = Library::open(&root).unwrap();
            assert_eq!(back.link_mode(), link::probe(&root).unwrap());
            let after = increart_core::check::check(&back).unwrap();
            assert!(after.problems.is_empty(), "{mode:?} {format:?}: {:?}", after.problems);
            assert_eq!(after.summary(), before.summary(), "{mode:?} {format:?}");
            let ix = back.scan().unwrap();
            for o in &ix.project(&project).unwrap().outputs {
                resolves(&back, &o.path, Store::Output);
            }
            // there already: refused, and nothing left behind
            let again = archive::unpack(&dest, &into, &mut |_, _| true).unwrap_err();
            assert!(again.to_string().contains("is there already"), "{again}");
            assert_eq!(fs::read_dir(&into).unwrap().count(), 1);
        }
        // stopped part way: no archive
        let dest = tmp.path().join("stopped.zip");
        let e = archive::export(lib.root(), &dest, ArchiveFormat::Zip, &mut |done, _| done == 0).unwrap_err();
        assert!(archive::is_cancelled(&e), "{e}");
        assert!(!dest.exists() && !tmp.path().join(".stopped.zip.part").exists());
    }
}

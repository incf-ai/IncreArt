# IncreArt gallery

The main screens, on the [example library](../../README.md#example-library)
that comes with each release. No servers were running: the jobs are
simulated. Retaken by `scripts/make-readme-gallery.sh`; don't edit by hand.

**Set up servers.** Choose KoboldCpp and IncreArt finds your GPUs, picks a preset for each card's memory and shares the work between them: here two 24 GB cards each make images and run prompt enhance.

![Set up servers](gallery/server-wizard.webp)

**Home.** Where the app opens: how ideas become finished images, step by step, each with its tab, its keys and what's waiting there. Start a new project (it asks for reference images too), write an idea, import images or open the drop folder.

![Home](gallery/home.webp)

**Idea inbox.** Ideas come in as text, images or both: typed in, imported, or saved into the drop folder from a browser. Save each one to the input library, discard it or promote it.

![Idea inbox](gallery/idea-inbox.webp)

**Input library.** Kept ideas in folders, with their source images, ready to start projects from.

![Input library](input-library.png)

**Projects.** Each project with its cover and its counts of good, neutral, bad and new images, in folders.

![Projects](gallery/projects-grid.webp)

**Generate: a style sweep.** One prompt in several styles. Prompt enhance suggested values for its blanks, `{cat_gear}` and `{sky}`, and one of each is picked.

![Generate: a style sweep](generate.png)

**Reference images.** Two vintage travel posters as `<image1>` and `<image2>` for a new one. Prompt enhance rewrote the prompt and has suggestions for its `{foreground}` blank.

![Reference images](gallery/generate-refs.webp)

**Setting sweep.** The same prompt and seeds at five CFG values, with the time it will take before you queue it.

![Setting sweep](gallery/setting-sweep.webp)

**Prompt history.** Every version of a project's prompt and settings, to compare with the current one or restore. Here the prompt before prompt enhance's rewrite.

![Prompt history](gallery/prompt-history.webp)

**Crop and mark.** An output as the start image of the next run, cropped, with marks to show what to change.

![Crop and mark](gallery/crop-mark.webp)

**Jobs.** Queued runs, spread over the servers. Finish a run's current images and stop, or stop at once.

![Jobs](gallery/jobs.webp)

**Review.** New images in a grid, each rated good, neutral or bad with a key.

![Review](review-grid.png)

**A/B compare.** Two images side by side, to pick the better one.

![A/B compare](review-compare.png)

**Contact sheet.** A style sweep with a row per style and a column per seed. Rate a whole row at once, or ask for more seeds of the styles you kept.

![Contact sheet](gallery/contact-sheet.webp)

**Tile.** Picked images laid out as one image with a title and captions, to save or copy.

![Tile](gallery/tile-window.webp)

**Output library: lineage.** Every kept image with where it came from (its idea, references and recipe) and what was made from it.

![Output library: lineage](output-lineage.png)

**Export.** Copy kept images out of the library, with or without their metadata.

![Export](gallery/export.webp)

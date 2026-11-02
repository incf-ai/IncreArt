# IncreArt GUI screenshots

The real app (`crates/increart-gui`, binary `increart`) on a fresh copy of
the sample library with no servers connected. Taken by the app's screenshot
tour, which drives the screens through the gui-core reducer, on a private Xvfb
display, then saved as WebP:

```sh
cargo run --release -p increart-core --example sample_library tmp/gui-tour/library
scripts/headless.sh env INCREART_CONFIG=$PWD/tmp/gui-tour/config.rsn \
    XDG_STATE_HOME=$PWD/tmp/gui-tour/state \
    INCREART_TOUR=$PWD/tmp/gui-tour/shots INCREART_TOUR_LIBRARY=$PWD/tmp/gui-tour/library \
    target/release/increart
for f in tmp/gui-tour/shots/*.png; do
    magick "$f" -quality 85 -define webp:method=6 "docs/gui/$(basename "$f" .png).webp"
done
```

That runs the main scenes. The rest (`EXTRA_SCENES` in
`crates/increart-gui/src/tour.rs`) run when named by prefix in
`INCREART_TOUR_SCENES`, a few at a time after the ones they need (its comments
say which), each on a fresh library: e.g. `01,02-server-dialog,60-` for server
settings, `01,02-server-dialog,03-idea-inbox-grid,74` for the alerts. The
`60b`–`60g` scenes want a config whose sd-server is the suite's fake
(`cargo build --release -p increart-apitest --features fake`), and the logs
scenes some server logs in the state folder (passwords masked: the Logs
window shows them as written).

**[Gallery](GALLERY.md)**: every screen below, one after another at full
size (regenerate it with `scripts/make-gui-gallery.sh`).

| | |
| --- | --- |
| [01-library-dialog](01-library-dialog.webp) | Start-up: what a library is and why one is opened first; recent libraries (last used in the accent colour), Create a new library… / Open an existing library… |
| [02-server-dialog](02-server-dialog.webp) | Start-up: server presets, each with its servers and what they do (images · text · vision) (a click opens one; ✎ edit, 🗑 delete), Open the last used, New preset / Set up KoboldCpp for your GPUs…, Server settings… |
| [02c-koboldcpp-entry](02c-koboldcpp-entry.webp) | The preset editor with a KoboldCpp row: started by IncreArt or already running, its model and the prompt enhancer's chat model and mmproj, the command line |
| [03-idea-inbox-grid](03-idea-inbox-grid.webp) | Idea inbox grid: each card's Save / Discard, keys panel, card-size zoom, the drop-folder hint |
| [03c-idea-inbox-save-dialog](03c-idea-inbox-save-dialog.webp) | Idea inbox → Save to input library… (`1` / `2` / `S`): the library's folders on the left (a filter, the tree, New folder under it), the idea's name and where it's saved on the right; Save & new project |
| [04-idea-inbox-new-idea](04-idea-inbox-new-idea.webp) | New idea form |
| [04b-idea-inbox-new-idea-save-dialog](04b-idea-inbox-new-idea-save-dialog.webp) | New idea → Add & save to input library…: the save dialog on the new idea |
| [05-input-library](05-input-library.webp) | Input library: folder tree, selection actions, cards |
| [06-projects-grid](06-projects-grid.webp) | Projects: tree and project tiles (cover, counts); the details pane on the right with the folder's totals |
| [06b-projects-pane](06b-projects-pane.webp) | Projects → Grid, a project selected: the details pane (cover, counts, source ideas, latest run, recent runs, best outputs; Generate / Review / Show outputs / Open folder pinned at the bottom) |
| [06c-projects-pane-run](06c-projects-pane-run.webp) | Projects → Folder inside a run: the details pane shows the run (prompt, settings, input images, seeds, revisions; Open in Generate / Show outputs / Details) |
| [07-projects-generate](07-projects-generate.webp) | Generate: the form, and the Images pane on the right (each image a `<imageN>` reference, the start image, both, or parked) |
| [07b-generate-blanks](07b-generate-blanks.webp) | Generate, a single run whose prompt has blanks: the **Blanks** table under the prompt (a value each, prompt enhance's suggestions, Suggest for the empty ones), filled in when queued |
| [08-projects-jobs](08-projects-jobs.webp) | Jobs: servers and running runs; the order (Spread all · Sweep by sweep · One at a time) and Random |
| [09-review](09-review.webp) | Review: scope, image, recipe, verdicts, filmstrip |
| [10-output-library](10-output-library.webp) | Output library: all projects, Kept tab (the Grid's preset tabs: Kept · Good · Rejected · Unreviewed · All); Grid · Folder · Discarded (deleted images, restorable) |
| [11-jobs-popover](11-jobs-popover.webp) | Status bar jobs indicator and its popover (engine state simulated by the tour) |
| [12-review-grid](12-review-grid.webp) | Review → Grid |
| [13-review-compare](13-review-compare.webp) | Review → A/B compare: B (the next image, which ← → change) on the left, A pinned on the right with its own buttons |
| [13c-review-compare-with](13c-review-compare-with.webp) | Review → Compare with… (`Shift+C`): the picker opens on the image's references, its project's next; one click makes it A |
| [13d-review-compare-input](13d-review-compare-input.webp) | Review → A/B compare against an input image: A has no rating buttons, the keys rate B |
| [13e-review-collect](13e-review-collect.webp) | Review → Add to collection…: the image, a filter, the collections (none yet here: New collection…), and Rate (don't rate / good / neutral / bad), the button saying what happens |
| [13f-review-image-menu](13f-review-image-menu.webp) | Review → right-click on the image: pick, copy, crop, compare, the uses with their keys, Add to collection…, Tag… (`L`), Show in file manager |
| [13g-review-tag](13g-review-tag.webp) | Review → Tag… (`L`) on two images: the stack, the field (Enter adds what's typed), the tags ticked on all, on some ("1 / 2") or added, the Rate row (here bad), the button saying what happens |
| [13h-review-use-ref](13h-review-use-ref.webp) | Review → Use as a reference… (`U`) on two picked images: the stack, To (the project's Generate form / the input library, `R`), the Rate row (neutral to start), the button saying what happens |
| [14-output-library-rejected](14-output-library-rejected.webp) | Output library → Grid → Rejected tab: the source's bad images, with Delete these N rejected… (they go to Discarded, under the project's `discarded/`) |
| [14b-output-library-compare](14b-output-library-compare.webp) | Output library → right-click → Compare with…: the Grid shows A beside the selected image (B), ←/→ step B |
| [14c-output-library-tags](14c-output-library-tags.webp) | Output library filtered by the tag "sky" (Tags under the filters, with counts; by tag in the smart folders), the selected image's tags as chips in the lineage view |
| [15-generate-img2img](15-generate-img2img.webp) | Generate in a project set to img2img: start image with strength, refs counter |
| [16-servers-opening](16-servers-opening.webp) | Server dialog while opening: locked and dimmed, progress, Continue in background / Cancel |
| [17-prompt-history](17-prompt-history.webp) | Generate history: entries newest first with the settings changed and the runs they led to, word diff and settings against the current form, Restore all / prompt only / Copy |
| [18-edit-idea](18-edit-idea.webp) | Edit idea window (title, tags, text with history, images, variants) |
| [19-idea-inbox-import](19-idea-inbox-import.webp) | Idea inbox → Import: waiting files, Import now, recent imports with conversions |
| [20-input-library-filters](20-input-library-filters.webp) | Input library: filters (starred only), Grid · Folder · Discarded |
| [21-resolutions](21-resolutions.webp) | Resolutions editor (aspect, MP, time estimate, row actions) |
| [21b-model-presets](21b-model-presets.webp) | Model presets editor (the gear beside Generate's Model preset): which open servers run each, its recipe, files, steps, CFG, flow shift and sigmas |
| [22-output-library-lineage](22-output-library-lineage.webp) | Output library: sources (collections, smart folders), filters, lineage and recipe of an edited image |
| [23-export](23-export.webp) | Export dialog |
| [24-projects-grid-tile-actions](24-projects-grid-tile-actions.webp) | Projects grid: the selected tile shows Review / Generate (scene 25 clicks Generate and checks the result) |
| [26-projects-folder](26-projects-folder.webp) | Projects → Folder: folder view of `projects/` (two-tone icons; folders show images from inside them, projects their cover with a rocket badge; back / forward / up, path bar, Icons · Details, metadata toggle, refresh, open in file manager) |
| [27-projects-folder-images](27-projects-folder-images.webp) | Folder view inside a project's `unreviewed/`: image links with thumbnails (double-click opens Review) |
| [28-projects-folder-details](28-projects-folder-details.webp) | Folder view, Details mode: name, kind, info, size, modified |
| [29-projects-folder-project](29-projects-folder-project.webp) | Folder view inside a project: review folders and `runs/` with their images behind the icons |
| [30-projects-folder-runs](30-projects-folder-runs.webp) | Folder view of `runs/`: each run shows its own outputs behind a film-strip icon |
| [38-scale-dialog](38-scale-dialog.webp) | Scale dialog (Generate → Scale…, or a row of the resolutions editor): to megapixels, a width or a height, keeping the aspect; multiples of 32 |
| [39-output-library-folder](39-output-library-folder.webp) | Output library → Folder: collections, with the virtual read-only `smart/` folder first |
| [40-output-library-smart-folder](40-output-library-smart-folder.webp) | Output library → Folder: `smart › by project` (copy images from here into a collection) |
| [42-input-library-folder](42-input-library-folder.webp) | Input library → Folder: folders and ideas (purple, titled; their images behind) |
| [43-idea-inbox-folder](43-idea-inbox-folder.webp) | Idea inbox → Folder: `input/unreviewed/`, one folder per idea |
| [44-projects-folder-compact](44-projects-folder-compact.webp) | Folder view, Compact mode: small thumbnails / icons and names in columns |
| [45-reference-preview](45-reference-preview.webp) | Generate: double-click a reference (or the start image) to view it large |
| [47-new-project-name](47-new-project-name.webp) | Review → D / N: the new project's name and folder are asked first (the project folders on the left, starting on the image's project's folder; New folder under them) |
| [49-output-library-show-outputs](49-output-library-show-outputs.webp) | Show outputs on a project with a filter on: the banner says how many it hides, with Reset filters |
| [50-output-library-show-outputs-empty](50-output-library-show-outputs-empty.webp) | Show outputs on a project with nothing kept: the banner says so |
| [51-output-library-show-outputs-any-rating](51-output-library-show-outputs-any-rating.webp) | The same with every rating ticked: All, and still nothing |
| [52-server-dialog-edit](52-server-dialog-edit.webp) | The preset editor modal: name, server rows, What each server does (Images · Text (prompt enhance) · Vision), the listed devices to add, Save / Save and open / Open without saving / Cancel |
| [53-library-wizard](53-library-wizard.webp) | Create a new library: home folder, current folder or elsewhere |
| [55-library-open-wizard](55-library-open-wizard.webp) | Open an existing library: the folder, checked for library.rsn |
| [54-server-wizard](54-server-wizard.webp) | Server setup wizard (no presets), KoboldCpp: the GPUs through a runtime (Vulkan here; CUDA or ROCm) as its tool lists them (discrete ones ticked), each one's VRAM preset and role, how two GPUs share the work (by default 2 images at a time, prompt enhance ticked on every GPU with room for it beside the images, here both; **Prompt enhance on** ticks its GPUs), the presets table with the one in use highlighted, the summary, the plan and why; Download, save as preset and open |
| [54a-server-wizard-choose](54a-server-wizard-choose.webp) | Server setup wizard, **Choose presets myself**: GPU 1 set to the 32 GB preset from its menu, highlighted in the table, with a warning that it leaves little VRAM |
| [54c-server-wizard-pe-own-gpu](54c-server-wizard-pe-own-gpu.webp) | Server setup wizard, "Prompt enhance on a GPU of its own": GPU 1 makes images, GPU 2 runs only prompt enhance (answers at once, never swapped) |
| [54d-server-wizard-pe-on-one](54d-server-wizard-pe-on-one.webp) | Server setup wizard, prompt enhance unticked on GPU 2 by hand: only GPU 1 runs the chat model |
| [54b-server-wizard-sd-server](54b-server-wizard-sd-server.webp) | Server setup wizard, sd-server: the listed GPUs ticked, or connect to a running server |
| [56-server-status](56-server-status.webp) | Top bar Servers…: the server status window (here without servers) |
| [57-no-server-warning](57-no-server-warning.webp) | First Queue of a session with no server: Set up servers… / Queue anyway / Cancel |
| [58-server-dialog-from-main](58-server-dialog-from-main.webp) | The server dialog reopened from the main window: Close goes back |
| [59-continue-without-servers](59-continue-without-servers.webp) | Continue without servers asks first |
| [60-server-settings](60-server-settings.webp) | Server settings modal: KoboldCpp (binary, its version), sd-server (binary, devices), both (models folder, leave running, wait for free RAM, log server traffic), timeouts |
| [60b-server-test-button](60b-server-test-button.webp) | The preset editor: a connected-to server's **Test** button |
| [60c-server-test](60c-server-test.webp) | Test of a server: the engine's calls one by one (answers, model list, status, loads a model preset, makes a small image, cancels a job), each passed or why not; Copy, Traffic log… |
| [60d-validate-servers-button](60d-validate-servers-button.webp) | The Servers screen: each preset marked validated or not, **Validate servers…** beside New preset |
| [60e-validate-servers](60e-validate-servers.webp) | Validate servers, basic view: a preset, what validating it does and its checks as plain steps; Start validation / Skip and open |
| [60e2-validate-servers-summary](60e2-validate-servers-summary.webp) | Validation passed: "works with IncreArt", marked validated on the Servers screen, every check folded below; Open the preset, Run again, Copy report, Show report zip |
| [60e3-validate-servers-advanced](60e3-validate-servers-advanced.webp) | Validate servers → Advanced view…: every case of the suite in a tree, checked to run, with the last run's counts and log |
| [60e4-validate-servers-failed](60e4-validate-servers-failed.webp) | Validation failed (a box that isn't there): What went wrong with a **Try:** hint, the steps not checked and why; Open anyway |
| [60f-validate-servers-run](60f-validate-servers-run.webp) | The advanced view after a run: each case's mark and time |
| [60g-validate-first](60g-validate-first.webp) | Opening a preset not validated yet: validate it first (a few minutes), or open without |
| [61-duplicate-project](61-duplicate-project.webp) | A project's right-click Duplicate: name the copy first ("stickers copy") |
| [63-projects-search](63-projects-search.webp) | Projects side panel: New project / New folder on top, search ("fox": matching projects and the folders holding them) |
| [62-rename](62-rename.webp) | Rename modal (folders, projects, collections; also right after New folder / New project) |
| [62b-move-project](62b-move-project.webp) | A project's (or project folder's) right-click Move…: the project folders on the left (a filter, the tree, New folder under it), starting on the folder it is in; from and to on the right |
| [64-prompt-presets](64-prompt-presets.webp) | Generate → Presets…: tasks on the left, Modify style's presets under their group headings; the prompt replaced by Solid colour background, its `{colour}` selected and flagged; the library menu at the top right |
| [65-prompt-presets-search](65-prompt-presets-search.webp) | Presets search ("clay") across every task |
| [66-preset-editor](66-preset-editor.webp) | Edit presets…: tasks, the task's presets, the selected preset's fields (Claymation) |
| [67-save-preset](67-save-preset.webp) | Save as preset…: the current prompt as a new preset |
| [68-switch-library](68-switch-library.webp) | Switching libraries from the top bar: the library screen with Back to the open one |
| [69-style-sweep](69-style-sweep.webp) | Generate in Style sweep mode: the template with `{preset}`, 8 random styles from Modify style, the summary and the chosen names |
| [70-contact-sheet](70-contact-sheet.webp) | Review → a sweep's Contact sheet: a row per style with its counts and row buttons, a column per seed, `…` for an image still to come |
| [71-job-info](71-job-info.webp) | Projects → Jobs, a run clicked: its details with progress (waiting for a server / generating on …), prompt, the input images labelled `<image1>` / start image, the settings, and its Generation log (Copy, Show file) |
| [72-review-sweep-button](72-review-sweep-button.webp) | Review → Single on a sweep's image: **Sweep r02 contact sheet** in the top bar |
| [73-new-project-from-ideas](73-new-project-from-ideas.webp) | New project from ideas: the project folders on the left (New folder under them), the new project's name, always asked (the first idea title as the guess) |
| [74-notification-settings](74-notification-settings.webp) | Settings → Notifications: sound when a GPU idles, when a queue clears, when the review backlog grows; the sound player, volume, only when unfocused |
| [74b-timeout-settings](74b-timeout-settings.webp) | Settings → Timeouts: server start, image requests, prompt enhance, model downloads |
| [74c-bell-ringing](74c-bell-ringing.webp) | The status bar bell filled while an alert holds (here 18 images waiting for review), its menu: snooze 5 min / 30 min / 1 h / for… / until restart |
| [74d-snooze-dialog](74d-snooze-dialog.webp) | Snooze for…: a time typed in (45, 1h30, 1.5h, 1:30), checked as it's typed |
| [98h5-server-logs-folded](98h5-server-logs-folded.webp) | Logs: a KoboldCpp's latest launch, its lines timed, runs of one kind folded (`load_hparams: 8 lines`), find (Ctrl+F), errors only, wrap, follow; Save for a bug report… |
| [98h6-server-logs-errors-only](98h6-server-logs-errors-only.webp) | Logs → errors only: warnings and errors (amber, red) with the lines around them |
| [98i-server-start-failed](98i-server-start-failed.webp) | The Servers screen after a preset's servers failed to start: the error under each row, its log a click away |
| [99a-home](99a-home.webp) | Home, where the app opens: Start a new project… / Write an idea / Import images… / Open the drop folder, what's waiting, how it works in six cards (Collect, Sort the inbox, Input library, Projects, Review, Output library; each with its keys, a count and its button), the latest projects |
| [99b-home-start-project](99b-home-start-project.webp) | Home → Start a new project…: the name, and whether to add reference images (from the libraries, image files, or not now); the picker opens on its Generate form once it is made |

**[Gallery](GALLERY.md)**: every screen above, one after another at full size.

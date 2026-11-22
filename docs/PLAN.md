# Plan: sweep axes and ephemeral mode

Agreed on 2026-11-13. Every step is built; DESIGN.md describes them.

## Build order

1. **Tag axis** (built): a sweep with a run per value of a blank.
2. **Several axes** (built): the combining rules, the verdict counts per
   value and the pivot contact sheet.
3. **LoRA presets**, the LoRA axis and the weight axis (built).
4. **Ephemeral mode** (built).

Steps 1–3 don't depend on step 4.

## 1. Tag axis

- Vary: **Tag**. The prompt has a blank (`{hair}`); the sweep has a list of
  values, one a line, and makes a run per value with the same seeds in each.
- Values are typed or taken from prompt enhance's suggestions for the blank.
- A run is tagged `param: SweepParam { name: "{hair}", value: "red" }` and
  labelled `r05 · red`; its row on the contact sheet is the value.
- The blank's shared value isn't used; other blanks must be filled.

## 2. Several axes

- A sweep is a list of axes (**Add axis**) and a rule for combining them.
  Any axis kind can be one: style, tag (one per blank), a setting, model
  preset, image, and later LoRA and LoRA weight.
- Model preset sweeps stay an axis of their own: each run takes its
  preset's steps, CFG, sampler and schedule, which LoRAs often need.
- Combining rules:
  - **Every combination**: A × B × C.
  - **Paired**: value 1 of A with value 1 of B, and so on.
  - **Random K**: K combinations picked from the full grid, repeatable (the
    pick's seed is kept in the draft; Reroll picks again).
  - **Random per image**: every seed gets its own random combination.
- Seeds are the same in every combination (except random per image).
- Queue asks first from 100 images, as now.
- A run records one param per axis (`params`); a run with the old single
  `param` reads as a one-axis sweep.
- Contact sheet:
  - a pivot: one axis as rows, one as columns, the others and the seed as
    steppers;
  - a flat view (a row per combination, a column per seed) for random
    sweeps, whose grid has holes;
  - a summary of verdict counts per value of each axis ("pixel art LoRA: 9
    good of 14").

## 3. LoRA presets and axes

- **LoRA presets**: a list in the config beside the model presets, each a
  name, a file, a weight and trigger words.
- **LoRA axis**: tick LoRA presets, and "no LoRA"; each is applied over the
  form's model preset. No reloads: LoRAs are sent per request.
- **LoRA weight axis**: values such as 0.6, 0.8, 1.0.
- Trigger words go before the prompt as a model preset's do. A LoRA
  preset's are filled in from a model preset that names its file.
- A row can stack LoRA presets ("A + B").

## 4. Ephemeral mode

For privacy: nothing of the session reaches the disk unless exported.

- **Storage**: an overlay file system in RAM, inside the app, on every
  platform (Windows has no RAM disk of its own). Reads fall through to the
  library on disk where there is one; every write stays in RAM. The
  library's file access (about 400 calls in 43 files) goes through one
  layer with a disk backend and the overlay.
- **Two modes**, chosen at startup:
  - **Inputs from a library**: an existing library is read, never written.
    Outputs, new inputs (ideas added, images imported, outputs sent to the
    input library), drafts, history and every other change are in the
    overlay only, and gone at quit.
  - **New ephemeral library**: an empty library wholly in RAM, inputs
    included.
- **RAM budget**: asked at startup, in GiB. When it is full, the oldest
  images are deleted, unreviewed ones included (the default), or jobs halt
  until images are reviewed (the option).
- **Export** is the only way to disk. With no real paths, opening the file
  manager and dragging files out are unavailable.
- **Outside the library**: the app log, server logs and traffic log go to
  RAM or are off; the recent-libraries list and load timings aren't
  updated; notifications name nothing; nothing goes to the system trash.
- **Swap**: the app asks the OS to keep the overlay's memory out of swap,
  and warns when it can't.
- **Not covered**: a server keeps prompts in memory (its conditioning cache)
  until restarted, and servers the app didn't start keep their own logs.
  Said at startup.

## Left out

- An image sweep filled from an output tag (**From tag…**: one edit over
  every image tagged "bad hands"). Discussed; not asked for.

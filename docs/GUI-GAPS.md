# Real GUI vs POC1: gaps

What POC1 (`tmp/gui-prototype`, [POC1.md](POC1.md)) had that the real app
(`crates/increart-gui`) lacked on 2026-10-24, and where each stands. ✓ = done.

## Idea inbox (POC1: Grid · Folder · New idea · Import · Discarded)

| Item | Status |
| --- | --- |
| Grid with keys panel, card menu, desktop drop | ✓ |
| New idea form (title, text, tags, images, three add buttons) | ✓ |
| Discarded tab | ✓ |
| **Edit idea** window (Enter / T / double-click: title, tags, text, variants, images) | ✓ (this round) |
| **Import** tab: waiting files with Import now, recent imports with conversions / duplicates | ✓ (this round) |
| **Folder** tab (folder view of `inbox_drop/`, `unreviewed/`, `discarded/`; ideas as folders, Edit idea, Import now) | ✓ |
| PE rewrite states, diff, Accept / Reject, Rewrite with PE | needs the PE client |

## Input library (POC1: Grid · Folder · Discarded)

| Item | Status |
| --- | --- |
| Grid, input collections tree (nested; new / rename / delete), multi-select, new project from ideas, add as refs | ✓ |
| **Discarded** tab | ✓ (this round) |
| Left-panel **filters**: text, tag, starred, has images, used in a project / not | ✓ (this round) |
| Collections pane as the Output library's (All ideas, All collections, counts, menus); drag cards onto a collection; Add to collection…; Remove from this collection | ✓ |
| Smart folders: by date, by source, by project, by tag, with images, starred, not in a collection; the Folder tab's virtual `smart/` | ✓ |
| **Folder** tab (collections and idea entries: new collection, rename, move, copy, remove; the tree drives it) | ✓ |

## Projects (POC1: Grid · Folder · Generate · Jobs)

| Item | Status |
| --- | --- |
| Tree, Grid tiles, Generate, Jobs, tile menu (generate more, duplicate, …) | ✓ |
| Grid **breadcrumb** (scope path, click to go up) | ✓ (this round) |
| **Delete confirmation** listing folders, projects, images and pending seeds | ✓ (this round) |
| **Resolutions editor** (from Generate) | ✓ (this round) |
| **Prompt history** (new, not in POC1) | ✓ (this round) |
| **Folder** tab (Thunar-like: path bar, back / forward / up, Icons · Details, thumbnails, open, rename, copy / cut / paste, rate, delete, keys) | ✓ |
| Drag and drop (tree folders and projects, grid tiles, folder-view entries and selections, onto tree folders, folder entries, the path bar or empty space; Ctrl copies) and copy / cut / paste in the folder view | ✓ |

## Review (POC1: Single · Grid · A/B compare)

| Item | Status |
| --- | --- |
| Single / Grid / A/B compare, keys, undo, use-image actions | ✓ |
| Scope panel as a **tree** (folders with unreviewed counts) | ✓ (this round) |
| Full **recipe** panel (project, run, seed, recipe, size, steps/CFG, alpha, refs, parent, GPU, elapsed, sd.cpp, prompt_sent) | ✓ (this round) |
| **Checkerboard behind alpha** toggle | ✓ (this round) |
| **newest ›** jump | ✓ (this round) |

## Output library (POC1: Grid · Folder · Rejected)

| Item | Status |
| --- | --- |
| Grid, Rejected, sources (all kept, good only, by project) | ✓ |
| **Filters**: prompt text, has refs, has alpha | ✓ (this round) |
| **Smart folders**: by date, by rating, by source idea, by project, transparent | ✓ (this round) |
| **Collections**: tree in the sources, Add to collection ▸ / New collection, remove | ✓ (this round) |
| **Lineage** panel (source idea → parent ← image → children) with recipe | ✓ (this round) |
| **Use as project cover** | ✓ (this round) |
| **Export…** dialog (destination, metadata on/off, names) | ✓ (this round) |
| **Show outputs** on projects and project folders (menus, tile button, breadcrumb) → Grid on that source, with a banner for active filters (Reset) or no kept images (show all, bad included; Review them); lineage links back to the project (new) | ✓ (this round) |
| **Folder** tab: collections (new, rename, move, copy, remove; images moved / copied between them) with a virtual read-only `smart/` folder (by date, rating, source idea, project, transparent; copy images out into collections); own location and history; the sources panel drives it | ✓ |

## Shared

| Item | Status |
| --- | --- |
| **Folder-view component** (path bar, back/up, Icons / Compact / Details, thumbnails, open, rename/delete, rubber band, arrows, type-ahead), one instance per Folder tab with its own location and history | ✓ |
| **Open in file manager** on folders, projects, ideas, collections, images, the drop folder and the library (new) | ✓ (this round) |
| **Prompt history** for project prompts, the New idea form and idea texts (new) | ✓ (this round) |

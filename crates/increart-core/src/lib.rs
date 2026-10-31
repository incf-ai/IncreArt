//! IncreArt core: the on-disk library (see `docs/DESIGN.md`, Data model and
//! Filesystem layout).
//!
//! - [`Library`]: open/create a library, hold `library.lock`, and every
//!   operation that changes it (import, ideas, folders, projects, runs,
//!   outputs, collections, clean up).
//! - [`model`]: the RSN file types (`idea.rsn`, `project.rsn`, `run.rsn`,
//!   the `increart:recipe` chunk, …).
//! - [`config`]: the user's `config.rsn` (resolutions).
//! - [`id`], [`pixel`], [`png`], [`link`]: IDs, the pixel hash, PNG chunks and
//!   store links.

pub mod applog;
pub mod archive;
pub mod bugreport;
pub mod check;
pub mod config;
pub mod container;
pub mod engine;
pub mod error;
pub mod export;
pub mod fsutil;
pub mod history;
pub mod id;
pub mod import;
pub mod kobold;
pub mod kobold_image;
pub mod launcher;
pub mod library;
pub mod link;
pub mod markup;
pub mod model;
pub mod modelhash;
pub mod notify;
pub mod pe;
pub mod pixel;
pub mod png;
pub mod process;
pub mod prompts;
pub mod request;
pub mod rsnfmt;
pub mod scan;
pub mod scheduler;
pub mod sdserver;
pub mod thumbs;
pub mod tiers;
pub mod traffic;
pub mod webp;

pub use error::{Error, Result};
pub use id::{Id, IdKind};
pub use import::Original;
pub use library::{Disposal, HistoryKey, Imported, Library, NewIdea, NewRun, Tree};
pub use model::{ImageRef, Store};
pub use scan::{IdeaLoc, Index, Verdict};

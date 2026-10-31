//! IncreArt's GUI logic without the GUI: [`AppState`], the [`update`]
//! reducer (`Msg` → state + `Effect`s) and the [`Runtime`] that carries the
//! effects out. The egui crate only draws state and sends messages.

pub mod runtime;
pub mod state;
pub mod tile;
pub mod timing;
pub mod update;
pub mod watch;

pub use runtime::Runtime;
pub use state::*;
pub use update::{
    AddIdea, BLANK_RETRIES, DraftSave, Effect, LibOp, Msg, OpOk, PeUse, Queued, Restore, SelectHow, TileTarget,
    from_ideas_folder, from_ideas_problem, kobold_binary, save_idea_folder, save_idea_problem, suggest_message, update,
};

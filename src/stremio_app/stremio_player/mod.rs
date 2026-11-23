pub mod player;
pub use player::{get_chapters, get_playback_stats, set_glsl_shaders, Player};
pub mod thumbnailer;
pub mod communication;
pub use communication::{
    CmdVal, InMsg, InMsgArgs, InMsgFn, PlayerEnded, PlayerEvent, PlayerProprChange, PlayerResponse,
    PropKey, PropVal,
};
#[cfg(test)]
mod communication_tests;

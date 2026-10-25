mod job;
mod process;
pub mod server;
pub use process::ServerEvent;
pub use server::{
    default_server_path, download_and_extract_ffmpeg, find_server_script, is_ffmpeg_installed,
    is_streaming_server_online, server_dir, StremioServer,
};

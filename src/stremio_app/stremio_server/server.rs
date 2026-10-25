use super::process::{ServerEvent, ServerProcess};
use crate::stremio_app::constants::STREMIO_SERVER_DEV_MODE;
use native_windows_gui::{self as nwg, PartialUi};
use std::{env, fs, os::windows::process::CommandExt, process::Command, time::Duration};
use winapi::um::winbase::CREATE_NO_WINDOW;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct StremioServer {
    pub notice: nwg::Notice,
    process: std::cell::RefCell<Option<ServerProcess>>,
}

use std::path::PathBuf;

pub fn server_dir() -> PathBuf {
    let dir = if let Some(cfg) = dirs::config_dir() {
        cfg.join("stremio-enhanced").join("streamingserver")
    } else if let Some(home) = dirs::home_dir() {
        home.join("AppData").join("Roaming").join("stremio-enhanced").join("streamingserver")
    } else {
        PathBuf::from("streamingserver")
    };
    let _ = fs::create_dir_all(&dir);
    cleanup_ffmpeg_leftovers(&dir);
    dir
}

pub fn ffmpeg_download_url() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-winarm64-gpl.zip"
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if let Ok(arch) = env::var("PROCESSOR_ARCHITECTURE") {
            if arch.eq_ignore_ascii_case("ARM64") {
                return "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-winarm64-gpl.zip";
            }
        }
        "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
    }
}

pub fn find_shipped_bin_dir() -> Option<PathBuf> {
    if let Ok(mut app_dir) = env::current_exe() {
        app_dir.pop();
        if app_dir.join("ffmpeg.exe").exists() && app_dir.join("ffprobe.exe").exists() {
            return Some(app_dir);
        }
    }

    let dev_bin = if cfg!(target_arch = "aarch64") {
        PathBuf::from("bin-arm64")
    } else {
        PathBuf::from("bin")
    };
    if dev_bin.join("ffmpeg.exe").exists() && dev_bin.join("ffprobe.exe").exists() {
        return Some(dev_bin);
    }

    None
}

pub fn is_ffmpeg_installed() -> bool {
    let dir = server_dir();
    if dir.join("ffmpeg.exe").exists() && dir.join("ffprobe.exe").exists() {
        return true;
    }
    find_shipped_bin_dir().is_some()
}

pub fn cleanup_ffmpeg_leftovers(dir: &std::path::Path) {
    let ffmpeg_exe = dir.join("ffmpeg.exe");
    let ffprobe_exe = dir.join("ffprobe.exe");
    let bin_dir = dir.join("bin");

    if bin_dir.exists() {
        let bin_ffmpeg = bin_dir.join("ffmpeg.exe");
        let bin_ffprobe = bin_dir.join("ffprobe.exe");
        if !ffmpeg_exe.exists() && bin_ffmpeg.exists() {
            let _ = fs::rename(&bin_ffmpeg, &ffmpeg_exe);
        }
        if !ffprobe_exe.exists() && bin_ffprobe.exists() {
            let _ = fs::rename(&bin_ffprobe, &ffprobe_exe);
        }
        if ffmpeg_exe.exists() && ffprobe_exe.exists() {
            let _ = fs::remove_dir_all(&bin_dir);
        }
    }

    let doc_dir = dir.join("doc");
    if doc_dir.exists() {
        let _ = fs::remove_dir_all(&doc_dir);
    }

    let presets_dir = dir.join("presets");
    if presets_dir.exists() {
        let _ = fs::remove_dir_all(&presets_dir);
    }

    let license_file = dir.join("LICENSE.txt");
    if license_file.exists() {
        let _ = fs::remove_file(&license_file);
    }
}

pub fn download_and_extract_ffmpeg() -> Result<(), String> {
    let dir = server_dir();
    let _ = fs::create_dir_all(&dir);

    let ffmpeg_exe = dir.join("ffmpeg.exe");
    let ffprobe_exe = dir.join("ffprobe.exe");

    cleanup_ffmpeg_leftovers(&dir);

    if ffmpeg_exe.exists() && ffprobe_exe.exists() {
        return Ok(());
    }

    let url = ffmpeg_download_url();
    println!("[FFmpeg] Downloading FFmpeg from {url} to {}...", dir.display());

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let response = client
        .get(url)
        .header("User-Agent", "stremio-shell-ng")
        .send()
        .map_err(|e| format!("Failed to download FFmpeg archive: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("FFmpeg download returned status {}", response.status()));
    }

    let bytes = response.bytes().map_err(|e| format!("Failed to read FFmpeg bytes: {e}"))?;
    println!("[FFmpeg] Download completed ({} bytes). Extracting binaries...", bytes.len());

    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_dir = std::env::temp_dir().join(format!("stremio_ffmpeg_{}_{}", std::process::id(), millis));
    let _ = fs::create_dir_all(&temp_dir);

    let cursor = std::io::Cursor::new(bytes);
    let extract_result = zip_extract::extract(cursor, &temp_dir, true);
    if let Err(e) = extract_result {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err(format!("Failed to extract FFmpeg zip: {e}"));
    }

    find_and_move_binary(&temp_dir, "ffmpeg.exe", &ffmpeg_exe);
    find_and_move_binary(&temp_dir, "ffprobe.exe", &ffprobe_exe);

    let _ = fs::remove_dir_all(&temp_dir);
    cleanup_ffmpeg_leftovers(&dir);

    if ffmpeg_exe.exists() && ffprobe_exe.exists() {
        println!("[FFmpeg] FFmpeg setup complete: {}", ffmpeg_exe.display());
        Ok(())
    } else {
        Err("Archive did not contain ffmpeg.exe and ffprobe.exe".to_string())
    }
}

fn find_and_move_binary(dir: &std::path::Path, filename: &str, dest: &std::path::Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.eq_ignore_ascii_case(filename) {
                        if fs::rename(&path, dest).is_err() {
                            let _ = fs::copy(&path, dest);
                        }
                        return;
                    }
                }
            } else if path.is_dir() {
                find_and_move_binary(&path, filename, dest);
                if dest.exists() {
                    return;
                }
            }
        }
    }
}

pub fn default_server_path() -> PathBuf {
    server_dir().join("server.js")
}

pub fn find_server_script() -> Option<PathBuf> {
    let default_path = default_server_path();
    if default_path.exists() {
        return Some(default_path);
    }

    if let Ok(path_str) = env::var("SERVER_PATH") {
        let path = PathBuf::from(&path_str);
        if path.exists() {
            return Some(path);
        }
    }

    if let Ok(mut path) = env::current_exe() {
        path.pop();
        let app_path = path.join("server.js");
        if app_path.exists() {
            return Some(app_path);
        }
    }

    let cwd_path = PathBuf::from("server.js");
    if cwd_path.exists() {
        return Some(cwd_path);
    }

    None
}

pub fn find_runtime_exe() -> Result<PathBuf, String> {
    if let Ok(mut path) = env::current_exe() {
        path.pop();
        let runtime = path.join("stremio-runtime.exe");
        if runtime.exists() {
            return Ok(runtime);
        }
    }

    let dev_bin = if cfg!(target_arch = "aarch64") {
        PathBuf::from("bin-arm64").join("stremio-runtime.exe")
    } else {
        PathBuf::from("bin").join("stremio-runtime.exe")
    };
    if dev_bin.exists() {
        return Ok(dev_bin);
    }

    Ok(PathBuf::from("node"))
}

pub fn is_streaming_server_online() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11470".parse().unwrap(),
        std::time::Duration::from_millis(150),
    ).is_ok()
}

impl StremioServer {
    pub fn development(&self) -> bool {
        self.process.borrow().is_none()
    }

    pub fn start(&self) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.start();
        }
    }

    pub fn restart(&self) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.stop();
        }
        let sender = self.notice.sender();
        let process = ServerProcess::new(
            server_command,
            STARTUP_TIMEOUT,
            move || {
                sender.notice();
            },
        );
        process.start();
        *self.process.borrow_mut() = Some(process);
    }

    pub fn stop(&self) {
        if let Some(process) = self.process.borrow().as_ref() {
            process.stop();
        }
    }

    pub fn events(&self) -> Vec<ServerEvent> {
        if let Some(process) = self.process.borrow().as_ref() {
            process.events().collect()
        } else {
            Vec::new()
        }
    }
}

fn server_command() -> Result<Command, String> {
    super::job::protect_shell()
        .map_err(|error| format!("Cannot supervise Stremio child processes: {error}"))?;
    let runtime = find_runtime_exe()?;
    let server_js = find_server_script()
        .ok_or_else(|| "server.js not found in expected locations".to_string())?;

    if !is_ffmpeg_installed() {
        println!("[StreamingServer] ffmpeg missing, downloading ffmpeg...");
        let _ = download_and_extract_ffmpeg();
    }

    let s_dir = server_dir();
    let mut command = Command::new(runtime);
    command
        .arg(&server_js)
        .current_dir(&s_dir)
        .creation_flags(CREATE_NO_WINDOW);

    let mut path_entries = vec![s_dir.display().to_string()];
    if let Some(shipped) = find_shipped_bin_dir() {
        if let Ok(abs) = shipped.canonicalize() {
            path_entries.push(abs.display().to_string());
        } else {
            path_entries.push(shipped.display().to_string());
        }
    }
    if let Ok(path_var) = env::var("PATH") {
        path_entries.push(path_var);
    }
    command.env("PATH", path_entries.join(";"));

    Ok(command)
}

impl PartialUi for StremioServer {
    fn build_partial<W: Into<nwg::ControlHandle>>(
        data: &mut Self,
        parent: Option<W>,
    ) -> Result<(), nwg::NwgError> {
        let parent = parent.expect("No parent window").into();
        nwg::Notice::builder()
            .parent(parent)
            .build(&mut data.notice)?;

        if env::var(STREMIO_SERVER_DEV_MODE).as_deref() != Ok("true") {
            let sender = data.notice.sender();
            *data.process.borrow_mut() = Some(ServerProcess::new(
                server_command,
                STARTUP_TIMEOUT,
                move || {
                    sender.notice();
                },
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cleanup_ffmpeg_leftovers() {
        let temp = std::env::temp_dir().join(format!("test_ffmpeg_clean_{}", rand::random::<u32>()));
        let _ = fs::create_dir_all(&temp);
        let bin = temp.join("bin");
        let doc = temp.join("doc");
        let presets = temp.join("presets");
        let license = temp.join("LICENSE.txt");
        let _ = fs::create_dir_all(&bin);
        let _ = fs::create_dir_all(&doc);
        let _ = fs::create_dir_all(&presets);
        let _ = fs::write(&license, "license");
        let _ = fs::write(bin.join("ffmpeg.exe"), "bin_ffmpeg");
        let _ = fs::write(bin.join("ffprobe.exe"), "bin_ffprobe");

        cleanup_ffmpeg_leftovers(&temp);
        assert!(temp.join("ffmpeg.exe").exists());
        assert!(temp.join("ffprobe.exe").exists());
        assert!(!bin.exists());
        assert!(!doc.exists());
        assert!(!presets.exists());
        assert!(!license.exists());

        let _ = fs::remove_dir_all(&temp);
    }
}

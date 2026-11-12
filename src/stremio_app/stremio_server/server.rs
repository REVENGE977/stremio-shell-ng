use super::process::{ServerEvent, ServerProcess};
use crate::stremio_app::constants::STREMIO_SERVER_DEV_MODE;
use native_windows_gui::{self as nwg, PartialUi};
use std::{
    env, fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use winapi::um::winbase::CREATE_NO_WINDOW;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct StremioServer {
    pub notice: nwg::Notice,
    process: std::cell::RefCell<Option<ServerProcess>>,
}

pub fn server_dir() -> PathBuf {
    let dir = crate::stremio_app::enhanced::base_dir().join("streamingserver");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn clean_abs_path(path: PathBuf) -> PathBuf {
    let abs = if path.is_absolute() {
        path
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path
    };
    if let Ok(canon) = abs.canonicalize() {
        let s = canon.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
        return canon;
    }
    abs
}

fn arch_bin_dir_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "bin-arm64"
    } else {
        "bin"
    }
}

fn has_ffmpeg_pair(dir: &Path) -> bool {
    dir.join("ffmpeg.exe").is_file() && dir.join("ffprobe.exe").is_file()
}

pub fn find_shipped_bin_dir() -> Option<PathBuf> {
    let arch_dir = arch_bin_dir_name();
    if let Ok(mut app_dir) = env::current_exe() {
        app_dir.pop();
        if has_ffmpeg_pair(&app_dir) {
            return Some(clean_abs_path(app_dir));
        }
        let app_sub_bin = app_dir.join(arch_dir);
        if has_ffmpeg_pair(&app_sub_bin) {
            return Some(clean_abs_path(app_sub_bin));
        }
    }

    let dev_bin = PathBuf::from(arch_dir);
    if has_ffmpeg_pair(&dev_bin) {
        return Some(clean_abs_path(dev_bin));
    }

    None
}

pub fn find_ffmpeg_in_path() -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .find(|dir| has_ffmpeg_pair(dir))
        .map(clean_abs_path)
}

pub fn find_ffmpeg_bin_dir() -> Option<PathBuf> {
    if let Some(shipped) = find_shipped_bin_dir() {
        return Some(shipped);
    }
    let dir = server_dir();
    if has_ffmpeg_pair(&dir) {
        return Some(clean_abs_path(dir));
    }
    find_ffmpeg_in_path()
}

pub fn is_ffmpeg_installed() -> bool {
    find_ffmpeg_bin_dir().is_some()
}

/// Checks whether FFmpeg binaries (`ffmpeg.exe` and `ffprobe.exe`) are available
/// in the shipped application directory, streaming server directory, or `PATH`.
pub fn ensure_ffmpeg_available() -> Result<(), String> {
    if is_ffmpeg_installed() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg.exe and ffprobe.exe were not found. Reinstall Stremio Enhanced or place them in {}",
            server_dir().display()
        ))
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
    let arch_dir = arch_bin_dir_name();
    if let Ok(mut path) = env::current_exe() {
        path.pop();
        let runtime = path.join("stremio-runtime.exe");
        if runtime.exists() {
            return Ok(clean_abs_path(runtime));
        }
        let sub_runtime = path.join(arch_dir).join("stremio-runtime.exe");
        if sub_runtime.exists() {
            return Ok(clean_abs_path(sub_runtime));
        }
    }

    let dev_bin = PathBuf::from(arch_dir).join("stremio-runtime.exe");
    if dev_bin.exists() {
        return Ok(clean_abs_path(dev_bin));
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

fn apply_ffmpeg_env(command: &mut Command, s_dir: &Path, bin_dir: &Path) {
    command.env("FFMPEG_BIN", bin_dir.join("ffmpeg.exe"));
    command.env("FFPROBE_BIN", bin_dir.join("ffprobe.exe"));

    let mut path_entries = vec![
        bin_dir.display().to_string(),
        s_dir.display().to_string(),
    ];
    if let Ok(path_var) = env::var("PATH") {
        path_entries.push(path_var);
    }
    command.env("PATH", path_entries.join(";"));
}

fn server_command() -> Result<Command, String> {
    super::job::protect_shell()
        .map_err(|error| format!("Cannot supervise Stremio child processes: {error}"))?;
    let runtime = find_runtime_exe()?;
    let server_js = find_server_script()
        .ok_or_else(|| "server.js not found in expected locations".to_string())?;
    let bin_dir = find_ffmpeg_bin_dir().ok_or_else(|| {
        format!(
            "ffmpeg.exe and ffprobe.exe were not found. Reinstall Stremio Enhanced or place them in {}",
            server_dir().display()
        )
    })?;

    let s_dir = server_dir();
    let mut command = Command::new(runtime);
    command
        .arg(&server_js)
        .current_dir(&s_dir)
        .creation_flags(CREATE_NO_WINDOW);
    apply_ffmpeg_env(&mut command, &s_dir, &bin_dir);

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
    fn test_has_ffmpeg_pair() {
        let temp = std::env::temp_dir().join(format!("test_ffmpeg_pair_{}", rand::random::<u32>()));
        let _ = fs::create_dir_all(&temp);
        assert!(!has_ffmpeg_pair(&temp));

        fs::write(temp.join("ffmpeg.exe"), b"ffmpeg").unwrap();
        assert!(!has_ffmpeg_pair(&temp));

        fs::write(temp.join("ffprobe.exe"), b"ffprobe").unwrap();
        assert!(has_ffmpeg_pair(&temp));

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_apply_ffmpeg_env_uses_bundled_bin() {
        let bin_dir = find_shipped_bin_dir().expect("bundled bin dir should exist in workspace");
        assert!(bin_dir.join("ffmpeg.exe").is_file());
        assert!(bin_dir.join("ffprobe.exe").is_file());

        let mut cmd = Command::new(find_runtime_exe().unwrap());
        let s_dir = server_dir();
        apply_ffmpeg_env(&mut cmd, &s_dir, &bin_dir);

        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert_eq!(
            envs.get("FFMPEG_BIN").map(PathBuf::from),
            Some(bin_dir.join("ffmpeg.exe"))
        );
        assert_eq!(
            envs.get("FFPROBE_BIN").map(PathBuf::from),
            Some(bin_dir.join("ffprobe.exe"))
        );
        assert!(envs
            .get("PATH")
            .is_some_and(|p| p.starts_with(&bin_dir.display().to_string())));
    }
}

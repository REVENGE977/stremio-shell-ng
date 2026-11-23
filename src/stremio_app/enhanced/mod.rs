pub mod shaders;

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use once_cell::sync::Lazy;
use serde::Deserialize;
use url::Url;

pub const ENHANCED_SCRIPT: &str = include_str!("enhanced.js");

static HTTP_CLIENT: Lazy<reqwest::blocking::Client> = Lazy::new(|| {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("StremioEnhanced/5.0")
        .build()
        .expect("Failed to initialize HTTP client")
});

/// Metadata parsed from plugin/theme file headers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    pub update_url: String,
    pub version: String,
    pub author: String,
}

/// A loaded plugin with its metadata and script content.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub file_name: String,
    pub metadata: Metadata,
    pub content: String,
}

/// A loaded theme with its metadata and CSS content.
#[derive(Debug, Clone)]
pub struct Theme {
    pub file_name: String,
    pub metadata: Metadata,
    pub content: String,
}

fn deserialize_enabled_plugins<'de, D>(deserializer: D) -> Result<HashMap<String, bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum PluginsRepr {
        Map(HashMap<String, bool>),
        List(Vec<String>),
    }

    match Option::<PluginsRepr>::deserialize(deserializer)? {
        Some(PluginsRepr::Map(map)) => Ok(map),
        Some(PluginsRepr::List(list)) => {
            let mut map = HashMap::new();
            for item in list {
                map.insert(item, true);
            }
            Ok(map)
        }
        None => Ok(HashMap::new()),
    }
}

pub const DEFAULT_THUMBNAIL_PREVIEW_SIZE: u32 = 240;
pub const MIN_THUMBNAIL_PREVIEW_SIZE: u32 = 160;
pub const MAX_THUMBNAIL_PREVIEW_SIZE: u32 = 360;

fn default_true() -> bool {
    true
}

fn default_shader_preset() -> String {
    "off".to_string()
}

fn default_thumbnail_preview_size() -> u32 {
    DEFAULT_THUMBNAIL_PREVIEW_SIZE
}

pub fn clamp_thumbnail_preview_size(size: u32) -> u32 {
    size.clamp(MIN_THUMBNAIL_PREVIEW_SIZE, MAX_THUMBNAIL_PREVIEW_SIZE)
}

/// Persistent settings stored in `settings.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(
        default,
        alias = "enabled_plugins",
        deserialize_with = "deserialize_enabled_plugins"
    )]
    pub enabled_plugins: HashMap<String, bool>,
    #[serde(default, alias = "active_theme")]
    pub active_theme: Option<String>,
    #[serde(default = "default_true", alias = "thumbnail_preview")]
    pub thumbnail_preview: bool,
    #[serde(default = "default_true", alias = "thumbnail_precache")]
    pub thumbnail_precache: bool,
    #[serde(
        default = "default_thumbnail_preview_size",
        alias = "thumbnail_preview_size"
    )]
    pub thumbnail_preview_size: u32,
    #[serde(default = "default_true", alias = "discord_rpc")]
    pub discord_rpc: bool,
    #[serde(default = "default_shader_preset", alias = "shader_preset")]
    pub shader_preset: String,
    #[serde(default = "default_true", alias = "chapter_markers")]
    pub chapter_markers: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled_plugins: HashMap::new(),
            active_theme: None,
            thumbnail_preview: true,
            thumbnail_precache: true,
            thumbnail_preview_size: DEFAULT_THUMBNAIL_PREVIEW_SIZE,
            discord_rpc: true,
            shader_preset: default_shader_preset(),
            chapter_markers: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatusUiData {
    pub exists: bool,
    pub running: bool,
    pub server_dir: String,
    pub server_path: String,
    pub current_version: Option<String>,
    pub skipped_version: Option<String>,
    #[serde(default)]
    pub ffmpeg_installed: bool,
}

/// Incoming IPC action from the web interface.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EnhancedAction {
    GetData,
    OpenThemesFolder,
    OpenPluginsFolder,
    OpenStreamingServerFolder,
    GetServerStatus {
        id: String,
    },
    StartStreamingServer {
        id: String,
    },
    LoadUrl {
        url: String,
    },
    PrepareServerUpdate {
        version: String,
    },
    SkipServerVersion {
        version: String,
    },
    SkipClientVersion {
        version: String,
    },
    TogglePlugin {
        file_name: String,
        enabled: bool,
    },
    ApplyTheme {
        file_name: String,
    },
    Log {
        level: String,
        message: String,
    },
    HttpGet {
        id: String,
        url: String,
    },
    InstallMod {
        mod_type: String,
        file_name: String,
        content: String,
    },
    DeleteMod {
        mod_type: String,
        file_name: String,
    },
    DownloadFfmpeg {
        id: String,
    },
    SetPictureInPicture {
        enabled: bool,
    },
    StartPipDrag,
    StartPipResize {
        direction: u32,
    },
    RequestThumbnail {
        time: f64,
        max_width: Option<u32>,
    },
    SetThumbnailPreview {
        enabled: bool,
    },
    SetThumbnailPrecache {
        enabled: bool,
    },
    SetThumbnailPreviewSize {
        size: u32,
    },
    SetDiscordRpc {
        enabled: bool,
    },
    ExportSettings {
        id: String,
        #[serde(default)]
        plugin_settings: Option<serde_json::Map<String, serde_json::Value>>,
    },
    ImportSettings {
        id: String,
    },
    SetShaderPreset {
        id: String,
        preset: String,
    },
    OpenShadersFolder,
    GetPlaybackStats {
        id: String,
    },
    GetChapters,
    SetChapterMarkers {
        enabled: bool,
    },
}

/// Serialized plugin information for the web UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUiData {
    pub file_name: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub update_url: String,
    pub content: String,
    pub enabled: bool,
}

/// Serialized theme information for the web UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeUiData {
    pub file_name: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub update_url: String,
    pub css: String,
    pub is_active: bool,
}

/// Consolidated enhanced data sent to the web UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnhancedUiData {
    pub plugins: Vec<PluginUiData>,
    pub themes: Vec<ThemeUiData>,
    pub active_theme: Option<String>,
    pub server_status: ServerStatusUiData,
    pub client_version: String,
    pub skipped_client_version: Option<String>,
    pub thumbnail_preview: bool,
    pub thumbnail_precache: bool,
    pub thumbnail_preview_size: u32,
    pub discord_rpc: bool,
    pub shader_preset: String,
    pub shader_presets: Vec<shaders::ShaderPresetUiData>,
    pub chapter_markers: bool,
}

pub fn base_dir() -> PathBuf {
    if let Some(cfg) = dirs::config_dir() {
        return cfg.join("stremio-enhanced");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join("AppData").join("Roaming").join("stremio-enhanced");
    }
    PathBuf::from("stremio-enhanced")
}

/// The main manager that handles loading plugins/themes and tracking settings.
pub struct EnhancedManager {
    #[allow(dead_code)]
    pub base_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub themes_dir: PathBuf,
    pub shaders_dir: PathBuf,
    pub settings_path: PathBuf,
    pub settings: Settings,
    pub plugins: Vec<Plugin>,
    pub themes: Vec<Theme>,
}

impl Default for EnhancedManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EnhancedManager {
    /// Creates a new `EnhancedManager`, creating the directory structure if needed.
    pub fn new() -> Self {
        let base_dir = base_dir();
        let plugins_dir = base_dir.join("plugins");
        let themes_dir = base_dir.join("themes");
        let shaders_dir = base_dir.join("shaders");
        let settings_path = base_dir.join("settings.json");

        // Ensure directories exist
        let streaming_server_dir = base_dir.join("streamingserver");
        if let Err(e) = fs::create_dir_all(&streaming_server_dir) {
            eprintln!("[Enhanced] Failed to create streaming server directory: {e}");
        }
        if let Err(e) = fs::create_dir_all(&plugins_dir) {
            eprintln!("[Enhanced] Failed to create plugins directory: {e}");
        }
        if let Err(e) = fs::create_dir_all(&themes_dir) {
            eprintln!("[Enhanced] Failed to create themes directory: {e}");
        }
        if let Err(e) = fs::create_dir_all(&shaders_dir) {
            eprintln!("[Enhanced] Failed to create shaders directory: {e}");
        }

        let settings = Self::load_settings(&settings_path);

        let mut manager = Self {
            base_dir,
            plugins_dir,
            themes_dir,
            shaders_dir,
            settings_path,
            settings,
            plugins: Vec::new(),
            themes: Vec::new(),
        };

        manager.discover_plugins();
        manager.discover_themes();
        manager.save_settings();

        manager
    }

    /// Reload plugins, themes, and settings from disk.
    pub fn reload(&mut self) {
        self.settings = Self::load_settings(&self.settings_path);
        self.plugins.clear();
        self.themes.clear();
        self.discover_plugins();
        self.discover_themes();
        self.save_settings();
    }

    /// Toggle a plugin's enabled status.
    pub fn toggle_plugin(&mut self, file_name: &str, enabled: bool) {
        self.settings
            .enabled_plugins
            .insert(file_name.to_string(), enabled);
        self.save_settings();
    }

    /// Retrieve a plugin by file name.
    #[allow(dead_code)]
    pub fn get_plugin(&self, file_name: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.file_name == file_name)
    }

    /// Set or clear the active theme. Returns the CSS of the active theme if any.
    pub fn set_active_theme(&mut self, file_name: Option<&str>) -> Option<String> {
        self.settings.active_theme = file_name.map(str::to_string);
        self.save_settings();
        self.active_theme().map(|t| t.content.clone())
    }

    /// Enable or disable ThumbFast seek-bar thumbnail previews.
    pub fn set_thumbnail_preview(&mut self, enabled: bool) {
        self.settings.thumbnail_preview = enabled;
        self.save_settings();
    }

    /// Enable or disable background storyboard pre-caching for ThumbFast.
    pub fn set_thumbnail_precache(&mut self, enabled: bool) {
        self.settings.thumbnail_precache = enabled;
        self.save_settings();
    }

    /// Set the ThumbFast preview card width (clamped to supported bounds).
    pub fn set_thumbnail_preview_size(&mut self, size: u32) {
        self.settings.thumbnail_preview_size = clamp_thumbnail_preview_size(size);
        self.save_settings();
    }

    /// Enable or disable Discord Rich Presence integration.
    pub fn set_discord_rpc(&mut self, enabled: bool) {
        self.settings.discord_rpc = enabled;
        self.save_settings();
    }

    /// Persist active GLSL shader preset ID.
    pub fn set_shader_preset(&mut self, preset: &str) {
        self.settings.shader_preset = preset.to_string();
        self.save_settings();
    }

    /// Enable or disable chapter markers on the player seek bar.
    pub fn set_chapter_markers(&mut self, enabled: bool) {
        self.settings.chapter_markers = enabled;
        self.save_settings();
    }

    /// Open the shaders directory in the default file manager.
    pub fn open_shaders_folder(&self) {
        open_folder(&self.shaders_dir);
    }

    /// Build a JSON backup string of `settings.json` plus optional `plugin_setting:*` localStorage entries.
    pub fn build_export_json(
        &self,
        plugin_settings: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Result<String, String> {
        let mut root = match serde_json::to_value(&self.settings).map_err(|e| e.to_string())? {
            serde_json::Value::Object(map) => map,
            _ => return Err("Failed to serialize settings object".to_string()),
        };

        if let Some(ps) = plugin_settings {
            let filtered: serde_json::Map<String, serde_json::Value> = ps
                .iter()
                .filter(|(k, _)| k.starts_with("plugin_setting:"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !filtered.is_empty() {
                root.insert(
                    "pluginSettings".to_string(),
                    serde_json::Value::Object(filtered),
                );
            }
        }

        serde_json::to_string_pretty(&serde_json::Value::Object(root)).map_err(|e| e.to_string())
    }

    /// Parse, validate, and apply an imported settings JSON string, returning any extracted `plugin_setting:*` map.
    pub fn import_settings_json(
        &mut self,
        raw_json: &str,
    ) -> Result<Option<serde_json::Map<String, serde_json::Value>>, String> {
        let val: serde_json::Value =
            serde_json::from_str(raw_json).map_err(|e| format!("Invalid JSON: {e}"))?;
        let obj = val
            .as_object()
            .ok_or_else(|| "Settings file must contain a JSON object".to_string())?;

        let plugin_settings = obj
            .get("pluginSettings")
            .and_then(|v| v.as_object())
            .map(|ps| {
                ps.iter()
                    .filter(|(k, _)| k.starts_with("plugin_setting:"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<serde_json::Map<String, serde_json::Value>>()
            })
            .filter(|m| !m.is_empty());

        let mut imported = serde_json::from_value::<Settings>(val)
            .map_err(|e| format!("Invalid settings format: {e}"))?;
        imported.thumbnail_preview_size =
            clamp_thumbnail_preview_size(imported.thumbnail_preview_size);

        if let Some(ref active) = imported.active_theme {
            if let Some(matched) = self
                .themes
                .iter()
                .find(|t| t.file_name.eq_ignore_ascii_case(active))
            {
                imported.active_theme = Some(matched.file_name.clone());
            } else {
                eprintln!(
                    "[Enhanced] Imported active theme '{}' not found in themes directory, clearing selection",
                    active
                );
                imported.active_theme = None;
            }
        }

        for plugin in &self.plugins {
            imported
                .enabled_plugins
                .entry(plugin.file_name.clone())
                .or_insert(false);
        }

        self.settings = imported;
        self.save_settings();
        Ok(plugin_settings)
    }

    /// Open the themes directory in the default file manager.
    pub fn open_themes_folder(&self) {
        open_folder(&self.themes_dir);
    }

    /// Open the plugins directory in the default file manager.
    pub fn open_plugins_folder(&self) {
        open_folder(&self.plugins_dir);
    }

    /// Installs a plugin or theme file and reloads.
    pub fn install_mod(
        &mut self,
        mod_type: &str,
        file_name: &str,
        content: &str,
    ) -> std::io::Result<()> {
        let safe_name = sanitize_mod_file_name(mod_type, file_name)?;
        let dir = if mod_type.eq_ignore_ascii_case("theme") {
            &self.themes_dir
        } else {
            &self.plugins_dir
        };
        let target = dir.join(safe_name);
        fs::write(&target, content)?;
        println!("[Enhanced] Installed {mod_type} to {}", target.display());
        self.reload();
        Ok(())
    }

    /// Deletes a plugin or theme file and reloads.
    pub fn delete_mod(&mut self, mod_type: &str, file_name: &str) -> std::io::Result<()> {
        let safe_name = sanitize_mod_file_name(mod_type, file_name)?;
        let dir = if mod_type.eq_ignore_ascii_case("theme") {
            &self.themes_dir
        } else {
            &self.plugins_dir
        };
        let target = dir.join(safe_name);
        if target.exists() {
            fs::remove_file(&target)?;
            println!("[Enhanced] Deleted {mod_type} file {}", target.display());
        }
        if mod_type.eq_ignore_ascii_case("theme") {
            if self
                .settings
                .active_theme
                .as_deref()
                .is_some_and(|active| active.eq_ignore_ascii_case(safe_name))
            {
                self.settings.active_theme = None;
                self.save_settings();
            }
        } else {
            self.settings.enabled_plugins.remove(safe_name);
            self.save_settings();
        }
        self.reload();
        Ok(())
    }

    /// Returns the current streaming server status and paths.
    pub fn get_server_status(&self, running: bool) -> ServerStatusUiData {
        let server_dir = crate::stremio_app::stremio_server::server_dir();
        let server_path = crate::stremio_app::stremio_server::default_server_path();
        let exists = crate::stremio_app::stremio_server::find_server_script().is_some();
        let is_running = running || crate::stremio_app::stremio_server::is_streaming_server_online();

        let current_version_file = server_dir.join("version.txt");
        let current_version = fs::read_to_string(&current_version_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let skip_version_file = self.base_dir.join("skip_server_version.txt");
        let skipped_version = fs::read_to_string(&skip_version_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        ServerStatusUiData {
            exists,
            running: is_running,
            server_dir: server_dir.to_string_lossy().to_string(),
            server_path: server_path.to_string_lossy().to_string(),
            current_version,
            skipped_version,
            ffmpeg_installed: crate::stremio_app::stremio_server::is_ffmpeg_installed(),
        }
    }

    /// Open the streaming server directory in the default file manager.
    pub fn open_streaming_server_folder(&self) {
        let dir = crate::stremio_app::stremio_server::server_dir();
        let _ = fs::create_dir_all(&dir);
        open_folder(&dir);
    }

    /// Prepares for a server.js update by deleting old files and recording the new version.
    pub fn prepare_server_update(&self, new_version: &str) -> std::io::Result<()> {
        let dir = crate::stremio_app::stremio_server::server_dir();
        let _ = fs::create_dir_all(&dir);
        let server_file = dir.join("server.js");
        let version_file = dir.join("version.txt");

        if server_file.exists() {
            let _ = fs::remove_file(&server_file);
        }
        fs::write(&version_file, new_version)?;
        Ok(())
    }

    /// Saves a version string to skip future update prompts for that version.
    pub fn skip_server_version(&self, version: &str) -> std::io::Result<()> {
        let skip_file = self.base_dir.join("skip_server_version.txt");
        fs::write(&skip_file, version)?;
        Ok(())
    }

    /// Saves a client version string to skip future client update prompts for that version.
    pub fn skip_client_version(&self, version: &str) -> std::io::Result<()> {
        let skip_file = self.base_dir.join("skip_client_version.txt");
        fs::write(&skip_file, version)?;
        Ok(())
    }

    /// Formats plugins, themes, and server status for the web interface.
    pub fn ui_data(&self, server_running: bool) -> EnhancedUiData {
        let plugins = self
            .plugins
            .iter()
            .map(|p| {
                let enabled = self
                    .settings
                    .enabled_plugins
                    .get(&p.file_name)
                    .copied()
                    .unwrap_or(false);
                PluginUiData {
                    file_name: p.file_name.clone(),
                    name: p.metadata.name.clone(),
                    description: p.metadata.description.clone(),
                    version: p.metadata.version.clone(),
                    author: p.metadata.author.clone(),
                    update_url: p.metadata.update_url.clone(),
                    content: p.content.clone(),
                    enabled,
                }
            })
            .collect();

        let themes = self
            .themes
            .iter()
            .map(|t| {
                let is_active = self
                    .settings
                    .active_theme
                    .as_deref()
                    .is_some_and(|active| active.eq_ignore_ascii_case(&t.file_name));
                ThemeUiData {
                    file_name: t.file_name.clone(),
                    name: t.metadata.name.clone(),
                    description: t.metadata.description.clone(),
                    version: t.metadata.version.clone(),
                    author: t.metadata.author.clone(),
                    update_url: t.metadata.update_url.clone(),
                    css: t.content.clone(),
                    is_active,
                }
            })
            .collect();

        let skip_client_file = self.base_dir.join("skip_client_version.txt");
        let skipped_client_version = fs::read_to_string(&skip_client_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        EnhancedUiData {
            plugins,
            themes,
            active_theme: self.settings.active_theme.clone(),
            server_status: self.get_server_status(server_running),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            skipped_client_version,
            thumbnail_preview: self.settings.thumbnail_preview,
            thumbnail_precache: self.settings.thumbnail_precache,
            thumbnail_preview_size: clamp_thumbnail_preview_size(
                self.settings.thumbnail_preview_size,
            ),
            discord_rpc: self.settings.discord_rpc,
            shader_preset: self.settings.shader_preset.clone(),
            shader_presets: shaders::ui_presets(&self.shaders_dir),
            chapter_markers: self.settings.chapter_markers,
        }
    }

    /// Loads settings from disk, or returns defaults if the file doesn't exist.
    fn load_settings(path: &Path) -> Settings {
        match fs::read_to_string(path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
                eprintln!("[Enhanced] Failed to parse settings.json, using defaults: {e}");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// Persists the current settings to disk.
    pub fn save_settings(&self) {
        match serde_json::to_string_pretty(&self.settings) {
            Ok(data) => {
                if let Err(e) = fs::write(&self.settings_path, data) {
                    eprintln!("[Enhanced] Failed to write settings.json: {e}");
                }
            }
            Err(e) => eprintln!("[Enhanced] Failed to serialize settings: {e}"),
        }
    }

    /// Discover all `.plugin.js` or `.js` files in the plugins directory.
    fn discover_plugins(&mut self) {
        let entries = match fs::read_dir(&self.plugins_dir) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("[Enhanced] Failed to read plugins directory: {e}");
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "js") {
                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                match fs::read_to_string(&path) {
                    Ok(content) => {
                        let metadata = parse_metadata(&content).unwrap_or_else(|| {
                            let name = file_name
                                .strip_suffix(".plugin.js")
                                .or_else(|| file_name.strip_suffix(".js"))
                                .unwrap_or(&file_name)
                                .to_string();
                            Metadata {
                                name,
                                description: String::new(),
                                update_url: String::new(),
                                version: "1.0.0".to_string(),
                                author: "Unknown".to_string(),
                            }
                        });

                        println!("[Enhanced] Loaded plugin: {} ({})", metadata.name, file_name);

                        // If this plugin isn't in settings yet, default to disabled
                        self.settings
                            .enabled_plugins
                            .entry(file_name.clone())
                            .or_insert(false);

                        self.plugins.push(Plugin {
                            file_name,
                            metadata,
                            content,
                        });
                    }
                    Err(e) => eprintln!("[Enhanced] Failed to read plugin file '{}': {e}", file_name),
                }
            }
        }
    }

    /// Discover all `.theme.css` or `.css` files in the themes directory.
    fn discover_themes(&mut self) {
        let entries = match fs::read_dir(&self.themes_dir) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("[Enhanced] Failed to read themes directory: {e}");
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "css") {
                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                match fs::read_to_string(&path) {
                    Ok(content) => {
                        let metadata = parse_metadata(&content).unwrap_or_else(|| {
                            let name = file_name
                                .strip_suffix(".theme.css")
                                .or_else(|| file_name.strip_suffix(".css"))
                                .unwrap_or(&file_name)
                                .to_string();
                            Metadata {
                                name,
                                description: String::new(),
                                update_url: String::new(),
                                version: "1.0.0".to_string(),
                                author: "Unknown".to_string(),
                            }
                        });

                        println!("[Enhanced] Loaded theme: {} ({})", metadata.name, file_name);

                        self.themes.push(Theme {
                            file_name,
                            metadata,
                            content,
                        });
                    }
                    Err(e) => eprintln!("[Enhanced] Failed to read theme file '{}': {e}", file_name),
                }
            }
        }

        // Validate that the active theme still exists on disk
        if let Some(ref active) = self.settings.active_theme {
            if let Some(matched) = self
                .themes
                .iter()
                .find(|t| t.file_name.eq_ignore_ascii_case(active))
            {
                // Normalize casing to actual file name on disk
                if &matched.file_name != active {
                    self.settings.active_theme = Some(matched.file_name.clone());
                }
            } else {
                eprintln!(
                    "[Enhanced] Active theme '{}' not found in themes directory, clearing selection",
                    active
                );
                self.settings.active_theme = None;
            }
        }
    }

    /// Returns only the plugins that are currently enabled in settings.
    #[allow(dead_code)]
    pub fn enabled_plugins(&self) -> Vec<&Plugin> {
        self.plugins
            .iter()
            .filter(|p| {
                self.settings
                    .enabled_plugins
                    .get(&p.file_name)
                    .copied()
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Returns the currently active theme, if any.
    pub fn active_theme(&self) -> Option<&Theme> {
        self.settings
            .active_theme
            .as_ref()
            .and_then(|active| {
                self.themes
                    .iter()
                    .find(|t| t.file_name.eq_ignore_ascii_case(active))
            })
    }
}

/// Validates that a plugin or theme filename is a single safe path component with the expected extension.
pub fn sanitize_mod_file_name<'a>(mod_type: &str, file_name: &'a str) -> std::io::Result<&'a str> {
    if file_name.is_empty() || file_name.contains(['/', '\\', '\0']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid mod file name",
        ));
    }
    let mut components = Path::new(file_name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Mod file name must be a single path segment",
            ));
        }
    }
    let expected_ext = if mod_type.eq_ignore_ascii_case("theme") {
        "css"
    } else if mod_type.eq_ignore_ascii_case("plugin") {
        "js"
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unsupported mod type",
        ));
    };
    if !Path::new(file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected_ext))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Mod file name must end with .{expected_ext}"),
        ));
    }
    Ok(file_name)
}

/// Builds the JavaScript snippet that creates or updates the `#activeTheme` `<style>` element.
pub fn theme_injection_script(css: Option<&str>) -> String {
    let css_json = serde_json::to_string(css.unwrap_or_default()).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(function() {{
    try {{
        var s = document.getElementById('activeTheme') || document.createElement('style');
        s.id = 'activeTheme';
        s.textContent = {css_json};
        (document.head || document.documentElement).appendChild(s);
    }} catch(e) {{}}
}})();"#
    )
}

/// Opens a directory in the user's default file manager.
pub fn open_folder(path: &Path) {
    if !path.exists() {
        let _ = fs::create_dir_all(path);
    }
    if let Err(e) = open::that(path) {
        eprintln!("[Enhanced] Failed to open folder via open::that: {e}");
    }
}

/// Helper to fetch HTTP(S) URL content synchronously bypassing CORS via reqwest.
pub fn http_get(url: &str) -> Result<(u16, String), String> {
    let parsed = Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err("Only HTTP(S) URLs with a valid host are allowed".to_string());
    }

    let res = HTTP_CLIENT
        .get(parsed)
        .send()
        .map_err(|e| e.to_string())?;

    let status = res.status().as_u16();
    let body = res.text().unwrap_or_default();
    Ok((status, body))
}

/// Parses the metadata block from the top of a plugin/theme file.
fn parse_metadata(content: &str) -> Option<Metadata> {
    let start = content.find("/**")?;
    let end = content[start..].find("*/")? + start + 2;
    let block = &content[start..end];

    let name = extract_tag(block, "name")?;
    let description = extract_tag(block, "description").unwrap_or_default();
    let update_url = extract_tag(block, "updateUrl").unwrap_or_default();
    let version = extract_tag(block, "version").unwrap_or_default();
    let author = extract_tag(block, "author").unwrap_or_default();

    Some(Metadata {
        name,
        description,
        update_url,
        version,
        author,
    })
}

/// Extracts the value of a `@tag` from a metadata comment block.
fn extract_tag(block: &str, tag: &str) -> Option<String> {
    let target = format!("@{tag}").to_lowercase();
    for line in block.lines() {
        let trimmed = line.trim().trim_start_matches('*').trim();
        let trimmed_lower = trimmed.to_lowercase();
        if trimmed_lower.starts_with(&target) {
            let rest = trimmed[target.len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_actions() {
        let get_data = r#"{"action":"getData"}"#;
        let action: EnhancedAction = serde_json::from_str(get_data).unwrap();
        assert!(matches!(action, EnhancedAction::GetData));

        let toggle = r#"{"action":"togglePlugin","fileName":"tidb.plugin.js","enabled":true}"#;
        let action: EnhancedAction = serde_json::from_str(toggle).unwrap();
        match action {
            EnhancedAction::TogglePlugin { file_name, enabled } => {
                assert_eq!(file_name, "tidb.plugin.js");
                assert_eq!(enabled, true);
            }
            _ => panic!("Wrong action"),
        }

        let theme = r#"{"action":"applyTheme","fileName":"amoled.theme.css"}"#;
        let action: EnhancedAction = serde_json::from_str(theme).unwrap();
        match action {
            EnhancedAction::ApplyTheme { file_name } => {
                assert_eq!(file_name, "amoled.theme.css");
            }
            _ => panic!("Wrong action"),
        }

        let install = r#"{"action":"installMod","modType":"plugin","fileName":"test.js","content":"console.log(1);"}"#;
        let action: EnhancedAction = serde_json::from_str(install).unwrap();
        match action {
            EnhancedAction::InstallMod { mod_type, file_name, content } => {
                assert_eq!(mod_type, "plugin");
                assert_eq!(file_name, "test.js");
                assert_eq!(content, "console.log(1);");
            }
            _ => panic!("Wrong action"),
        }

        let delete = r#"{"action":"deleteMod","modType":"theme","fileName":"dark.css"}"#;
        let action: EnhancedAction = serde_json::from_str(delete).unwrap();
        match action {
            EnhancedAction::DeleteMod { mod_type, file_name } => {
                assert_eq!(mod_type, "theme");
                assert_eq!(file_name, "dark.css");
            }
            _ => panic!("Wrong action"),
        }

        let req_thumb = r#"{"action":"requestThumbnail","time":42.5,"maxWidth":240}"#;
        let action: EnhancedAction = serde_json::from_str(req_thumb).unwrap();
        match action {
            EnhancedAction::RequestThumbnail { time, max_width } => {
                assert!((time - 42.5).abs() < f64::EPSILON);
                assert_eq!(max_width, Some(240));
            }
            _ => panic!("Wrong action"),
        }

        let set_thumb = r#"{"action":"setThumbnailPreview","enabled":false}"#;
        let action: EnhancedAction = serde_json::from_str(set_thumb).unwrap();
        match action {
            EnhancedAction::SetThumbnailPreview { enabled } => {
                assert!(!enabled);
            }
            _ => panic!("Wrong action"),
        }

        let set_precache = r#"{"action":"setThumbnailPrecache","enabled":false}"#;
        let action: EnhancedAction = serde_json::from_str(set_precache).unwrap();
        match action {
            EnhancedAction::SetThumbnailPrecache { enabled } => {
                assert!(!enabled);
            }
            _ => panic!("Wrong action"),
        }

        let set_thumb_size = r#"{"action":"setThumbnailPreviewSize","size":320}"#;
        let action: EnhancedAction = serde_json::from_str(set_thumb_size).unwrap();
        match action {
            EnhancedAction::SetThumbnailPreviewSize { size } => {
                assert_eq!(size, 320);
            }
            _ => panic!("Wrong action"),
        }

        let set_discord = r#"{"action":"setDiscordRpc","enabled":false}"#;
        let action: EnhancedAction = serde_json::from_str(set_discord).unwrap();
        match action {
            EnhancedAction::SetDiscordRpc { enabled } => {
                assert!(!enabled);
            }
            _ => panic!("Wrong action"),
        }

        let export_settings = r#"{"action":"exportSettings","id":"exp-1","pluginSettings":{"plugin_setting:demo:flag":true}}"#;
        let action: EnhancedAction = serde_json::from_str(export_settings).unwrap();
        match action {
            EnhancedAction::ExportSettings { id, plugin_settings } => {
                assert_eq!(id, "exp-1");
                assert_eq!(
                    plugin_settings
                        .as_ref()
                        .and_then(|m| m.get("plugin_setting:demo:flag")),
                    Some(&serde_json::json!(true))
                );
            }
            _ => panic!("Wrong action"),
        }

        let import_settings = r#"{"action":"importSettings","id":"imp-1"}"#;
        let action: EnhancedAction = serde_json::from_str(import_settings).unwrap();
        match action {
            EnhancedAction::ImportSettings { id } => {
                assert_eq!(id, "imp-1");
            }
            _ => panic!("Wrong action"),
        }

        let skip_client = r#"{"action":"skipClientVersion","version":"5.0.27-enhanced.4"}"#;
        let action: EnhancedAction = serde_json::from_str(skip_client).unwrap();
        match action {
            EnhancedAction::SkipClientVersion { version } => {
                assert_eq!(version, "5.0.27-enhanced.4");
            }
            _ => panic!("Wrong action"),
        }

        let set_shader = r#"{"action":"setShaderPreset","id":"sh_1","preset":"anime4k-a-fast"}"#;
        match serde_json::from_str::<EnhancedAction>(set_shader).unwrap() {
            EnhancedAction::SetShaderPreset { id, preset } => {
                assert_eq!(id, "sh_1");
                assert_eq!(preset, "anime4k-a-fast");
            }
            _ => panic!("Wrong action"),
        }

        let open_shaders = r#"{"action":"openShadersFolder"}"#;
        assert!(matches!(
            serde_json::from_str::<EnhancedAction>(open_shaders).unwrap(),
            EnhancedAction::OpenShadersFolder
        ));

        let get_stats = r#"{"action":"getPlaybackStats","id":"st_1"}"#;
        match serde_json::from_str::<EnhancedAction>(get_stats).unwrap() {
            EnhancedAction::GetPlaybackStats { id } => assert_eq!(id, "st_1"),
            _ => panic!("Wrong action"),
        }

        let get_chapters = r#"{"action":"getChapters"}"#;
        assert!(matches!(
            serde_json::from_str::<EnhancedAction>(get_chapters).unwrap(),
            EnhancedAction::GetChapters
        ));

        let set_chapters = r#"{"action":"setChapterMarkers","enabled":false}"#;
        match serde_json::from_str::<EnhancedAction>(set_chapters).unwrap() {
            EnhancedAction::SetChapterMarkers { enabled } => assert!(!enabled),
            _ => panic!("Wrong action"),
        }
    }

    #[test]
    fn test_deserialize_settings() {
        // Snake case map
        let json1 = r#"{"enabled_plugins":{"test.js":true},"active_theme":"dark.css","shader_preset":"fsrcnnx-8","chapter_markers":false}"#;
        let s1: Settings = serde_json::from_str(json1).unwrap();
        assert_eq!(s1.enabled_plugins.get("test.js"), Some(&true));
        assert_eq!(s1.active_theme.as_deref(), Some("dark.css"));
        assert_eq!(s1.shader_preset, "fsrcnnx-8");
        assert!(!s1.chapter_markers);

        // Camel case map
        let json2 = r#"{"enabledPlugins":{"test.js":false},"activeTheme":"amoled.css","shaderPreset":"artcnn-c4f32","chapterMarkers":true}"#;
        let s2: Settings = serde_json::from_str(json2).unwrap();
        assert_eq!(s2.enabled_plugins.get("test.js"), Some(&false));
        assert_eq!(s2.active_theme.as_deref(), Some("amoled.css"));
        assert_eq!(s2.shader_preset, "artcnn-c4f32");
        assert!(s2.chapter_markers);

        // Array list of enabled plugins
        let json3 = r#"{"enabledPlugins":["test1.js","test2.js"],"activeTheme":null}"#;
        let s3: Settings = serde_json::from_str(json3).unwrap();
        assert_eq!(s3.enabled_plugins.get("test1.js"), Some(&true));
        assert_eq!(s3.enabled_plugins.get("test2.js"), Some(&true));
        assert_eq!(s3.active_theme, None);
        assert_eq!(s3.shader_preset, "off");
        assert!(s3.chapter_markers);

        // Empty json
        let json4 = r#"{}"#;
        let s4: Settings = serde_json::from_str(json4).unwrap();
        assert!(s4.enabled_plugins.is_empty());
        assert_eq!(s4.active_theme, None);
        assert!(s4.thumbnail_preview);
        assert!(s4.thumbnail_precache);
        assert_eq!(s4.thumbnail_preview_size, DEFAULT_THUMBNAIL_PREVIEW_SIZE);
        assert!(s4.discord_rpc);
        assert_eq!(s4.shader_preset, "off");
        assert!(s4.chapter_markers);
    }

    #[test]
    fn test_export_and_import_settings_round_trip() {
        let temp_root = std::env::temp_dir().join(format!(
            "stremio_enhanced_settings_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        let plugins_dir = temp_root.join("plugins");
        let themes_dir = temp_root.join("themes");
        let shaders_dir = temp_root.join("shaders");
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::create_dir_all(&themes_dir).unwrap();
        fs::create_dir_all(&shaders_dir).unwrap();

        let mut mgr = EnhancedManager {
            base_dir: temp_root.clone(),
            plugins_dir,
            themes_dir,
            shaders_dir,
            settings_path: temp_root.join("settings.json"),
            settings: Settings {
                enabled_plugins: std::iter::once(("alpha.plugin.js".to_string(), true)).collect(),
                active_theme: Some("amoled.theme.css".to_string()),
                thumbnail_preview: false,
                thumbnail_precache: false,
                thumbnail_preview_size: 320,
                discord_rpc: false,
                shader_preset: "cas-balanced".to_string(),
                chapter_markers: false,
            },
            plugins: vec![Plugin {
                file_name: "beta.plugin.js".to_string(),
                metadata: Metadata {
                    name: "Beta".to_string(),
                    description: String::new(),
                    update_url: String::new(),
                    version: "1.0.0".to_string(),
                    author: "Test".to_string(),
                },
                content: String::new(),
            }],
            themes: vec![Theme {
                file_name: "amoled.theme.css".to_string(),
                metadata: Metadata {
                    name: "AMOLED".to_string(),
                    description: String::new(),
                    update_url: String::new(),
                    version: "1.0.0".to_string(),
                    author: "Test".to_string(),
                },
                content: String::new(),
            }],
        };

        let mut ps = serde_json::Map::new();
        ps.insert(
            "plugin_setting:alpha:token".to_string(),
            serde_json::json!("abc123"),
        );
        ps.insert("unrelated_key".to_string(), serde_json::json!("ignored"));

        let exported = mgr.build_export_json(Some(&ps)).unwrap();
        assert!(exported.contains("plugin_setting:alpha:token"));
        assert!(!exported.contains("unrelated_key"));

        mgr.settings = Settings::default();
        let imported_ps = mgr.import_settings_json(&exported).unwrap().unwrap();
        assert_eq!(
            imported_ps.get("plugin_setting:alpha:token"),
            Some(&serde_json::json!("abc123"))
        );
        assert!(!imported_ps.contains_key("unrelated_key"));
        assert!(!mgr.settings.thumbnail_preview);
        assert!(!mgr.settings.thumbnail_precache);
        assert_eq!(mgr.settings.thumbnail_preview_size, 320);
        assert!(!mgr.settings.discord_rpc);
        assert_eq!(mgr.settings.active_theme.as_deref(), Some("amoled.theme.css"));
        assert_eq!(mgr.settings.enabled_plugins.get("alpha.plugin.js"), Some(&true));
        assert_eq!(mgr.settings.enabled_plugins.get("beta.plugin.js"), Some(&false));

        // Raw settings.json without pluginSettings, with missing theme & out-of-range size
        let raw_json = r#"{"enabledPlugins":{},"activeTheme":"Missing.theme.css","thumbnailPreview":true,"thumbnailPreviewSize":999,"discordRpc":true}"#;
        let imported_none = mgr.import_settings_json(raw_json).unwrap();
        assert!(imported_none.is_none());
        assert_eq!(mgr.settings.thumbnail_preview_size, MAX_THUMBNAIL_PREVIEW_SIZE);
        assert_eq!(mgr.settings.active_theme, None);

        // Rejects non-object / invalid JSON
        assert!(mgr.import_settings_json("[1,2,3]").is_err());
        assert!(mgr.import_settings_json("{not json").is_err());

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn test_sanitize_mod_file_name() {
        assert_eq!(
            sanitize_mod_file_name("plugin", "example.plugin.js").unwrap(),
            "example.plugin.js"
        );
        assert_eq!(
            sanitize_mod_file_name("theme", "amoled.theme.css").unwrap(),
            "amoled.theme.css"
        );
        for bad in [
            "",
            ".",
            "..",
            "../evil.js",
            r"..\evil.js",
            "sub/evil.js",
            "evil.exe",
            "plugin.css",
        ] {
            assert!(sanitize_mod_file_name("plugin", bad).is_err(), "{}", bad);
        }
        assert!(sanitize_mod_file_name("theme", "script.js").is_err());
        assert!(sanitize_mod_file_name("other", "script.js").is_err());
    }

    #[test]
    fn test_http_get_rejects_non_http_schemes() {
        assert!(http_get("file:///C:/Windows/System32/drivers/etc/hosts").is_err());
        assert!(http_get("javascript:alert(1)").is_err());
        assert!(http_get("not-a-url").is_err());
    }
}

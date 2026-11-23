use native_windows_derive::NwgUi;
use native_windows_gui as nwg;
use serde_json;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io::Read,
    path::PathBuf,
    str,
    sync::{Arc, Mutex},
    thread, time,
};
use url::Url;
use winapi::{
    shared::windef::RECT,
    um::winuser::{
        GetWindowLongA, SetWindowPos, GWL_STYLE, SWP_NOACTIVATE, SWP_NOZORDER, WM_DPICHANGED,
        WM_NCCALCSIZE, WS_CAPTION, WS_EX_TOPMOST,
    },
};

use crate::stremio_app::{
    constants::{
        is_trusted_shell_navigation_url, safe_url, web_endpoint_with_streaming_server, APP_NAME,
        UPDATE_INTERVAL, WEB_ENDPOINT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
    },
    enhanced::{shaders, theme_injection_script, EnhancedAction, EnhancedManager},
    ipc::{CacheDirectoryRequest, RPCRequest, RPCResponse},
    splash::SplashImage,
    stremio_player::{
        get_chapters, get_playback_stats, set_glsl_shaders,
        thumbnailer::{spawn_thumbnail_worker, ThumbnailCmd, DEFAULT_THUMB_MAX_WIDTH},
        CmdVal, InMsg, InMsgArgs, InMsgFn, Player,
    },
    stremio_server::{ensure_ffmpeg_available, is_streaming_server_online, ServerEvent, StremioServer},
    stremio_wevbiew::{WebView, WebViewCommand},
    systray::SystemTray,
    updater,
    window_helper::WindowStyle,
    window_settings::WindowSettings,
    PipeServer,
};

use super::discord::DiscordRpc;

const WEBUI_RETRY_DELAYS_SECS: [u64; 2] = [2, 5];
const WEBUI_READY_TIMEOUT_SECS: u64 = 30;
const WEBUI_RETRY_READY_TIMEOUT_SECS: u64 = 15;
const WEBUI_UNREACHABLE: &str = "Stremio couldn't load its interface. \
    Check your internet connection, VPN or firewall, then click Retry.";

pub enum WebUiEvent {
    Ready,
    LoadFailed(String),
    Timeout(u64),
    Retry(u64),
}

type WebUiEvents = Arc<Mutex<VecDeque<WebUiEvent>>>;

fn send_webui_event(events: &WebUiEvents, notice: &nwg::NoticeSender, event: WebUiEvent) {
    events.lock().unwrap().push_back(event);
    notice.notice();
}

pub enum OpenRequest {
    Input(String),
    Ready,
    Reset,
}

#[derive(Clone)]
pub struct ExportSettingsRequest {
    pub id: String,
    pub plugin_settings: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Default, NwgUi)]
pub struct MainWindow {
    pub command: String,
    pub commands_server: RefCell<Option<PipeServer>>,
    pub webui_url: String,
    pub no_splash: bool,
    pub dev_tools: bool,
    pub start_hidden: bool,
    pub autoupdater_endpoint: Option<Url>,
    pub force_update: bool,
    #[allow(dead_code)]
    pub release_candidate: bool,
    pub autoupdater_setup_file: Arc<Mutex<Option<PathBuf>>>,
    pub requested_fullscreen: Arc<Mutex<Option<bool>>>,
    pub requested_pip: Arc<Mutex<Option<bool>>>,
    pub requested_pip_resize: Arc<Mutex<Option<u32>>>,
    pub pip_mode_active: Arc<std::sync::atomic::AtomicBool>,
    pub requested_cache_directory: Arc<Mutex<Option<CacheDirectoryRequest>>>,
    pub requested_export_settings: Arc<Mutex<Option<ExportSettingsRequest>>>,
    pub requested_import_settings: Arc<Mutex<Option<String>>>,
    pub requested_interface_scale: Arc<Mutex<Option<u64>>>,
    pub saved_window_style: RefCell<WindowStyle>,
    pub open_media_sender: RefCell<Option<flume::Sender<OpenRequest>>>,
    pub thumbnail_tx: RefCell<Option<flume::Sender<ThumbnailCmd>>>,
    pub active_stream_url: Arc<Mutex<Option<String>>>,
    pub discord_rpc: RefCell<Option<DiscordRpc>>,
    pub local_server_url: Arc<Mutex<Option<String>>>,
    pub webui_events: WebUiEvents,
    pub webui_ready: Cell<bool>,
    pub webui_retries: Cell<usize>,
    pub webui_generation: Cell<u64>,
    #[nwg_resource]
    pub embed: nwg::EmbedResource,
    #[nwg_resource(source_embed: Some(&data.embed), source_embed_str: Some("MAINICON"))]
    pub window_icon: nwg::Icon,
    #[nwg_resource(title: "Choose cache folder", action: nwg::FileDialogAction::OpenDirectory)]
    pub cache_directory_picker: nwg::FileDialog,
    #[nwg_resource(title: "Export Stremio Enhanced Settings", action: nwg::FileDialogAction::Save, filters: "JSON Files(*.json)")]
    pub export_settings_picker: nwg::FileDialog,
    #[nwg_resource(title: "Import Stremio Enhanced Settings", action: nwg::FileDialogAction::Open, filters: "JSON Files(*.json)")]
    pub import_settings_picker: nwg::FileDialog,
    #[nwg_control(icon: Some(&data.window_icon), title: APP_NAME, flags: "MAIN_WINDOW")]
    #[nwg_events(
        OnWindowClose: [Self::on_quit(SELF, EVT_DATA)],
        OnInit: [Self::on_init],
        OnPaint: [Self::on_paint],
        OnResize: [Self::on_paint],
        OnMinMaxInfo: [Self::on_min_max(SELF, EVT_DATA)],
        OnWindowMinimize: [Self::transmit_window_state_change],
        OnWindowMaximize: [Self::on_window_state_changed],
        OnWindowFocus: [Self::transmit_window_state_change],
        OnResizeEnd: [Self::save_window_settings],
    )]
    pub window: nwg::Window,
    #[nwg_partial(parent: window)]
    #[nwg_events(
        (tray, MousePressLeftUp): [Self::on_show],
        (tray_exit, OnMenuItemSelected): [Self::on_exit],
        (tray_show_hide, OnMenuItemSelected): [Self::on_show_hide],
        (tray_topmost, OnMenuItemSelected): [Self::on_toggle_topmost],
    )]
    pub tray: SystemTray,
    #[nwg_partial(parent: window)]
    pub splash_screen: SplashImage,
    #[nwg_partial(parent: window)]
    #[nwg_events((notice, OnNotice): [Self::on_server_notice])]
    pub server: StremioServer,
    #[nwg_partial(parent: window)]
    pub player: Player,
    #[nwg_partial(parent: window)]
    pub webview: WebView,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_toggle_fullscreen_notice] )]
    pub toggle_fullscreen_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_set_interface_scale_notice] )]
    pub set_interface_scale_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [nwg::stop_thread_dispatch()] )]
    pub quit_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_hide_splash_notice] )]
    pub hide_splash_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_focus_notice] )]
    pub focus_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_cache_directory_notice])]
    pub cache_directory_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_webui_notice])]
    pub webui_notice: nwg::Notice,
    pub enhanced: Arc<Mutex<crate::stremio_app::enhanced::EnhancedManager>>,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_server_restart_notice])]
    pub server_restart_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_server_stop_notice])]
    pub server_stop_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_toggle_pip_notice])]
    pub toggle_pip_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_pip_drag_notice])]
    pub pip_drag_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_pip_resize_notice])]
    pub pip_resize_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_export_settings_notice])]
    pub export_settings_notice: nwg::Notice,
    #[nwg_control]
    #[nwg_events(OnNotice: [Self::on_import_settings_notice])]
    pub import_settings_notice: nwg::Notice,
}

impl MainWindow {
    fn transmit_window_visibility_change(&self) {
        if let (Ok(web_channel), Ok(style)) = (
            self.webview.channel.try_borrow(),
            self.saved_window_style.try_borrow(),
        ) {
            let (web_tx, _) = web_channel
                .as_ref()
                .expect("Cannont obtain communication channel for the Web UI");
            let web_tx_app = web_tx.clone();
            web_tx_app
                .send(RPCResponse::visibility_change(
                    self.window.visible(),
                    style.full_screen as u32,
                    style.full_screen,
                ))
                .ok();
        } else {
            eprintln!("Cannot obtain communication channel or window style");
        }
    }
    fn transmit_window_state_change(&self) {
        if let (Some(hwnd), Ok(web_channel), Ok(style)) = (
            self.window.handle.hwnd(),
            self.webview.channel.try_borrow(),
            self.saved_window_style.try_borrow(),
        ) {
            let state = style.clone().get_window_state(hwnd);
            drop(style);
            let (web_tx, _) = web_channel
                .as_ref()
                .expect("Cannont obtain communication channel for the Web UI");
            let web_tx_app = web_tx.clone();
            web_tx_app.send(RPCResponse::state_change(state)).ok();
        } else {
            eprintln!("Cannot obtain window handle or communication channel");
        }
    }
    fn on_init(&self) {
        self.webview.dev_tools.set(self.dev_tools).ok();
        self.webview.set_enhanced(self.enhanced.clone());
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(mut saved_style) = self.saved_window_style.try_borrow_mut() {
                saved_style.set_title_bar_color(hwnd);
                let pip_mode_raw = self.pip_mode_active.clone();
                nwg::bind_raw_event_handler(&self.window.handle, 0x10001, move |hwnd, msg, w, l| {
                    if msg == WM_NCCALCSIZE && w != 0 && pip_mode_raw.load(std::sync::atomic::Ordering::Relaxed) {
                        return Some(0);
                    }
                    if msg != WM_DPICHANGED
                        || unsafe { GetWindowLongA(hwnd, GWL_STYLE) } as u32 & WS_CAPTION == 0
                    {
                        return None;
                    }
                    let rect = unsafe { &*(l as *const RECT) };
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                    Some(0)
                })
                .ok();
                if let Some(window_settings) = WindowSettings::load() {
                    saved_style
                        .restore_window_placement(hwnd, window_settings.to_window_placement());
                } else {
                    saved_style.center_window(hwnd, WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT);
                }
            }
        }

        self.window.set_visible(!self.start_hidden);
        self.tray.tray_show_hide.set_checked(!self.start_hidden);
        if self.no_splash {
            self.splash_screen.hide();
        }

        let player_channel = self.player.channel.borrow();
        let (player_tx, player_rx) = player_channel
            .as_ref()
            .expect("Cannont obtain communication channel for the Player");
        let player_tx = player_tx.clone();
        let player_rx = player_rx.clone();

        let web_channel = self.webview.channel.borrow();
        let (web_tx, web_rx) = web_channel
            .as_ref()
            .expect("Cannont obtain communication channel for the Web UI");
        let web_tx_player = web_tx.clone();
        let web_rx = web_rx.clone();

        let (updater_tx, updater_rx) = flume::unbounded::<String>();
        let (open_sender, open_receiver) = flume::unbounded();
        *self.open_media_sender.borrow_mut() = Some(open_sender.clone());

        spawn_open_media_worker(self.command.clone(), open_receiver, web_tx.clone());
        spawn_updater_worker(
            self.autoupdater_endpoint.clone(),
            self.force_update,
            self.autoupdater_setup_file.clone(),
            updater_rx,
            web_tx.clone(),
        );

        // Single application IPC
        if let Some(listener) = self.commands_server.borrow_mut().take() {
            spawn_single_instance_listener(listener, self.focus_notice.sender(), open_sender.clone());
        }

        let (initial_thumb_enabled, initial_thumb_precache, initial_thumb_size, initial_shader_paths) = {
            let mgr = self.enhanced.lock().unwrap();
            let shader_paths = if mgr.settings.shader_preset != "off" {
                shaders::resolve_preset_paths(&mgr.shaders_dir, &mgr.settings.shader_preset)
            } else {
                Vec::new()
            };
            (
                mgr.settings.thumbnail_preview,
                mgr.settings.thumbnail_precache,
                mgr.settings.thumbnail_preview_size,
                shader_paths,
            )
        };
        if !initial_shader_paths.is_empty() {
            if let Some(ref mpv) = self.player.mpv {
                set_glsl_shaders(mpv, &initial_shader_paths);
            }
        }
        let (thumbnail_tx, _thumbnail_handle) = spawn_thumbnail_worker(
            self.webview.get_command_sender(),
            initial_thumb_enabled,
            initial_thumb_precache,
            initial_thumb_size,
        );
        *self.thumbnail_tx.borrow_mut() = Some(thumbnail_tx.clone());
        let active_stream_url = self.active_stream_url.clone();

        // Forward messages from player to WebView
        let thumbnail_tx_player = thumbnail_tx.clone();
        let active_stream_url_player = active_stream_url.clone();
        let player_mpv_events = self.player.mpv.clone();
        let webview_cmd_tx_player = self.webview.get_command_sender();
        thread::spawn(move || {
            for msg in player_rx.iter() {
                if msg.contains("mpv-event-video-ready") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&msg) {
                        let is_ready = v
                            .get("args")
                            .and_then(|a| a.get(1))
                            .and_then(|p| p.get("ready"))
                            .and_then(|r| r.as_bool())
                            .unwrap_or(false);
                        if is_ready {
                            let url = active_stream_url_player.lock().unwrap().clone();
                            let _ = thumbnail_tx_player.send(ThumbnailCmd::SetSource(url));
                            if let Some(ref mpv) = player_mpv_events {
                                let chapters = get_chapters(mpv);
                                if let Ok(json) = serde_json::to_string(&chapters) {
                                    send_js_callback(
                                        &webview_cmd_tx_player,
                                        "__receiveChapters",
                                        &[json],
                                    );
                                }
                            }
                        }
                    }
                } else if msg.contains("mpv-event-ended") {
                    let _ = thumbnail_tx_player.send(ThumbnailCmd::SetSource(None));
                    send_js_callback(
                        &webview_cmd_tx_player,
                        "__receiveChapters",
                        &["[]".to_string()],
                    );
                }
                web_tx_player.send(msg).ok();
            }
        });

        let discord_rpc = DiscordRpc::new(web_tx.clone());
        *self.discord_rpc.borrow_mut() = Some(discord_rpc.clone());

        let bridge = IpcBridgeContext {
            web_tx: web_tx.clone(),
            player_tx,
            player_mpv: self.player.mpv.clone(),
            window_handle: self.player.window_handle,
            updater_tx,
            open_sender,
            webview_cmd_tx: self.webview.get_command_sender(),
            enhanced: self.enhanced.clone(),
            thumbnail_tx,
            active_stream_url,
            discord_rpc,
            toggle_fullscreen_sender: self.toggle_fullscreen_notice.sender(),
            set_interface_scale_sender: self.set_interface_scale_notice.sender(),
            quit_sender: self.quit_notice.sender(),
            hide_splash_sender: self.hide_splash_notice.sender(),
            focus_sender: self.focus_notice.sender(),
            cache_directory_sender: self.cache_directory_notice.sender(),
            webui_sender: self.webui_notice.sender(),
            server_restart_sender: self.server_restart_notice.sender(),
            server_stop_sender: self.server_stop_notice.sender(),
            toggle_pip_sender: self.toggle_pip_notice.sender(),
            pip_drag_sender: self.pip_drag_notice.sender(),
            pip_resize_sender: self.pip_resize_notice.sender(),
            export_settings_sender: self.export_settings_notice.sender(),
            import_settings_sender: self.import_settings_notice.sender(),
            autoupdater_setup_file: self.autoupdater_setup_file.clone(),
            requested_fullscreen: self.requested_fullscreen.clone(),
            requested_cache_directory: self.requested_cache_directory.clone(),
            requested_export_settings: self.requested_export_settings.clone(),
            requested_import_settings: self.requested_import_settings.clone(),
            local_server_url: self.local_server_url.clone(),
            requested_interface_scale: self.requested_interface_scale.clone(),
            webui_events: self.webui_events.clone(),
            requested_pip: self.requested_pip.clone(),
            requested_pip_resize: self.requested_pip_resize.clone(),
        };
        spawn_ipc_bridge(web_rx, bridge);

        if self.server.development() {
            self.load_webui(None);
        } else if is_streaming_server_online() {
            self.load_webui(Some(crate::stremio_app::constants::DEV_ENDPOINT));
        } else if crate::stremio_app::stremio_server::find_server_script().is_some()
            && crate::stremio_app::stremio_server::is_ffmpeg_installed()
        {
            self.server.start();
        } else {
            self.load_webui(None);
        }
    }
    fn load_webui(&self, server_url: Option<&str>) {
        *self.local_server_url.lock().unwrap() = server_url
            .filter(|server_url| {
                Url::parse(server_url).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
                })
            })
            .map(str::to_owned);
        if let Some(sender) = self.open_media_sender.borrow().as_ref() {
            sender.send(OpenRequest::Reset).ok();
        }
        let endpoint = if self.webui_url.trim_end_matches('/') == WEB_ENDPOINT.trim_end_matches('/')
        {
            server_url
                .map(web_endpoint_with_streaming_server)
                .unwrap_or_else(|| self.webui_url.clone())
        } else {
            self.webui_url.clone()
        };
        self.webui_ready.set(false);
        self.webui_retries.set(0);
        if let Err(error) = self.webview.navigate(endpoint) {
            self.splash_screen.hide();
            nwg::modal_error_message(
                &self.window,
                "Cannot load Stremio Web UI",
                &error.to_string(),
            );
        } else {
            self.start_webui_deadline();
        }
    }
    fn start_webui_deadline(&self) {
        let generation = self.webui_generation.get() + 1;
        self.webui_generation.set(generation);
        let timeout = if self.webui_retries.get() > 0 {
            WEBUI_RETRY_READY_TIMEOUT_SECS
        } else {
            WEBUI_READY_TIMEOUT_SECS
        };
        let events = self.webui_events.clone();
        let sender = self.webui_notice.sender();
        thread::spawn(move || {
            thread::sleep(time::Duration::from_secs(timeout));
            send_webui_event(&events, &sender, WebUiEvent::Timeout(generation));
        });
    }
    fn retry_webui_silently(&self) -> bool {
        let Some(&delay) = WEBUI_RETRY_DELAYS_SECS.get(self.webui_retries.get()) else {
            return false;
        };
        self.webui_retries.set(self.webui_retries.get() + 1);
        let generation = self.webui_generation.get() + 1;
        self.webui_generation.set(generation);
        let events = self.webui_events.clone();
        let sender = self.webui_notice.sender();
        thread::spawn(move || {
            thread::sleep(time::Duration::from_secs(delay));
            send_webui_event(&events, &sender, WebUiEvent::Retry(generation));
        });
        true
    }
    fn retry_webui(&self) {
        match self.webview.retry() {
            Ok(()) => self.start_webui_deadline(),
            Err(error) => self.show_webui_error(&error.to_string()),
        }
    }
    fn show_webui_error(&self, details: &str) {
        self.splash_screen.hide();
        self.on_show();
        let choice = nwg::modal_message(
            &self.window,
            &nwg::MessageParams {
                title: "Stremio",
                content: details,
                buttons: nwg::MessageButtons::RetryCancel,
                icons: nwg::MessageIcons::Error,
            },
        );
        // The web UI can become ready while the dialog is open.
        if self.webui_ready.get() {
            return;
        }
        if choice == nwg::MessageChoice::Retry {
            if !self.no_splash {
                self.splash_screen.show();
            }
            self.retry_webui();
        } else {
            self.on_exit();
        }
    }
    fn on_webui_notice(&self) {
        let events: Vec<_> = self.webui_events.lock().unwrap().drain(..).collect();
        for event in events {
            let current = self.webui_generation.get();
            match event {
                WebUiEvent::Ready => {
                    self.webui_ready.set(true);
                    self.webui_retries.set(0);
                }
                _ if self.webui_ready.get() => {}
                WebUiEvent::LoadFailed(detail) => {
                    eprintln!("Web UI load error: {detail}");
                    if !self.retry_webui_silently() {
                        self.show_webui_error(&format!("{WEBUI_UNREACHABLE}\n\n{detail}"));
                    }
                }
                WebUiEvent::Timeout(generation) if generation == current => {
                    eprintln!("Web UI startup timed out");
                    if self.webui_retries.get() > 0 || !self.retry_webui_silently() {
                        self.show_webui_error(WEBUI_UNREACHABLE);
                    }
                }
                WebUiEvent::Retry(generation) if generation == current => {
                    eprintln!("Retrying the web UI (attempt {})", self.webui_retries.get());
                    self.retry_webui();
                }
                WebUiEvent::Timeout(_) | WebUiEvent::Retry(_) => {}
            }
        }
    }
    fn on_server_notice(&self) {
        for event in self.server.events() {
            match event {
                ServerEvent::Ready(endpoint) => {
                    if self.webui_ready.get() {
                        *self.local_server_url.lock().unwrap() = Some(endpoint);
                    } else {
                        self.load_webui(Some(&endpoint));
                    }
                }
                ServerEvent::Failed(details) => {
                    if crate::stremio_app::stremio_server::is_streaming_server_online() {
                        self.load_webui(Some(crate::stremio_app::constants::DEV_ENDPOINT));
                    } else {
                        eprintln!("Streaming server unavailable: {details}");
                        *self.local_server_url.lock().unwrap() = None;
                        if let Some(sender) = self.open_media_sender.borrow().as_ref() {
                            sender.send(OpenRequest::Reset).ok();
                        }
                        // Instead of blocking with a modal error, fall back to Web UI so Enhanced UI
                        // can load and guide the user through downloading server.js or using Stremio Service.
                        self.load_webui(None);
                    }
                }
            }
        }
    }
    fn on_server_restart_notice(&self) {
        println!("[MainWindow] Restarting streaming server via notice...");
        self.server.restart();
    }
    fn on_server_stop_notice(&self) {
        println!("[MainWindow] Stopping streaming server via notice...");
        self.server.stop();
    }
    fn on_min_max(&self, data: &nwg::EventData) {
        let data = data.on_min_max();
        let is_pip = self.pip_mode_active.load(std::sync::atomic::Ordering::Relaxed)
            || self
                .saved_window_style
                .try_borrow()
                .map(|style| style.pip_mode)
                .unwrap_or(false);
        if is_pip {
            data.set_min_size(280, 158);
        } else {
            data.set_min_size(WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT);
        }
    }
    fn on_paint(&self) {
        if !self.splash_screen.visible() {
            self.webview.fit_to_window(self.window.handle.hwnd());
        }
    }
    fn on_window_state_changed(&self) {
        self.save_window_settings();
        self.transmit_window_state_change();
    }
    fn save_window_settings(&self) {
        if self
            .saved_window_style
            .try_borrow()
            .map(|style| style.full_screen || style.pip_mode)
            .unwrap_or(false)
        {
            return;
        }
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Err(err) = WindowSettings::save(hwnd) {
                eprintln!("Cannot save window settings: {err}");
            }
        }
    }
    fn on_toggle_fullscreen_notice(&self) {
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(mut saved_style) = self.saved_window_style.try_borrow_mut() {
                let target = self
                    .requested_fullscreen
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or(!saved_style.full_screen);
                if target && saved_style.pip_mode {
                    self.pip_mode_active.store(false, std::sync::atomic::Ordering::Relaxed);
                    saved_style.set_pip(hwnd, false);
                    self.webview.execute_script(
                        "window.__syncPipState && window.__syncPipState(false);".to_string(),
                    );
                }
                saved_style.set_full_screen(hwnd, target);
                self.tray.tray_topmost.set_enabled(!saved_style.full_screen);
                self.tray
                    .tray_topmost
                    .set_checked((saved_style.ex_style as u32 & WS_EX_TOPMOST) == WS_EX_TOPMOST);
            }
            self.webview.fit_to_window(Some(hwnd));
        }
        self.transmit_window_visibility_change();
    }
    fn on_toggle_pip_notice(&self) {
        let mut pip_active = false;
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(mut saved_style) = self.saved_window_style.try_borrow_mut() {
                let target = self
                    .requested_pip
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or(!saved_style.pip_mode);
                self.pip_mode_active.store(target, std::sync::atomic::Ordering::Relaxed);
                saved_style.set_pip(hwnd, target);
                pip_active = saved_style.pip_mode;
                self.pip_mode_active.store(pip_active, std::sync::atomic::Ordering::Relaxed);
                self.tray
                    .tray_topmost
                    .set_enabled(!saved_style.full_screen && !saved_style.pip_mode);
                self.tray.tray_topmost.set_checked(
                    (saved_style.ex_style as u32 & WS_EX_TOPMOST) == WS_EX_TOPMOST
                        || saved_style.pip_mode,
                );
            }
            self.webview.fit_to_window(Some(hwnd));
        }
        self.webview.execute_script(format!(
            "window.__syncPipState && window.__syncPipState({pip_active});"
        ));
        self.transmit_window_visibility_change();
        self.transmit_window_state_change();
    }
    fn on_pip_drag_notice(&self) {
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(saved_style) = self.saved_window_style.try_borrow() {
                saved_style.start_pip_drag(hwnd);
            }
        }
    }
    fn on_pip_resize_notice(&self) {
        let direction = self.requested_pip_resize.lock().unwrap().take();
        if let (Some(hwnd), Some(direction)) = (self.window.handle.hwnd(), direction) {
            if let Ok(saved_style) = self.saved_window_style.try_borrow() {
                saved_style.start_pip_resize(hwnd, direction);
            }
            self.webview.fit_to_window(Some(hwnd));
        }
    }
    fn on_set_interface_scale_notice(&self) {
        let scale = self.requested_interface_scale.lock().unwrap().take();
        if let Some(scale) = scale {
            self.webview.set_interface_scale(scale);
        }
    }
    fn on_hide_splash_notice(&self) {
        self.splash_screen.hide();
    }
    fn on_cache_directory_notice(&self) {
        let request = self.requested_cache_directory.lock().unwrap().clone();
        let Some(request) = request else { return };
        if let Some(directory) = &request.directory {
            // An unplugged drive must not prevent choosing a different folder.
            self.cache_directory_picker
                .set_default_folder(directory)
                .ok();
        }
        // Show runs a nested message loop; keep the pending request, but no locks or borrows.
        let result = if self.cache_directory_picker.run(Some(&self.window)) {
            self.cache_directory_picker
                .get_selected_item()
                .map_err(|error| error.to_string())
                .and_then(|path| {
                    path.into_string()
                        .map(Some)
                        .map_err(|_| "The selected folder path is not valid Unicode".to_owned())
                })
        } else {
            Ok(None)
        };
        self.requested_cache_directory.lock().unwrap().take();
        if let Some((web_tx, _)) = self.webview.channel.borrow().as_ref() {
            web_tx
                .send(RPCResponse::cache_directory_selected(
                    request.request_id,
                    result,
                ))
                .ok();
        }
    }
    fn on_export_settings_notice(&self) {
        let Some(request) = self.requested_export_settings.lock().unwrap().clone() else {
            return;
        };
        let default_dir = self
            .enhanced
            .lock()
            .unwrap()
            .base_dir
            .to_string_lossy()
            .to_string();
        self.export_settings_picker
            .set_default_folder(&default_dir)
            .ok();

        let payload = if self.export_settings_picker.run(Some(&self.window)) {
            match self
                .export_settings_picker
                .get_selected_item()
                .map_err(|e| e.to_string())
                .and_then(|os| {
                    os.into_string()
                        .map(PathBuf::from)
                        .map_err(|_| "The selected file path is not valid Unicode".to_string())
                }) {
                Ok(mut path) => {
                    if path.extension().is_none() {
                        path.set_extension("json");
                    }
                    let json_res = self
                        .enhanced
                        .lock()
                        .unwrap()
                        .build_export_json(request.plugin_settings.as_ref());
                    match json_res.and_then(|json| {
                        std::fs::write(&path, json).map_err(|e| e.to_string())
                    }) {
                        Ok(()) => serde_json::json!({
                            "ok": true,
                            "path": path.to_string_lossy().to_string(),
                        }),
                        Err(err) => serde_json::json!({
                            "ok": false,
                            "error": err,
                        }),
                    }
                }
                Err(err) => serde_json::json!({
                    "ok": false,
                    "error": err,
                }),
            }
        } else {
            serde_json::json!({
                "ok": false,
                "cancelled": true,
            })
        };

        self.requested_export_settings.lock().unwrap().take();
        if let (Ok(id_json), Ok(payload_json)) = (
            serde_json::to_string(&request.id),
            serde_json::to_string(&payload),
        ) {
            send_js_callback(
                &self.webview.get_command_sender(),
                "__receiveExportSettingsResult",
                &[id_json, payload_json],
            );
        }
    }
    fn on_import_settings_notice(&self) {
        let Some(request_id) = self.requested_import_settings.lock().unwrap().clone() else {
            return;
        };
        let default_dir = self
            .enhanced
            .lock()
            .unwrap()
            .base_dir
            .to_string_lossy()
            .to_string();
        self.import_settings_picker
            .set_default_folder(&default_dir)
            .ok();

        let payload = if self.import_settings_picker.run(Some(&self.window)) {
            match self
                .import_settings_picker
                .get_selected_item()
                .map_err(|e| e.to_string())
                .and_then(|os| {
                    os.into_string()
                        .map(PathBuf::from)
                        .map_err(|_| "The selected file path is not valid Unicode".to_string())
                })
                .and_then(|path| std::fs::read_to_string(&path).map_err(|e| e.to_string()))
            {
                Ok(raw_json) => {
                    let import_res = {
                        let mut mgr = self.enhanced.lock().unwrap();
                        mgr.import_settings_json(&raw_json).map(|plugin_settings| {
                            let css = mgr.active_theme().map(|t| t.content.clone());
                            let thumb_enabled = mgr.settings.thumbnail_preview;
                            let thumb_precache = mgr.settings.thumbnail_precache;
                            let thumb_size = mgr.settings.thumbnail_preview_size;
                            let discord_enabled = mgr.settings.discord_rpc;
                            let shader_paths = if mgr.settings.shader_preset != "off" {
                                shaders::resolve_preset_paths(
                                    &mgr.shaders_dir,
                                    &mgr.settings.shader_preset,
                                )
                            } else {
                                Vec::new()
                            };
                            let cmd_tx = self.webview.get_command_sender();
                            let theme_script = theme_injection_script(css.as_deref());
                            cmd_tx
                                .send(WebViewCommand::ExecuteScript(theme_script))
                                .ok();
                            send_enhanced_ui_data(&mgr, &cmd_tx);
                            (
                                plugin_settings,
                                thumb_enabled,
                                thumb_precache,
                                thumb_size,
                                discord_enabled,
                                shader_paths,
                            )
                        })
                    };

                    match import_res {
                        Ok((
                            plugin_settings,
                            thumb_enabled,
                            thumb_precache,
                            thumb_size,
                            discord_enabled,
                            shader_paths,
                        )) => {
                            if let Some(ref mpv) = self.player.mpv {
                                set_glsl_shaders(mpv, &shader_paths);
                            }
                            if let Some(tx) = self.thumbnail_tx.borrow().as_ref() {
                                let _ = tx.send(ThumbnailCmd::SetPreviewSize(thumb_size));
                                let _ = tx.send(ThumbnailCmd::SetPrecache(thumb_precache));
                                let _ = tx.send(ThumbnailCmd::SetEnabled(thumb_enabled));
                                if thumb_enabled {
                                    if let Some(url) = self.active_stream_url.lock().unwrap().clone()
                                    {
                                        let _ = tx.send(ThumbnailCmd::SetSource(Some(url)));
                                    }
                                }
                            }
                            if let Some(rpc) = self.discord_rpc.borrow().as_ref() {
                                if !discord_enabled {
                                    let _ = rpc.clear_activity();
                                    let _ = rpc.disconnect();
                                    if let Ok(web_channel) = self.webview.channel.try_borrow() {
                                        if let Some((web_tx, _)) = web_channel.as_ref() {
                                            web_tx.send(RPCResponse::discord_status(false)).ok();
                                        }
                                    }
                                } else {
                                    let _ = rpc.connect();
                                }
                            }
                            serde_json::json!({
                                "ok": true,
                                "pluginSettings": plugin_settings,
                            })
                        }
                        Err(err) => serde_json::json!({
                            "ok": false,
                            "error": err,
                        }),
                    }
                }
                Err(err) => serde_json::json!({
                    "ok": false,
                    "error": err,
                }),
            }
        } else {
            serde_json::json!({
                "ok": false,
                "cancelled": true,
            })
        };

        self.requested_import_settings.lock().unwrap().take();
        if let (Ok(id_json), Ok(payload_json)) = (
            serde_json::to_string(&request_id),
            serde_json::to_string(&payload),
        ) {
            send_js_callback(
                &self.webview.get_command_sender(),
                "__receiveImportSettingsResult",
                &[id_json, payload_json],
            );
        }
    }
    fn on_focus_notice(&self) {
        self.window.set_visible(true);
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(mut saved_style) = self.saved_window_style.try_borrow_mut() {
                saved_style.set_active(hwnd);
            }
        }
    }
    fn on_toggle_topmost(&self) {
        if let Some(hwnd) = self.window.handle.hwnd() {
            if let Ok(mut saved_style) = self.saved_window_style.try_borrow_mut() {
                saved_style.toggle_topmost(hwnd);
                self.tray
                    .tray_topmost
                    .set_checked((saved_style.ex_style as u32 & WS_EX_TOPMOST) == WS_EX_TOPMOST);
            }
        }
    }
    fn on_show(&self) {
        self.window.set_visible(true);
        if let (Some(hwnd), Ok(mut saved_style)) = (
            self.window.handle.hwnd(),
            self.saved_window_style.try_borrow_mut(),
        ) {
            if saved_style.is_window_minimized(hwnd) {
                self.window.restore();
            }
            saved_style.set_active(hwnd);
        }
        self.tray.tray_show_hide.set_checked(self.window.visible());
        self.transmit_window_state_change();
        self.transmit_window_visibility_change();
    }
    fn on_show_hide(&self) {
        if self.window.visible() {
            self.window.set_visible(false);
            self.tray.tray_show_hide.set_checked(self.window.visible());
            self.transmit_window_state_change();
            self.transmit_window_visibility_change();
        } else {
            self.on_show();
        }
    }
    fn on_quit(&self, data: &nwg::EventData) {
        if let nwg::EventData::OnWindowClose(data) = data {
            data.close(false);
        }
        self.save_window_settings();
        self.window.set_visible(false);
        self.tray.tray_show_hide.set_checked(self.window.visible());
        self.transmit_window_visibility_change();
    }
    fn on_exit(&self) {
        self.save_window_settings();
        nwg::stop_thread_dispatch();
    }
}

struct IpcBridgeContext {
    web_tx: flume::Sender<String>,
    player_tx: flume::Sender<String>,
    player_mpv: Option<Arc<libmpv2::Mpv>>,
    window_handle: isize,
    updater_tx: flume::Sender<String>,
    open_sender: flume::Sender<OpenRequest>,
    webview_cmd_tx: flume::Sender<WebViewCommand>,
    enhanced: Arc<Mutex<EnhancedManager>>,
    thumbnail_tx: flume::Sender<ThumbnailCmd>,
    active_stream_url: Arc<Mutex<Option<String>>>,
    discord_rpc: DiscordRpc,
    toggle_fullscreen_sender: nwg::NoticeSender,
    set_interface_scale_sender: nwg::NoticeSender,
    quit_sender: nwg::NoticeSender,
    hide_splash_sender: nwg::NoticeSender,
    focus_sender: nwg::NoticeSender,
    cache_directory_sender: nwg::NoticeSender,
    webui_sender: nwg::NoticeSender,
    server_restart_sender: nwg::NoticeSender,
    server_stop_sender: nwg::NoticeSender,
    toggle_pip_sender: nwg::NoticeSender,
    pip_drag_sender: nwg::NoticeSender,
    pip_resize_sender: nwg::NoticeSender,
    export_settings_sender: nwg::NoticeSender,
    import_settings_sender: nwg::NoticeSender,
    autoupdater_setup_file: Arc<Mutex<Option<PathBuf>>>,
    requested_fullscreen: Arc<Mutex<Option<bool>>>,
    requested_cache_directory: Arc<Mutex<Option<CacheDirectoryRequest>>>,
    requested_export_settings: Arc<Mutex<Option<ExportSettingsRequest>>>,
    requested_import_settings: Arc<Mutex<Option<String>>>,
    local_server_url: Arc<Mutex<Option<String>>>,
    requested_interface_scale: Arc<Mutex<Option<u64>>>,
    webui_events: WebUiEvents,
    requested_pip: Arc<Mutex<Option<bool>>>,
    requested_pip_resize: Arc<Mutex<Option<u32>>>,
}

fn spawn_open_media_worker(
    initial_command: String,
    open_receiver: flume::Receiver<OpenRequest>,
    web_tx: flume::Sender<String>,
) {
    thread::spawn(move || {
        let mut ready = false;
        let mut pending = (!initial_command.is_empty()).then_some(initial_command);
        for request in open_receiver {
            match request {
                OpenRequest::Input(input) => pending = Some(input),
                OpenRequest::Ready => ready = true,
                OpenRequest::Reset => ready = false,
            }
            if ready {
                if let Some(input) = pending.take() {
                    let message = super::open_media::message(&input);
                    web_tx.send(RPCResponse::response_message(Some(message))).ok();
                }
            }
        }
    });
}

fn spawn_updater_worker(
    autoupdater_endpoint: Option<Url>,
    force_update: bool,
    autoupdater_setup_file: Arc<Mutex<Option<PathBuf>>>,
    updater_rx: flume::Receiver<String>,
    web_tx: flume::Sender<String>,
) {
    let Some(endpoint) = autoupdater_endpoint else {
        return;
    };
    thread::spawn(move || {
        for msg in updater_rx.iter() {
            if msg == "check_for_update" {
                break;
            }
        }

        let current_version: semver::Version = env!("CARGO_PKG_VERSION")
            .parse()
            .expect("Should always be valid");

        loop {
            let updater = updater::Updater::new(current_version.clone(), &endpoint, force_update);
            match updater.autoupdate() {
                Ok(Some(update)) => {
                    println!("New version ready to install v{}", update.version);
                    *autoupdater_setup_file.lock().unwrap() = Some(update.file);
                    web_tx.send(RPCResponse::update_available()).ok();
                }
                Ok(None) => println!("No new updates found"),
                Err(e) => eprintln!("Failed to fetch updates: {e}"),
            }

            thread::sleep(time::Duration::from_secs(UPDATE_INTERVAL));
        }
    });
}

fn spawn_single_instance_listener(
    mut listener: PipeServer,
    focus_sender: nwg::NoticeSender,
    open_sender: flume::Sender<OpenRequest>,
) {
    thread::spawn(move || loop {
        if let Ok(mut stream) = listener.accept() {
            let mut buf = vec![];
            stream.read_to_end(&mut buf).ok();
            if let Ok(s) = str::from_utf8(&buf) {
                focus_sender.notice();
                if !s.is_empty() {
                    open_sender.send(OpenRequest::Input(s.to_string())).ok();
                }
            }
        }
    });
}

fn spawn_ipc_bridge(web_rx: flume::Receiver<String>, ctx: IpcBridgeContext) {
    thread::spawn(move || {
        while let Ok(raw) = web_rx.recv() {
            if let Ok(action) = serde_json::from_str::<EnhancedAction>(&raw) {
                handle_enhanced_action(action, &ctx);
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<RPCRequest>(&raw) {
                handle_rpc_request(msg, &ctx);
            }
        }
    });
}

fn send_enhanced_ui_data(mgr: &EnhancedManager, webview_cmd_tx: &flume::Sender<WebViewCommand>) {
    let running = is_streaming_server_online();
    let data = mgr.ui_data(running);
    if let Ok(json) = serde_json::to_string(&data) {
        let script = format!("window.__receiveEnhancedData && window.__receiveEnhancedData({json});");
        webview_cmd_tx
            .send(WebViewCommand::ExecuteScript(script))
            .ok();
    }
}

fn send_js_callback(
    webview_cmd_tx: &flume::Sender<WebViewCommand>,
    fn_name: &str,
    args_json: &[String],
) {
    let args = args_json.join(", ");
    let script = format!("window.{fn_name} && window.{fn_name}({args});");
    webview_cmd_tx
        .send(WebViewCommand::ExecuteScript(script))
        .ok();
}

fn handle_enhanced_action(action: EnhancedAction, ctx: &IpcBridgeContext) {
    match action {
        EnhancedAction::GetData => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.reload();
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::OpenThemesFolder => {
            ctx.enhanced.lock().unwrap().open_themes_folder();
        }
        EnhancedAction::OpenPluginsFolder => {
            ctx.enhanced.lock().unwrap().open_plugins_folder();
        }
        EnhancedAction::OpenStreamingServerFolder => {
            ctx.enhanced.lock().unwrap().open_streaming_server_folder();
        }
        EnhancedAction::GetServerStatus { id } => {
            let running = is_streaming_server_online();
            let status = ctx.enhanced.lock().unwrap().get_server_status(running);
            if let (Ok(id_json), Ok(status_json)) =
                (serde_json::to_string(&id), serde_json::to_string(&status))
            {
                send_js_callback(
                    &ctx.webview_cmd_tx,
                    "__receiveServerStatus",
                    &[id_json, status_json],
                );
            }
        }
        EnhancedAction::StartStreamingServer { id } => {
            ctx.server_restart_sender.notice();
            let tx = ctx.webview_cmd_tx.clone();
            thread::spawn(move || {
                let mut running = false;
                for _ in 0..40 {
                    thread::sleep(time::Duration::from_millis(250));
                    if is_streaming_server_online() {
                        running = true;
                        break;
                    }
                }
                if let Ok(id_json) = serde_json::to_string(&id) {
                    send_js_callback(
                        &tx,
                        "__receiveServerStartResult",
                        &[id_json, running.to_string()],
                    );
                }
            });
        }
        EnhancedAction::LoadUrl { url } => {
            if is_trusted_shell_navigation_url(&url) {
                ctx.webview_cmd_tx.send(WebViewCommand::Navigate(url)).ok();
            } else {
                eprintln!("[Enhanced] Blocked navigation to untrusted URL: {url}");
            }
        }
        EnhancedAction::PrepareServerUpdate { version } => {
            ctx.server_stop_sender.notice();
            if let Err(e) = ctx.enhanced.lock().unwrap().prepare_server_update(&version) {
                eprintln!("Failed to prepare server update: {e}");
            }
        }
        EnhancedAction::SkipServerVersion { version } => {
            if let Err(e) = ctx.enhanced.lock().unwrap().skip_server_version(&version) {
                eprintln!("Failed to skip server version: {e}");
            }
        }
        EnhancedAction::SkipClientVersion { version } => {
            if let Err(e) = ctx.enhanced.lock().unwrap().skip_client_version(&version) {
                eprintln!("Failed to skip client version: {e}");
            }
        }
        EnhancedAction::TogglePlugin { file_name, enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.toggle_plugin(&file_name, enabled);
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::ApplyTheme { file_name } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            let new_theme = (file_name != "Default").then_some(file_name.as_str());
            let css = mgr.set_active_theme(new_theme);
            let theme_script = theme_injection_script(css.as_deref());
            ctx.webview_cmd_tx
                .send(WebViewCommand::ExecuteScript(theme_script))
                .ok();
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::Log { level, message } => match level.as_str() {
            "error" => eprintln!("[Enhanced ERROR] {message}"),
            "warn" => eprintln!("[Enhanced WARN] {message}"),
            _ => println!("[Enhanced INFO] {message}"),
        },
        EnhancedAction::HttpGet { id, url } => {
            let tx = ctx.webview_cmd_tx.clone();
            thread::spawn(move || {
                let Ok(id_json) = serde_json::to_string(&id) else {
                    return;
                };
                match crate::stremio_app::enhanced::http_get(&url) {
                    Ok((status, body)) => {
                        let body_json =
                            serde_json::to_string(&body).unwrap_or_else(|_| "\"\"".to_string());
                        send_js_callback(
                            &tx,
                            "__receiveHttpResponse",
                            &[id_json, status.to_string(), body_json],
                        );
                    }
                    Err(err) => {
                        eprintln!("HttpGet failed for {url}: {err}");
                        let err_json =
                            serde_json::to_string(&err).unwrap_or_else(|_| "\"Error\"".to_string());
                        send_js_callback(
                            &tx,
                            "__receiveHttpResponse",
                            &[id_json, "500".to_string(), err_json],
                        );
                    }
                }
            });
        }
        EnhancedAction::InstallMod {
            mod_type,
            file_name,
            content,
        } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            if let Err(e) = mgr.install_mod(&mod_type, &file_name, &content) {
                eprintln!("Failed to install mod {file_name}: {e}");
            }
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::DeleteMod {
            mod_type,
            file_name,
        } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            if let Err(e) = mgr.delete_mod(&mod_type, &file_name) {
                eprintln!("Failed to delete mod {file_name}: {e}");
            }
            let css = mgr.active_theme().map(|t| t.content.as_str());
            let theme_script = theme_injection_script(css);
            ctx.webview_cmd_tx
                .send(WebViewCommand::ExecuteScript(theme_script))
                .ok();
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::DownloadFfmpeg { id } => {
            let tx = ctx.webview_cmd_tx.clone();
            thread::spawn(move || {
                let success = ensure_ffmpeg_available().is_ok();
                if let Ok(id_json) = serde_json::to_string(&id) {
                    send_js_callback(&tx, "__receiveFfmpegResult", &[id_json, success.to_string()]);
                }
            });
        }
        EnhancedAction::SetPictureInPicture { enabled } => {
            *ctx.requested_pip.lock().unwrap() = Some(enabled);
            ctx.toggle_pip_sender.notice();
        }
        EnhancedAction::StartPipDrag => {
            ctx.pip_drag_sender.notice();
        }
        EnhancedAction::StartPipResize { direction } => {
            *ctx.requested_pip_resize.lock().unwrap() = Some(direction);
            ctx.pip_resize_sender.notice();
        }
        EnhancedAction::RequestThumbnail { time, max_width } => {
            let _ = ctx.thumbnail_tx.send(ThumbnailCmd::Request {
                time_sec: time,
                max_width: max_width.unwrap_or(DEFAULT_THUMB_MAX_WIDTH),
            });
        }
        EnhancedAction::SetThumbnailPreview { enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.set_thumbnail_preview(enabled);
            let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetEnabled(enabled));
            if enabled {
                if let Some(url) = ctx.active_stream_url.lock().unwrap().clone() {
                    let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetSource(Some(url)));
                }
            }
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::SetThumbnailPreviewSize { size } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.set_thumbnail_preview_size(size);
            let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetPreviewSize(size));
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::SetThumbnailPrecache { enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.set_thumbnail_precache(enabled);
            let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetPrecache(enabled));
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::SetDiscordRpc { enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.set_discord_rpc(enabled);
            if !enabled {
                if let Err(e) = ctx.discord_rpc.clear_activity() {
                    eprintln!("Discord clear activity error: {e}");
                }
                if let Err(e) = ctx.discord_rpc.disconnect() {
                    eprintln!("Discord disconnect error: {e}");
                }
                ctx.web_tx.send(RPCResponse::discord_status(false)).ok();
            } else if let Err(e) = ctx.discord_rpc.connect() {
                eprintln!("Discord connect error: {e}");
            }
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
        EnhancedAction::ExportSettings {
            id,
            plugin_settings,
        } => {
            let mut pending = ctx.requested_export_settings.lock().unwrap();
            if pending.is_some() {
                drop(pending);
                if let (Ok(id_json), Ok(payload_json)) = (
                    serde_json::to_string(&id),
                    serde_json::to_string(&serde_json::json!({
                        "ok": false,
                        "error": "An export dialog is already open",
                    })),
                ) {
                    send_js_callback(
                        &ctx.webview_cmd_tx,
                        "__receiveExportSettingsResult",
                        &[id_json, payload_json],
                    );
                }
            } else {
                *pending = Some(ExportSettingsRequest {
                    id,
                    plugin_settings,
                });
                ctx.export_settings_sender.notice();
            }
        }
        EnhancedAction::ImportSettings { id } => {
            let mut pending = ctx.requested_import_settings.lock().unwrap();
            if pending.is_some() {
                drop(pending);
                if let (Ok(id_json), Ok(payload_json)) = (
                    serde_json::to_string(&id),
                    serde_json::to_string(&serde_json::json!({
                        "ok": false,
                        "error": "An import dialog is already open",
                    })),
                ) {
                    send_js_callback(
                        &ctx.webview_cmd_tx,
                        "__receiveImportSettingsResult",
                        &[id_json, payload_json],
                    );
                }
            } else {
                *pending = Some(id);
                ctx.import_settings_sender.notice();
            }
        }
        EnhancedAction::OpenShadersFolder => {
            ctx.enhanced.lock().unwrap().open_shaders_folder();
        }
        EnhancedAction::SetShaderPreset { id, preset } => {
            let shaders_dir = ctx.enhanced.lock().unwrap().shaders_dir.clone();
            let preset_def = shaders::find_preset(&preset);
            let is_off_or_installed = preset == "off"
                || preset_def.is_some_and(|def| shaders::is_preset_installed(&shaders_dir, def));

            if is_off_or_installed {
                let paths = shaders::resolve_preset_paths(&shaders_dir, &preset);
                if let Some(ref mpv) = ctx.player_mpv {
                    set_glsl_shaders(mpv, &paths);
                }
                let mut mgr = ctx.enhanced.lock().unwrap();
                mgr.set_shader_preset(&preset);

                let id_json = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".to_string());
                let preset_json =
                    serde_json::to_string(&preset).unwrap_or_else(|_| "\"off\"".to_string());
                send_js_callback(
                    &ctx.webview_cmd_tx,
                    "__receiveShaderPresetResult",
                    &[id_json, "true".to_string(), preset_json, "null".to_string()],
                );
                send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
            } else {
                let tx = ctx.webview_cmd_tx.clone();
                let mpv = ctx.player_mpv.clone();
                let enhanced = ctx.enhanced.clone();
                thread::spawn(move || {
                    let result = shaders::ensure_preset_downloaded(&shaders_dir, &preset);
                    let id_json = serde_json::to_string(&id).unwrap_or_else(|_| "\"\"".to_string());
                    let preset_json =
                        serde_json::to_string(&preset).unwrap_or_else(|_| "\"off\"".to_string());

                    match result {
                        Ok(paths) => {
                            if let Some(ref mpv) = mpv {
                                set_glsl_shaders(mpv, &paths);
                            }
                            let mut mgr = enhanced.lock().unwrap();
                            mgr.set_shader_preset(&preset);
                            send_js_callback(
                                &tx,
                                "__receiveShaderPresetResult",
                                &[id_json, "true".to_string(), preset_json, "null".to_string()],
                            );
                            send_enhanced_ui_data(&mgr, &tx);
                        }
                        Err(err) => {
                            eprintln!("[Shaders] Failed to apply preset {preset}: {err}");
                            let err_json = serde_json::to_string(&err)
                                .unwrap_or_else(|_| "\"Error\"".to_string());
                            send_js_callback(
                                &tx,
                                "__receiveShaderPresetResult",
                                &[id_json, "false".to_string(), preset_json, err_json],
                            );
                        }
                    }
                });
            }
        }
        EnhancedAction::GetPlaybackStats { id } => {
            if let Some(ref mpv) = ctx.player_mpv {
                let stats = get_playback_stats(mpv, ctx.window_handle);
                if let (Ok(id_json), Ok(stats_json)) =
                    (serde_json::to_string(&id), serde_json::to_string(&stats))
                {
                    send_js_callback(
                        &ctx.webview_cmd_tx,
                        "__receivePlaybackStats",
                        &[id_json, stats_json],
                    );
                }
            }
        }
        EnhancedAction::GetChapters => {
            let chapters = ctx
                .player_mpv
                .as_ref()
                .map(|mpv| get_chapters(mpv))
                .unwrap_or_default();
            if let Ok(json) = serde_json::to_string(&chapters) {
                send_js_callback(&ctx.webview_cmd_tx, "__receiveChapters", &[json]);
            }
        }
        EnhancedAction::SetChapterMarkers { enabled } => {
            let mut mgr = ctx.enhanced.lock().unwrap();
            mgr.set_chapter_markers(enabled);
            send_enhanced_ui_data(&mgr, &ctx.webview_cmd_tx);
        }
    }
}

fn handle_rpc_request(msg: RPCRequest, ctx: &IpcBridgeContext) {
    let local_server_url = ctx.local_server_url.lock().unwrap().clone();
    match msg.get_method() {
        // The handshake. Here we send some useful data to the WEB UI
        None if msg.is_handshake() => {
            ctx.web_tx
                .send(RPCResponse::get_handshake(local_server_url.as_deref()))
                .ok();
        }
        Some("pick-cache-directory") => {
            if let Some(request) = msg
                .get_params()
                .and_then(|params| serde_json::from_value::<CacheDirectoryRequest>(params.clone()).ok())
            {
                let mut pending = ctx.requested_cache_directory.lock().unwrap();
                let error = if local_server_url.as_deref() != Some(request.server_url.as_str()) {
                    Some("The folder picker is only available for the shell's local server")
                } else if pending.is_some() {
                    Some("A folder picker is already open")
                } else {
                    None
                };
                if let Some(error) = error {
                    ctx.web_tx
                        .send(RPCResponse::cache_directory_selected(
                            request.request_id,
                            Err(error.to_owned()),
                        ))
                        .ok();
                } else {
                    *pending = Some(request);
                    ctx.cache_directory_sender.notice();
                }
            }
        }
        Some("win-set-interface-scale") => {
            if let Some(scale) = msg
                .get_params()
                .and_then(|params| params.get("scale"))
                .and_then(|value| value.as_u64())
                .filter(|scale| (75..=175).contains(scale))
            {
                *ctx.requested_interface_scale.lock().unwrap() = Some(scale);
                ctx.set_interface_scale_sender.notice();
            }
        }
        Some("win-set-visibility") => {
            if let Some(fullscreen) = msg
                .get_params()
                .and_then(|params| params.get("fullscreen"))
                .and_then(|value| value.as_bool())
            {
                *ctx.requested_fullscreen.lock().unwrap() = Some(fullscreen);
                ctx.toggle_fullscreen_sender.notice();
            }
        }
        Some("quit") => ctx.quit_sender.notice(),
        Some("app-ready") => {
            send_webui_event(&ctx.webui_events, &ctx.webui_sender, WebUiEvent::Ready);
            ctx.hide_splash_sender.notice();
            ctx.web_tx
                .send(RPCResponse::visibility_change(true, 1, false))
                .ok();
            ctx.updater_tx.send("check_for_update".to_owned()).ok();
            ctx.open_sender.send(OpenRequest::Ready).ok();
        }
        Some("app-load-error") => {
            if let Some(arg) = msg.get_params() {
                let detail = arg.as_str().unwrap_or_default().to_owned();
                send_webui_event(
                    &ctx.webui_events,
                    &ctx.webui_sender,
                    WebUiEvent::LoadFailed(detail),
                );
            }
        }
        Some("app-error") => {
            ctx.hide_splash_sender.notice();
            if let Some(arg) = msg.get_params() {
                eprintln!("Web App Error: {arg}");
            }
        }
        Some("open-external") => {
            if let Some(arg) = msg.get_params() {
                let arg = arg.as_str().unwrap_or("");
                let arg_lc = arg.to_lowercase();
                if arg_lc.starts_with("http://")
                    || arg_lc.starts_with("https://")
                    || arg_lc.starts_with("rtp://")
                    || arg_lc.starts_with("rtps://")
                    || arg_lc.starts_with("ftp://")
                    || arg_lc.starts_with("ipfs://")
                {
                    if let Some(url) = safe_url(arg) {
                        open::that(url).ok();
                    }
                }
            }
        }
        Some("play-external") => {
            if let Some(arg) = msg.get_params().and_then(|value| value.as_str()) {
                if let Err(error) = crate::stremio_app::external_player::play(arg) {
                    eprintln!("External player request failed: {error}");
                }
            }
        }
        Some("win-focus") => {
            ctx.focus_sender.notice();
        }
        Some("autoupdater-notif-clicked") => {
            let setup_file = ctx.autoupdater_setup_file.lock().unwrap().clone();
            if let Some(file_path) = setup_file {
                if file_path.is_file() {
                    println!("Opening installer at {file_path:?}");
                    match open::that(&file_path) {
                        Ok(()) => ctx.quit_sender.notice(),
                        Err(err) => eprintln!("Updater couldn't be started: {err}"),
                    }
                } else {
                    eprintln!("Update installer file does not exist: {file_path:?}");
                }
            } else {
                println!("Cannot obtain the setup file path");
            }
        }
        Some("discord-connect") => {
            if !ctx.enhanced.lock().unwrap().settings.discord_rpc {
                ctx.web_tx.send(RPCResponse::discord_status(false)).ok();
                return;
            }
            if let Err(e) = ctx.discord_rpc.connect() {
                eprintln!("Discord connect error: {e}");
                ctx.web_tx.send(RPCResponse::discord_status(false)).ok();
            }
        }
        Some("discord-disconnect") => {
            if let Err(e) = ctx.discord_rpc.disconnect() {
                eprintln!("Discord disconnect error: {e}");
            }
            ctx.web_tx.send(RPCResponse::discord_status(false)).ok();
        }
        Some("discord-set-activity") => {
            if !ctx.enhanced.lock().unwrap().settings.discord_rpc {
                return;
            }
            if let Some(params) = msg.get_params() {
                let state = params.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let details = params.get("details").and_then(|v| v.as_str()).unwrap_or("");
                let image = params.get("image").and_then(|v| v.as_str());
                let start_timestamp = params.get("startTimestamp").and_then(|v| v.as_i64());
                let end_timestamp = params.get("endTimestamp").and_then(|v| v.as_i64());

                if let Err(e) = ctx.discord_rpc.set_activity(
                    state,
                    details,
                    image,
                    start_timestamp,
                    end_timestamp,
                ) {
                    eprintln!("Discord set activity error: {e}");
                }
            }
        }
        Some("discord-clear-activity") => {
            if let Err(e) = ctx.discord_rpc.clear_activity() {
                eprintln!("Discord clear activity error: {e}");
            }
        }
        Some(player_command) if player_command.starts_with("mpv-") => {
            if let Some(args) = msg.args {
                if let Ok(resp_json) = serde_json::to_string(&args) {
                    if let Ok(InMsg(InMsgFn::MpvCommand, InMsgArgs::Cmd(cmd))) =
                        serde_json::from_str::<InMsg>(&resp_json)
                    {
                        match &cmd {
                            CmdVal::Loadfile(url) | CmdVal::LoadfileAt(url, _) => {
                                *ctx.active_stream_url.lock().unwrap() = Some(url.clone());
                                let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetSource(None));
                            }
                            CmdVal::Stop => {
                                *ctx.active_stream_url.lock().unwrap() = None;
                                let _ = ctx.thumbnail_tx.send(ThumbnailCmd::SetSource(None));
                            }
                            _ => {}
                        }
                    }
                    ctx.player_tx.send(resp_json).ok();
                }
            }
        }
        Some(unknown) => {
            eprintln!("Unsupported command {}({:?})", unknown, msg.get_params())
        }
        None => {}
    }
}

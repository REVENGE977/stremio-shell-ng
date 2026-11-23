use crate::stremio_app::stremio_wevbiew::WebViewCommand;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use flume::{Receiver, Sender};
use jpeg_encoder::{ColorType, Encoder};
use libmpv2::Mpv;
use std::{
    collections::{HashMap, VecDeque},
    ffi::{c_void, CString},
    os::raw::c_int,
    ptr,
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const DEFAULT_THUMB_MAX_WIDTH: u32 = 240;
const MIN_THUMB_MAX_WIDTH: u32 = 120;
const MAX_THUMB_MAX_WIDTH: u32 = 400;
const JPEG_QUALITY: u8 = 80;
const MAX_CACHE_ENTRIES: usize = 240;
const SEEK_TIMEOUT: Duration = Duration::from_secs(4);
const STREAM_LOAD_TIMEOUT: Duration = Duration::from_secs(12);
const PREEMPT_MIN_ELAPSED: Duration = Duration::from_millis(120);
const PRECACHE_START_DELAY: Duration = Duration::from_secs(2);
const PRECACHE_HOVER_COOLDOWN: Duration = Duration::from_millis(1500);
const PRECACHE_STEP_COOLDOWN: Duration = Duration::from_millis(60);
const PRECACHE_MAX_SLOTS: usize = 60;
const PRECACHE_MAX_CONSECUTIVE_FAILURES: u32 = 3;

pub enum ThumbnailCmd {
    SetEnabled(bool),
    SetPrecache(bool),
    SetPreviewSize(u32),
    SetSource(Option<String>),
    Request { time_sec: f64, max_width: u32 },
}

enum WorkerWake {
    Cmd,
    Mpv,
    Render,
}

#[derive(Clone)]
struct CachedThumb {
    data_url: String,
    width: u32,
    height: u32,
}

struct InFlightSeek {
    requested_time: f64,
    clamped_time: f64,
    quantized_key: i64,
    max_width: u32,
    started_at: Instant,
    seek_settled: bool,
    render_ready: bool,
    is_precache: bool,
}

struct SwRenderContext {
    ctx: *mut libmpv2_sys::mpv_render_context,
    _mpv: Arc<Mpv>,
    _wake_box: *mut Sender<WorkerWake>,
}

unsafe impl Send for SwRenderContext {}

impl SwRenderContext {
    fn new(mpv: Arc<Mpv>, wake_sender: Sender<WorkerWake>) -> Result<Self, String> {
        let api_type = libmpv2_sys::MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void;
        let mut params = [
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api_type,
            },
            libmpv2_sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];

        let mut raw_ctx: *mut libmpv2_sys::mpv_render_context = ptr::null_mut();
        let err = unsafe {
            libmpv2_sys::mpv_render_context_create(
                &mut raw_ctx,
                mpv.ctx.as_ptr(),
                params.as_mut_ptr(),
            )
        };
        if err < 0 || raw_ctx.is_null() {
            return Err(format!("mpv_render_context_create(sw) failed: {err}"));
        }

        let wake_box = Box::into_raw(Box::new(wake_sender));
        unsafe {
            libmpv2_sys::mpv_render_context_set_update_callback(
                raw_ctx,
                Some(render_update_callback),
                wake_box as *mut c_void,
            );
        }

        Ok(Self {
            ctx: raw_ctx,
            _mpv: mpv,
            _wake_box: wake_box,
        })
    }

    fn update_flags(&self) -> u64 {
        unsafe { libmpv2_sys::mpv_render_context_update(self.ctx) }
    }

    fn render_bgr0(&self, width: u32, height: u32) -> Result<(Vec<u8>, usize), String> {
        let w = width as usize;
        let h = height as usize;
        // 64-byte aligned stride as recommended by libmpv for SIMD swscale fast paths.
        let stride = (w * 4 + 63) & !63;
        let mut buf = vec![0u8; stride * h];

        let mut size: [c_int; 2] = [width as c_int, height as c_int];
        let fmt = CString::new("bgr0").expect("valid format cstr");
        let mut stride_val: usize = stride;
        let mut block_target: c_int = 0;

        let mut params = [
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr() as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: fmt.as_ptr() as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: (&mut stride_val as *mut usize) as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: buf.as_mut_ptr() as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: libmpv2_sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                data: (&mut block_target as *mut c_int) as *mut c_void,
            },
            libmpv2_sys::mpv_render_param {
                type_: 0,
                data: ptr::null_mut(),
            },
        ];

        let err = unsafe { libmpv2_sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        if err < 0 {
            return Err(format!("mpv_render_context_render(sw) failed: {err}"));
        }

        Ok((buf, stride))
    }
}

impl Drop for SwRenderContext {
    fn drop(&mut self) {
        unsafe {
            libmpv2_sys::mpv_render_context_set_update_callback(self.ctx, None, ptr::null_mut());
            libmpv2_sys::mpv_render_context_free(self.ctx);
            drop(Box::from_raw(self._wake_box));
        }
    }
}

unsafe extern "C" fn render_update_callback(cb_ctx: *mut c_void) {
    if cb_ctx.is_null() {
        return;
    }
    let sender = unsafe { &*(cb_ctx as *const Sender<WorkerWake>) };
    let _ = sender.try_send(WorkerWake::Render);
}

struct ThumbnailEngine {
    mpv: Arc<Mpv>,
    render: SwRenderContext,
}

impl ThumbnailEngine {
    fn create(wake_sender: Sender<WorkerWake>) -> Result<Self, String> {
        let mpv = Mpv::with_initializer(|initializer| {
            let options = [
                ("vo", "libmpv"),
                ("ao", "null"),
                ("aid", "no"),
                ("sid", "no"),
                ("sub-auto", "no"),
                ("osc", "no"),
                ("osd-level", "0"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("input-terminal", "no"),
                ("terminal", "no"),
                ("quiet", "yes"),
                ("msg-level", "all=no"),
                ("pause", "yes"),
                ("keep-open", "yes"),
                ("hwdec", "auto-copy"),
                ("hr-seek", "no"),
                ("hr-seek-framedrop", "yes"),
                ("vd-lavc-fast", "yes"),
                ("vd-lavc-skiploopfilter", "all"),
                ("vd-lavc-skipframe", "nonkey"),
                ("vd-lavc-threads", "4"),
                ("audio-display", "no"),
                ("video-timing-offset", "0"),
                ("sws-scaler", "fast-bilinear"),
                ("demuxer-readahead-secs", "0"),
                ("cache", "yes"),
                ("demuxer-max-bytes", "2MiB"),
                ("demuxer-max-back-bytes", "0"),
                ("stream-buffer-size", "64KiB"),
                ("demuxer-lavf-o", "fflags=+fastseek"),
                (
                    "stream-lavf-o",
                    "reconnect=1,reconnect_streamed=1,reconnect_on_network_error=1,reconnect_delay_max=5",
                ),
            ];
            for (name, value) in options {
                if let Err(err) = initializer.set_property(name, value) {
                    eprintln!("[ThumbFast] cannot set {name}={value}: {err:?}");
                }
            }
            Ok(())
        })
        .map_err(|e| format!("failed to initialize thumbnailer mpv: {e:?}"))?;

        let mut mpv = Arc::new(mpv);
        let wake_for_mpv = wake_sender.clone();
        Arc::get_mut(&mut mpv)
            .expect("unique mpv arc")
            .set_wakeup_callback(move || {
                let _ = wake_for_mpv.try_send(WorkerWake::Mpv);
            });

        let render = SwRenderContext::new(Arc::clone(&mpv), wake_sender)?;
        Ok(Self { mpv, render })
    }
}

pub fn compute_thumbnail_dimensions(
    video_w: Option<f64>,
    video_h: Option<f64>,
    max_width: u32,
) -> (u32, u32) {
    let clamped_max = max_width.clamp(MIN_THUMB_MAX_WIDTH, MAX_THUMB_MAX_WIDTH);
    let (vw, vh) = match (video_w, video_h) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => (w, h),
        _ => (16.0, 9.0),
    };

    let aspect = vw / vh;
    let (raw_w, raw_h) = if aspect >= 1.0 {
        let w = clamped_max as f64;
        let h = w / aspect;
        (w, h)
    } else {
        // Cap vertical video height at clamped_max so tall thumbnails don't overflow the player.
        let h = clamped_max as f64;
        let w = h * aspect;
        (w, h)
    };

    let width = ((raw_w.round() as u32).max(64)) & !1;
    let height = ((raw_h.round() as u32).max(36)) & !1;
    (width.max(64), height.max(36))
}

pub fn encode_bgr0_to_jpeg(
    bgr0: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    quality: u8,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width > u16::MAX as usize || height > u16::MAX as usize {
        return Err(format!("invalid thumbnail dimensions: {width}x{height}"));
    }
    if stride < width * 4 {
        return Err(format!("stride {stride} smaller than row width {}", width * 4));
    }
    let required = stride
        .checked_mul(height)
        .ok_or_else(|| "buffer size overflow".to_string())?;
    if bgr0.len() < required {
        return Err(format!(
            "bgr0 buffer too short: {} < {required}",
            bgr0.len()
        ));
    }

    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row_start = y * stride;
        for x in 0..width {
            let px = row_start + x * 4;
            rgb.push(bgr0[px + 2]); // R
            rgb.push(bgr0[px + 1]); // G
            rgb.push(bgr0[px]);     // B
        }
    }

    let mut out = Vec::new();
    Encoder::new(&mut out, quality)
        .encode(&rgb, width as u16, height as u16, ColorType::Rgb)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn quantize_step_secs(duration: Option<f64>) -> f64 {
    match duration {
        Some(d) if d > 0.0 && d <= 120.0 => 0.5,
        Some(d) if d > 0.0 && d <= 600.0 => 2.0,
        Some(d) if d > 0.0 && d <= 2400.0 => 4.0,
        Some(d) if d > 0.0 && d <= 5400.0 => 6.0,
        Some(d) if d > 5400.0 => 8.0,
        _ => 2.0,
    }
}

fn quantize_time(time_sec: f64, duration: Option<f64>) -> i64 {
    let step = quantize_step_secs(duration);
    (time_sec / step).round() as i64
}

pub fn is_precacheable_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Ok(parsed) = url::Url::parse(trimmed) {
        if parsed.scheme().len() == 1 && std::path::Path::new(trimmed).is_absolute() {
            return true;
        }
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port_or_known_default();
        if matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1") && port == Some(11470) {
            return false;
        }
        return matches!(parsed.scheme(), "http" | "https" | "file");
    }
    std::path::Path::new(trimmed).is_absolute()
}

pub fn build_precache_schedule(duration: f64, max_slots: usize) -> VecDeque<f64> {
    let mut out = VecDeque::new();
    if !duration.is_finite() || duration <= 10.0 || max_slots == 0 {
        return out;
    }
    let count = ((duration / 15.0).round() as usize).clamp(8, max_slots);
    let start = duration * 0.02;
    let end = duration * 0.98;
    let span = (end - start).max(0.0);
    let mut slots = Vec::with_capacity(count);
    if count == 1 {
        slots.push(duration * 0.5);
    } else {
        for i in 0..count {
            let frac = i as f64 / (count - 1) as f64;
            slots.push(start + span * frac);
        }
    }

    let mut ranges = VecDeque::new();
    ranges.push_back((0usize, slots.len() - 1));
    while let Some((lo, hi)) = ranges.pop_front() {
        if lo > hi {
            continue;
        }
        let mid = lo + (hi - lo) / 2;
        out.push_back(slots[mid]);
        if mid > lo {
            ranges.push_back((lo, mid - 1));
        }
        if mid < hi {
            ranges.push_back((mid + 1, hi));
        }
    }
    out
}

fn read_video_dimensions(mpv: &Mpv) -> (Option<f64>, Option<f64>) {
    if let (Ok(dw), Ok(dh)) = (
        mpv.get_property::<f64>("dwidth"),
        mpv.get_property::<f64>("dheight"),
    ) {
        if dw > 0.0 && dh > 0.0 {
            return (Some(dw), Some(dh));
        }
    }
    if let Ok(params_str) = mpv.get_property::<String>("video-params") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&params_str) {
            let w = val.get("w").and_then(|v| v.as_f64());
            let h = val.get("h").and_then(|v| v.as_f64());
            if w.is_some_and(|v| v > 0.0) && h.is_some_and(|v| v > 0.0) {
                return (w, h);
            }
        }
    }
    (None, None)
}

fn is_time_pos_matching(
    mpv: &Mpv,
    target_time: f64,
    duration: Option<f64>,
    render_ready: bool,
) -> bool {
    match mpv.get_property::<f64>("time-pos") {
        Ok(pos) if pos.is_finite() => {
            duration.is_some_and(|d| d <= 60.0) || (pos - target_time).abs() <= 30.0
        }
        _ => render_ready,
    }
}

fn send_thumbnail_to_webview(
    webview_cmd_tx: &Sender<WebViewCommand>,
    time: f64,
    data_url: &str,
    width: u32,
    height: u32,
    is_precache: bool,
) {
    let payload = serde_json::json!({
        "time": time,
        "dataUrl": data_url,
        "width": width,
        "height": height,
        "precache": is_precache,
    });
    if let Ok(payload_json) = serde_json::to_string(&payload) {
        let script = format!(
            "window.__receiveThumbnailPreview && window.__receiveThumbnailPreview({payload_json});"
        );
        let _ = webview_cmd_tx.send(WebViewCommand::ExecuteScript(script));
    }
}

pub fn spawn_thumbnail_worker(
    webview_cmd_tx: Sender<WebViewCommand>,
    initial_enabled: bool,
    initial_precache: bool,
    initial_preview_size: u32,
) -> (Sender<ThumbnailCmd>, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = flume::unbounded::<ThumbnailCmd>();
    let (wake_tx, wake_rx) = flume::bounded::<WorkerWake>(16);

    let cmd_wake_tx = wake_tx.clone();
    let (proxy_tx, proxy_rx) = flume::unbounded::<ThumbnailCmd>();

    // Forwarder notifies worker wake channel immediately whenever a command arrives.
    thread::Builder::new()
        .name("thumbfast-dispatch".to_string())
        .spawn(move || {
            while let Ok(cmd) = proxy_rx.recv() {
                if cmd_tx.send(cmd).is_err() {
                    break;
                }
                let _ = cmd_wake_tx.try_send(WorkerWake::Cmd);
            }
        })
        .expect("failed to start thumbfast dispatcher");

    let handle = thread::Builder::new()
        .name("thumbfast-worker".to_string())
        .spawn(move || {
            run_thumbnail_loop(
                cmd_rx,
                wake_rx,
                wake_tx,
                webview_cmd_tx,
                initial_enabled,
                initial_precache,
                initial_preview_size,
            );
        })
        .expect("failed to start thumbfast worker");

    (proxy_tx, handle)
}

fn run_thumbnail_loop(
    cmd_rx: Receiver<ThumbnailCmd>,
    wake_rx: Receiver<WorkerWake>,
    wake_tx: Sender<WorkerWake>,
    webview_cmd_tx: Sender<WebViewCommand>,
    initial_enabled: bool,
    initial_precache: bool,
    initial_preview_size: u32,
) {
    let mut enabled = initial_enabled;
    let mut precache_enabled = initial_precache;
    let mut preview_size = initial_preview_size.clamp(MIN_THUMB_MAX_WIDTH, MAX_THUMB_MAX_WIDTH);
    let mut engine: Option<ThumbnailEngine> = None;
    let mut current_url: Option<String> = None;
    let mut file_ready = false;
    let mut load_failed = false;
    let mut load_started_at: Option<Instant> = None;
    let mut duration: Option<f64> = None;
    let mut pending_request: Option<(f64, u32)> = None;
    let mut in_flight: Option<InFlightSeek> = None;
    let mut cache: HashMap<(i64, u32), CachedThumb> = HashMap::new();
    let mut cache_order: VecDeque<(i64, u32)> = VecDeque::new();
    let mut precache_queue: VecDeque<f64> = VecDeque::new();
    let mut precache_not_before: Option<Instant> = None;
    let mut precache_consecutive_failures: u32 = 0;

    let reset_cache = |cache: &mut HashMap<(i64, u32), CachedThumb>,
                       cache_order: &mut VecDeque<(i64, u32)>| {
        cache.clear();
        cache_order.clear();
    };

    loop {
        // 1. Drain all pending commands (latest-wins for thumbnail requests).
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                ThumbnailCmd::SetEnabled(next_enabled) => {
                    if enabled != next_enabled {
                        enabled = next_enabled;
                        if !enabled {
                            pending_request = None;
                            in_flight = None;
                            file_ready = false;
                            load_failed = false;
                            load_started_at = None;
                            duration = None;
                            precache_queue.clear();
                            precache_not_before = None;
                            precache_consecutive_failures = 0;
                            reset_cache(&mut cache, &mut cache_order);
                            if let Some(eng) = engine.take() {
                                let _ = eng.mpv.command("stop", &[]);
                                drop(eng);
                            }
                        } else if let Some(ref url) = current_url {
                            // Re-warm current stream when toggled back on.
                            if engine.is_none() {
                                match ThumbnailEngine::create(wake_tx.clone()) {
                                    Ok(e) => engine = Some(e),
                                    Err(err) => {
                                        eprintln!("[ThumbFast] {err}");
                                        load_failed = true;
                                        continue;
                                    }
                                }
                            }
                            if let Some(ref eng) = engine {
                                file_ready = false;
                                load_failed = false;
                                load_started_at = Some(Instant::now());
                                duration = None;
                                in_flight = None;
                                precache_queue.clear();
                                precache_not_before = None;
                                precache_consecutive_failures = 0;
                                if let Err(err) = eng.mpv.command("loadfile", &[url.as_str(), "replace"]) {
                                    eprintln!("[ThumbFast] loadfile failed: {err:?}");
                                    load_failed = true;
                                }
                            }
                        }
                    }
                }
                ThumbnailCmd::SetPrecache(next_precache) => {
                    if precache_enabled != next_precache {
                        precache_enabled = next_precache;
                        if !precache_enabled {
                            precache_queue.clear();
                            precache_not_before = None;
                            precache_consecutive_failures = 0;
                            if in_flight.as_ref().is_some_and(|s| s.is_precache) {
                                in_flight = None;
                            }
                        } else if enabled
                            && file_ready
                            && !load_failed
                            && current_url.as_deref().is_some_and(is_precacheable_url)
                        {
                            if let Some(d) = duration {
                                precache_queue = build_precache_schedule(d, PRECACHE_MAX_SLOTS);
                                precache_not_before = Some(Instant::now());
                                precache_consecutive_failures = 0;
                            }
                        }
                    }
                }
                ThumbnailCmd::SetPreviewSize(next_size) => {
                    let clamped = next_size.clamp(MIN_THUMB_MAX_WIDTH, MAX_THUMB_MAX_WIDTH);
                    if clamped != preview_size {
                        preview_size = clamped;
                        reset_cache(&mut cache, &mut cache_order);
                        if in_flight.as_ref().is_some_and(|s| s.is_precache) {
                            in_flight = None;
                        }
                        if precache_enabled
                            && enabled
                            && file_ready
                            && !load_failed
                            && current_url.as_deref().is_some_and(is_precacheable_url)
                        {
                            if let Some(d) = duration {
                                precache_queue = build_precache_schedule(d, PRECACHE_MAX_SLOTS);
                                precache_not_before = Some(Instant::now());
                                precache_consecutive_failures = 0;
                            }
                        }
                    }
                }
                ThumbnailCmd::SetSource(maybe_url) => {
                    if maybe_url == current_url && (!enabled || engine.is_some() || maybe_url.is_none()) {
                        continue;
                    }
                    current_url = maybe_url.clone();
                    file_ready = false;
                    load_failed = false;
                    load_started_at = None;
                    duration = None;
                    pending_request = None;
                    in_flight = None;
                    precache_queue.clear();
                    precache_not_before = None;
                    precache_consecutive_failures = 0;
                    reset_cache(&mut cache, &mut cache_order);

                    match maybe_url {
                        None => {
                            if let Some(ref eng) = engine {
                                let _ = eng.mpv.command("stop", &[]);
                            }
                        }
                        Some(url) if enabled => {
                            if engine.is_none() {
                                match ThumbnailEngine::create(wake_tx.clone()) {
                                    Ok(e) => engine = Some(e),
                                    Err(err) => {
                                        eprintln!("[ThumbFast] {err}");
                                        load_failed = true;
                                        continue;
                                    }
                                }
                            }
                            if let Some(ref eng) = engine {
                                load_started_at = Some(Instant::now());
                                if let Err(err) = eng.mpv.command("loadfile", &[url.as_str(), "replace"]) {
                                    eprintln!("[ThumbFast] loadfile failed: {err:?}");
                                    load_failed = true;
                                }
                            }
                        }
                        Some(_) => {}
                    }
                }
                ThumbnailCmd::Request { time_sec, max_width } => {
                    if !enabled || current_url.is_none() || !time_sec.is_finite() || time_sec < 0.0 {
                        continue;
                    }
                    let clamped_width = max_width.clamp(MIN_THUMB_MAX_WIDTH, MAX_THUMB_MAX_WIDTH);
                    pending_request = Some((time_sec, clamped_width));
                    precache_not_before = Some(Instant::now() + PRECACHE_HOVER_COOLDOWN);
                }
            }
        }

        // 2. Drain mpv events if engine is active.
        if let Some(ref eng) = engine {
            loop {
                let raw_ev = unsafe { &*libmpv2_sys::mpv_wait_event(eng.mpv.ctx.as_ptr(), 0.0) };
                match raw_ev.event_id {
                    libmpv2_sys::mpv_event_id_MPV_EVENT_NONE => break,
                    libmpv2_sys::mpv_event_id_MPV_EVENT_SHUTDOWN => {
                        file_ready = false;
                        load_failed = true;
                        break;
                    }
                    libmpv2_sys::mpv_event_id_MPV_EVENT_FILE_LOADED => {
                        file_ready = true;
                        load_failed = false;
                        load_started_at = None;
                        if let Ok(d) = eng.mpv.get_property::<f64>("duration") {
                            if d.is_finite() && d > 0.0 {
                                duration = Some(d);
                            }
                        }
                        if precache_enabled
                            && enabled
                            && current_url.as_deref().is_some_and(is_precacheable_url)
                        {
                            if let Some(d) = duration {
                                precache_queue = build_precache_schedule(d, PRECACHE_MAX_SLOTS);
                                precache_not_before = Some(Instant::now() + PRECACHE_START_DELAY);
                                precache_consecutive_failures = 0;
                            }
                        }
                    }
                    libmpv2_sys::mpv_event_id_MPV_EVENT_PLAYBACK_RESTART => {
                        if let Some(ref mut seek) = in_flight {
                            let stale_preempt = duration.map_or(false, |d| d > 60.0)
                                && seek.started_at.elapsed() < Duration::from_millis(2500)
                                && eng
                                    .mpv
                                    .get_property::<f64>("time-pos")
                                    .ok()
                                    .filter(|p| p.is_finite())
                                    .is_some_and(|pos| (pos - seek.clamped_time).abs() > 30.0);
                            if !stale_preempt {
                                seek.seek_settled = true;
                            }
                        }
                    }
                    libmpv2_sys::mpv_event_id_MPV_EVENT_END_FILE => {
                        if !file_ready {
                            load_failed = true;
                        }
                        in_flight = None;
                    }
                    _ => {}
                }
            }

            // Check render update flags so we know if a frame is ready.
            let update_flags = eng.render.update_flags();
            if (update_flags & (libmpv2_sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64)) != 0 {
                if let Some(ref mut seek) = in_flight {
                    seek.render_ready = true;
                }
            }

            if !file_ready
                && !load_failed
                && load_started_at.is_some_and(|t| t.elapsed() > STREAM_LOAD_TIMEOUT)
            {
                load_failed = true;
            }

            // Preempt stale in-flight seek when user requests a new hover spot.
            if let (Some((req_time, req_width)), Some(ref seek)) = (pending_request, in_flight.as_ref()) {
                let new_qkey = quantize_time(req_time, duration);
                if seek.is_precache
                    || ((seek.quantized_key != new_qkey || seek.max_width != req_width)
                        && seek.started_at.elapsed() >= PREEMPT_MIN_ELAPSED)
                {
                    in_flight = None;
                }
            }

            // 3. Complete in-flight seek & render frame when ready.
            if let Some(ref seek) = in_flight {
                let elapsed = seek.started_at.elapsed();
                let ready_to_render = seek.seek_settled
                    && (seek.render_ready
                        || (elapsed > Duration::from_millis(80)
                            && is_time_pos_matching(
                                &eng.mpv,
                                seek.clamped_time,
                                duration,
                                seek.render_ready,
                            )));
                if ready_to_render {
                    if duration.map_or(true, |d| d > 60.0)
                        && !is_time_pos_matching(
                            &eng.mpv,
                            seek.clamped_time,
                            duration,
                            seek.render_ready,
                        )
                    {
                        in_flight = None;
                    } else {
                        let (vw, vh) = read_video_dimensions(&eng.mpv);
                        let (tw, th) = compute_thumbnail_dimensions(vw, vh, seek.max_width);
                        match eng.render.render_bgr0(tw, th) {
                            Ok((bgr0, stride)) => {
                                match encode_bgr0_to_jpeg(
                                    &bgr0,
                                    stride,
                                    tw as usize,
                                    th as usize,
                                    JPEG_QUALITY,
                                ) {
                                    Ok(jpeg) => {
                                        let data_url = format!(
                                            "data:image/jpeg;base64,{}",
                                            BASE64_STANDARD.encode(&jpeg)
                                        );
                                        let cache_key = (seek.quantized_key, seek.max_width);
                                        if !cache.contains_key(&cache_key) {
                                            if cache_order.len() >= MAX_CACHE_ENTRIES {
                                                if let Some(old_key) = cache_order.pop_front() {
                                                    cache.remove(&old_key);
                                                }
                                            }
                                            cache_order.push_back(cache_key);
                                        }
                                        cache.insert(
                                            cache_key,
                                            CachedThumb {
                                                data_url: data_url.clone(),
                                                width: tw,
                                                height: th,
                                            },
                                        );
                                        send_thumbnail_to_webview(
                                            &webview_cmd_tx,
                                            seek.requested_time,
                                            &data_url,
                                            tw,
                                            th,
                                            seek.is_precache,
                                        );
                                        if seek.is_precache {
                                            precache_consecutive_failures = 0;
                                            precache_not_before =
                                                Some(Instant::now() + PRECACHE_STEP_COOLDOWN);
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!("[ThumbFast] jpeg encode failed: {err}");
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!("[ThumbFast] render failed: {err}");
                            }
                        }
                        in_flight = None;
                    }
                } else if elapsed > SEEK_TIMEOUT {
                    if seek.is_precache {
                        precache_consecutive_failures = precache_consecutive_failures.saturating_add(1);
                        if precache_consecutive_failures >= PRECACHE_MAX_CONSECUTIVE_FAILURES {
                            precache_queue.clear();
                        }
                    }
                    in_flight = None;
                }
            }

            // 4. Start next pending request (user hover first, then background precache).
            if enabled && file_ready && !load_failed && in_flight.is_none() {
                if let Some((req_time, req_width)) = pending_request.take() {
                    let qkey = quantize_time(req_time, duration);
                    if let Some(hit) = cache.get(&(qkey, req_width)) {
                        send_thumbnail_to_webview(
                            &webview_cmd_tx,
                            req_time,
                            &hit.data_url,
                            hit.width,
                            hit.height,
                            false,
                        );
                    } else {
                        let clamped_time = match duration {
                            Some(d) if d > 0.5 => req_time.clamp(0.0, d - 0.25),
                            _ => req_time.max(0.0),
                        };
                        let time_arg = format!("{clamped_time:.3}");
                        if let Err(err) = eng
                            .mpv
                            .command("seek", &[time_arg.as_str(), "absolute+keyframes"])
                        {
                            eprintln!("[ThumbFast] seek({time_arg}) failed: {err:?}");
                        } else {
                            in_flight = Some(InFlightSeek {
                                requested_time: req_time,
                                clamped_time,
                                quantized_key: qkey,
                                max_width: req_width,
                                started_at: Instant::now(),
                                seek_settled: false,
                                render_ready: false,
                                is_precache: false,
                            });
                        }
                    }
                } else if precache_enabled
                    && !precache_queue.is_empty()
                    && precache_not_before.map_or(true, |t| Instant::now() >= t)
                {
                    while let Some(precache_time) = precache_queue.pop_front() {
                        let qkey = quantize_time(precache_time, duration);
                        if cache.contains_key(&(qkey, preview_size)) {
                            continue;
                        }
                        let clamped_time = match duration {
                            Some(d) if d > 0.5 => precache_time.clamp(0.0, d - 0.25),
                            _ => precache_time.max(0.0),
                        };
                        let time_arg = format!("{clamped_time:.3}");
                        if let Err(err) = eng
                            .mpv
                            .command("seek", &[time_arg.as_str(), "absolute+keyframes"])
                        {
                            eprintln!("[ThumbFast] precache seek({time_arg}) failed: {err:?}");
                            precache_consecutive_failures =
                                precache_consecutive_failures.saturating_add(1);
                            if precache_consecutive_failures >= PRECACHE_MAX_CONSECUTIVE_FAILURES {
                                precache_queue.clear();
                            }
                        } else {
                            in_flight = Some(InFlightSeek {
                                requested_time: precache_time,
                                clamped_time,
                                quantized_key: qkey,
                                max_width: preview_size,
                                started_at: Instant::now(),
                                seek_settled: false,
                                render_ready: false,
                                is_precache: true,
                            });
                        }
                        break;
                    }
                }
            }
        }

        // 5. Wait for next command, mpv event, or render update (or short poll while seeking/precaching).
        let wait_timeout = if in_flight.is_some()
            || (!file_ready && !load_failed && load_started_at.is_some())
        {
            Duration::from_millis(20)
        } else if enabled
            && file_ready
            && !load_failed
            && precache_enabled
            && !precache_queue.is_empty()
            && pending_request.is_none()
        {
            match precache_not_before {
                Some(target) => target
                    .saturating_duration_since(Instant::now())
                    .clamp(Duration::from_millis(20), Duration::from_millis(200)),
                None => Duration::from_millis(20),
            }
        } else {
            Duration::from_secs(60)
        };

        match wake_rx.recv_timeout(wait_timeout) {
            Ok(_) => {
                // Drain extra queued wakes so we don't spin unnecessarily.
                while wake_rx.try_recv().is_ok() {}
            }
            Err(flume::RecvTimeoutError::Timeout) => {}
            Err(flume::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_thumbnail_dimensions() {
        assert_eq!(compute_thumbnail_dimensions(Some(1920.0), Some(1080.0), 240), (240, 134));
        assert_eq!(compute_thumbnail_dimensions(Some(1280.0), Some(720.0), 240), (240, 134));
        assert_eq!(compute_thumbnail_dimensions(Some(1080.0), Some(1920.0), 240), (134, 240));
        assert_eq!(compute_thumbnail_dimensions(None, None, 240), (240, 134));
    }

    #[test]
    fn test_encode_bgr0_to_jpeg_header_and_size() {
        let width = 160;
        let height = 90;
        let stride = width * 4;
        let mut bgr0 = vec![0u8; stride * height];
        for y in 0..height {
            for x in 0..width {
                let px = y * stride + x * 4;
                bgr0[px] = ((x * 2) % 256) as u8;
                bgr0[px + 1] = ((y * 2) % 256) as u8;
                bgr0[px + 2] = (((x + y) * 2) % 256) as u8;
                bgr0[px + 3] = 255;
            }
        }

        let jpeg = encode_bgr0_to_jpeg(&bgr0, stride, width, height, JPEG_QUALITY)
            .expect("jpeg encode should succeed");
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        assert!(jpeg.ends_with(&[0xFF, 0xD9]));
        assert!(
            jpeg.len() < 54 + 160 * 90 * 3 / 4,
            "expected JPEG size {} to be much smaller than BMP",
            jpeg.len()
        );
    }

    #[test]
    fn test_sw_render_context_creation() {
        let (wake_tx, _wake_rx) = flume::bounded(4);
        let engine = ThumbnailEngine::create(wake_tx);
        assert!(engine.is_ok(), "SwRenderContext failed: {:?}", engine.err());
    }

    #[test]
    fn test_thumbnail_worker_renders_real_video_frame() {
        let ffmpeg = std::path::Path::new("bin/ffmpeg.exe");
        if !ffmpeg.exists() {
            return;
        }
        let temp_mp4 = std::env::temp_dir().join(format!(
            "stremio_thumbfast_test_{}.mp4",
            std::process::id()
        ));
        let status = std::process::Command::new(ffmpeg)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=0x2288ee:s=320x180:d=2:r=25",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                temp_mp4.to_str().unwrap(),
            ])
            .status()
            .expect("failed to run ffmpeg");
        if !status.success() {
            // Fallback to mpeg4 if libx264 isn't built into bundled ffmpeg
            let status2 = std::process::Command::new(ffmpeg)
                .args([
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=0x2288ee:s=320x180:d=2:r=25",
                    "-c:v",
                    "mpeg4",
                    "-pix_fmt",
                    "yuv420p",
                    temp_mp4.to_str().unwrap(),
                ])
                .status()
                .expect("failed to run ffmpeg fallback");
            assert!(status2.success(), "ffmpeg test fixture failed");
        }

        let (wv_tx, wv_rx) = flume::unbounded();
        let (thumb_tx, _handle) = spawn_thumbnail_worker(wv_tx, true, false, 160);
        thumb_tx
            .send(ThumbnailCmd::SetSource(Some(
                temp_mp4.to_string_lossy().to_string(),
            )))
            .unwrap();
        thumb_tx
            .send(ThumbnailCmd::Request {
                time_sec: 1.0,
                max_width: 160,
            })
            .unwrap();

        let msg = wv_rx
            .recv_timeout(Duration::from_secs(8))
            .expect("expected thumbnail script callback within timeout");
        let _ = std::fs::remove_file(&temp_mp4);

        match msg {
            WebViewCommand::ExecuteScript(script) => {
                assert!(script.contains("__receiveThumbnailPreview"));
                assert!(script.contains("data:image/jpeg;base64,/9j/"));
                assert!(script.contains("\"width\":160"));
                assert!(script.contains("\"height\":90"));
            }
            _ => panic!("unexpected WebViewCommand"),
        }
    }

    #[test]
    fn test_precache_schedule_and_url_filter() {
        assert!(!is_precacheable_url("http://127.0.0.1:11470/hash/0"));
        assert!(!is_precacheable_url("http://localhost:11470/stream.mp4"));
        assert!(!is_precacheable_url(""));
        assert!(is_precacheable_url("https://cdn.example.com/movie.mkv"));
        assert!(is_precacheable_url("http://example.com:8080/video.mp4"));
        assert!(is_precacheable_url("file:///C:/Videos/test.mp4"));
        assert!(is_precacheable_url(r"C:\Videos\test.mp4"));

        assert_eq!(quantize_step_secs(Some(60.0)), 0.5);
        assert_eq!(quantize_step_secs(Some(300.0)), 2.0);
        assert_eq!(quantize_step_secs(Some(1800.0)), 4.0);
        assert_eq!(quantize_step_secs(Some(3600.0)), 6.0);
        assert_eq!(quantize_step_secs(Some(7200.0)), 8.0);
        assert_eq!(quantize_step_secs(None), 2.0);

        assert!(build_precache_schedule(5.0, PRECACHE_MAX_SLOTS).is_empty());
        assert!(build_precache_schedule(f64::NAN, PRECACHE_MAX_SLOTS).is_empty());

        let sched = build_precache_schedule(3600.0, 20);
        assert_eq!(sched.len(), 20);
        let first = *sched.front().unwrap();
        assert!(
            (first - 1800.0).abs() < 250.0,
            "expected first bisection point near midpoint, got {}",
            first
        );
        for &t in &sched {
            assert!(t >= 3600.0 * 0.02 - 1e-6 && t <= 3600.0 * 0.98 + 1e-6);
        }
    }
}

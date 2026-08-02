//! Background playback daemon for CLI mode.
//!
//! Runs a persistent `AudioPlayer` on a dedicated thread, exposes a localhost
//! TCP control socket for `wave playback …` subcommands, and shows a system
//! tray / menu-bar icon with playlist and transport controls.

#[cfg(not(target_os = "android"))]
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "android"))]
use tray_icon::{TrayIconBuilder, TrayIconEvent};

use crate::app_paths::{daemon_state_path, library_db_path};
use crate::audio::player::{AudioPlayer, RepeatMode};
use crate::library::Library;
use crate::media_controls::TrackMetadata;
use crate::metadata::Track;
use crate::path_validation::validate_audio_path;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── IPC types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DaemonEnvelope {
    token: String,
    #[serde(flatten)]
    request: DaemonRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum DaemonRequest {
    Start { id: String },
    Pause,
    Resume,
    Stop,
    Next,
    Previous,
    Seek { seconds: f64 },
    Status,
    Shutdown,
    QueueList,
    QueueAdd { track_id: String },
    QueueRemove { index: usize },
    QueueInsertNext { track_id: String },
    QueueShuffle { enable: Option<bool> },
    QueueRepeat { mode: String },
    QueueClear,
    Volume { level: f32 },
    SetDevice { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackStatus {
    pub state: String,
    pub file: String,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub volume: f32,
    pub device: String,
    pub repeat: String,
    pub shuffle: bool,
    pub queue_index: usize,
    pub queue_total: usize,
    pub title: Option<String>,
    pub artist: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PlaybackStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<String>>,
}

impl DaemonResponse {
    fn ok_msg(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: Some(message.into()),
            error: None,
            status: None,
            queue: None,
        }
    }

    fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: None,
            error: Some(error.into()),
            status: None,
            queue: None,
        }
    }
}

// ── Daemon state ──────────────────────────────────────────────────────────────

struct DaemonState {
    player: AudioPlayer,
    library: Library,
    media: DaemonMedia,
    shutdown: bool,
}

struct SharedState(Arc<Mutex<DaemonState>>);

#[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
struct DaemonMedia {
    controls: Option<souvlaki::MediaControls>,
}

#[cfg(any(target_os = "windows", target_os = "android"))]
struct DaemonMedia;

#[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
fn normalize_daemon_cover_url(cover_url: Option<&str>) -> Option<String> {
    let url = cover_url?.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with("file://")
    {
        return Some(url.to_string());
    }
    let path = std::path::Path::new(url);
    if !path.is_absolute() {
        return None;
    }
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !abs.is_file() {
        return None;
    }
    url::Url::from_file_path(&abs)
        .ok()
        .map(|u| u.to_string())
        .or_else(|| Some(format!("file://{}", abs.to_string_lossy())))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "android")))]
impl DaemonMedia {
    fn new() -> Self {
        use souvlaki::{MediaControls, PlatformConfig};
        let config = PlatformConfig {
            dbus_name: "app.bmdarklight.wave.daemon",
            display_name: "Wave",
            hwnd: None,
        };
        let controls = MediaControls::new(config).ok();
        Self { controls }
    }

    fn set_metadata(&mut self, meta: &TrackMetadata) {
        use souvlaki::MediaMetadata;
        let duration = meta.duration_seconds.map(Duration::from_secs_f64);
        let cover_url = normalize_daemon_cover_url(meta.cover_url.as_deref());
        if let Some(controls) = &mut self.controls {
            let _ = controls.set_metadata(MediaMetadata {
                title: meta.title.as_deref(),
                artist: meta.artist.as_deref(),
                album: meta.album.as_deref(),
                duration,
                cover_url: cover_url.as_deref(),
            });
        }
    }

    fn set_playback(&mut self, playing: bool, position_secs: f64, stopped: bool) {
        use souvlaki::{MediaPlayback, MediaPosition};
        if let Some(controls) = &mut self.controls {
            let playback = if stopped {
                MediaPlayback::Stopped
            } else if playing {
                MediaPlayback::Playing {
                    progress: Some(MediaPosition(Duration::from_secs_f64(position_secs))),
                }
            } else {
                MediaPlayback::Paused {
                    progress: Some(MediaPosition(Duration::from_secs_f64(position_secs))),
                }
            };
            let _ = controls.set_playback(playback);
        }
    }

    fn clear(&mut self) {
        self.set_playback(false, 0.0, true);
    }
}

#[cfg(any(target_os = "windows", target_os = "android"))]
impl DaemonMedia {
    fn new() -> Self {
        Self
    }

    fn set_metadata(&mut self, meta: &TrackMetadata) {
        let _ = meta;
    }

    fn set_playback(&mut self, playing: bool, position_secs: f64, stopped: bool) {
        let _ = (playing, position_secs, stopped);
    }

    fn clear(&mut self) {}
}

// ── Client API ────────────────────────────────────────────────────────────────

/// Send a request only if the daemon is already running (does not spawn).
pub fn daemon_request_if_running(request: DaemonRequest) -> Result<Option<DaemonResponse>, String> {
    let Some(conn) = read_daemon_connection() else {
        return Ok(None);
    };
    send_daemon_request(&conn, request).map(Some)
}

/// Send a request to the running playback daemon, starting it if needed.
pub fn daemon_request(request: DaemonRequest) -> Result<DaemonResponse, String> {
    let conn = ensure_daemon_connection()?;
    send_daemon_request(&conn, request)
}

fn send_daemon_request(conn: &DaemonConnection, request: DaemonRequest) -> Result<DaemonResponse, String> {
    let addr = format!("127.0.0.1:{}", conn.port);
    let mut stream =
        TcpStream::connect(&addr).map_err(|e| format!("Failed to connect to playback daemon: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;

    let envelope = DaemonEnvelope {
        token: conn.token.clone(),
        request,
    };
    let line = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    writeln!(stream, "{line}").map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(response_line.trim()).map_err(|e| e.to_string())
}

/// Ensure the background daemon is running; returns connection details.
fn ensure_daemon_connection() -> Result<DaemonConnection, String> {
    if let Some(conn) = read_daemon_connection() {
        return Ok(conn);
    }
    if crate::single_instance::gui_is_running() {
        return Err(crate::single_instance::already_running_message());
    }
    if crate::single_instance::daemon_is_running() {
        return Err(
            "Playback daemon is starting but not yet accepting connections. Try again.".to_string(),
        );
    }
    spawn_daemon()?;
    read_daemon_connection()
        .ok_or_else(|| "Playback daemon failed to publish connection details.".to_string())
}

/// Ensure the background daemon is running; returns its TCP port.
pub fn ensure_daemon_running() -> Result<u16, String> {
    Ok(ensure_daemon_connection()?.port)
}

/// Whether the CLI playback daemon is running and accepting IPC.
pub fn daemon_is_running() -> bool {
    read_daemon_connection().is_some()
}

// ── Daemon entry point ────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn init_macos_app() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
}

#[cfg(not(target_os = "macos"))]
fn init_macos_app() {}

pub fn run_daemon() {
    init_macos_app();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let library = Library::new_with_path(&library_db_path()).unwrap_or_else(|e| {
        eprintln!("Failed to open library: {e}");
        std::process::exit(1);
    });

    let player = AudioPlayer::new().unwrap_or_else(|e| {
        eprintln!("Failed to initialize audio player: {e}");
        std::process::exit(1);
    });

    let state = Arc::new(Mutex::new(DaemonState {
        player,
        library,
        media: DaemonMedia::new(),
        shutdown: false,
    }));

    let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|e| {
        eprintln!("Failed to bind playback daemon socket: {e}");
        std::process::exit(1);
    });
    let port = listener
        .local_addr()
        .expect("daemon listener address")
        .port();

    let auth_token = uuid::Uuid::new_v4().to_string();
    write_daemon_state(port, &auth_token).unwrap_or_else(|e| {
        eprintln!("Failed to write daemon state: {e}");
        std::process::exit(1);
    });

    let ipc_state = Arc::clone(&state);
    let token_for_ipc = auth_token.clone();
    std::thread::spawn(move || ipc_server_loop(listener, ipc_state, token_for_ipc));

    let tick_state = Arc::clone(&state);
    let tooltip: Arc<Mutex<String>> = Arc::new(Mutex::new("Wave".to_string()));
    let tooltip_for_tick = Arc::clone(&tooltip);
    std::thread::spawn(move || playback_tick_loop(tick_state, tooltip_for_tick));

    #[cfg(not(target_os = "android"))]
    run_tray_loop(state, tooltip);
    
    #[cfg(target_os = "android")]
    {
        // On Android, we don't run the tray loop - the main thread handles the daemon
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

fn request_shutdown(state: &Arc<Mutex<DaemonState>>) {
    if let Ok(mut guard) = state.lock() {
        guard.shutdown = true;
        let _ = guard.player.stop();
    }
    remove_daemon_state();
    std::process::exit(0);
}

// ── IPC server ────────────────────────────────────────────────────────────────

fn ipc_server_loop(listener: TcpListener, state: Arc<Mutex<DaemonState>>, auth_token: String) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        let state = Arc::clone(&state);
        let auth_token = auth_token.clone();
        std::thread::spawn(move || handle_ipc_connection(stream, state, auth_token));
    }
}

fn handle_ipc_connection(
    mut stream: TcpStream,
    state: Arc<Mutex<DaemonState>>,
    auth_token: String,
) {
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&stream);
        if reader.read_line(&mut line).is_err() {
            return;
        }
    }

    let envelope: DaemonEnvelope = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            let resp = DaemonResponse::err(format!("Invalid request: {e}"));
            let _ = write_response(&mut stream, &resp);
            return;
        }
    };

    if envelope.token != auth_token {
        let resp = DaemonResponse::err("Unauthorized daemon request");
        let _ = write_response(&mut stream, &resp);
        return;
    }

    let request = envelope.request;
    let should_shutdown = matches!(request, DaemonRequest::Shutdown | DaemonRequest::Stop);
    let response = {
        let mut guard = match state.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Daemon mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        handle_request(&mut guard, request)
    };

    let _ = write_response(&mut stream, &response);

    if should_shutdown {
        if let Ok(mut guard) = state.lock() {
            guard.shutdown = true;
        }
        remove_daemon_state();
        std::process::exit(0);
    }
}

fn write_response(stream: &mut TcpStream, response: &DaemonResponse) -> std::io::Result<()> {
    let line = serde_json::to_string(response).unwrap_or_else(|_| {
        r#"{"ok":false,"error":"serialization failed"}"#.to_string()
    });
    writeln!(stream, "{line}")
}

fn handle_request(state: &mut DaemonState, request: DaemonRequest) -> DaemonResponse {
    match request {
        DaemonRequest::Start { id } => daemon_start(state, &id),
        DaemonRequest::Pause => match state.player.pause() {
            Ok(()) => {
                sync_media_playback_state(state);
                DaemonResponse::ok_msg("Paused.")
            }
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::Resume => match state.player.resume() {
            Ok(()) => {
                sync_media_playback_state(state);
                DaemonResponse::ok_msg("Resumed.")
            }
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::Stop => {
            if let Err(e) = state.player.stop() {
                return DaemonResponse::err(e.to_string());
            }
            state.shutdown = true;
            state.media.clear();
            DaemonResponse::ok_msg("Playback stopped and daemon shut down.")
        }
        DaemonRequest::Next => match state.player.play_next() {
            Ok(Some(path)) => {
                sync_media_for_path(state, &path);
                let msg = format_now_playing(&state.library, &path);
                DaemonResponse::ok_msg(msg)
            }
            Ok(None) => DaemonResponse::ok_msg("End of queue."),
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::Previous => match state.player.play_previous() {
            Ok(Some(path)) => {
                sync_media_for_path(state, &path);
                let msg = format_now_playing(&state.library, &path);
                DaemonResponse::ok_msg(msg)
            }
            Ok(None) => DaemonResponse::ok_msg("Start of queue."),
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::Seek { seconds } => match state.player.seek(seconds) {
            Ok(()) => {
                sync_media_playback_state(state);
                DaemonResponse::ok_msg(format!("Seeked to {seconds:.1}s."))
            }
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::Status => DaemonResponse {
            ok: true,
            message: None,
            error: None,
            status: Some(build_status(&state.player, &state.library)),
            queue: None,
        },
        DaemonRequest::Shutdown => DaemonResponse::ok_msg("Daemon shutting down."),
        DaemonRequest::QueueList => {
            let tracks = state.player.queue.tracks().to_vec();
            DaemonResponse {
                ok: true,
                message: None,
                error: None,
                status: Some(build_status(&state.player, &state.library)),
                queue: Some(tracks),
            }
        }
        DaemonRequest::QueueAdd { track_id } => {
            let path = match resolve_track_path(&state.library, &track_id) {
                Ok(p) => p,
                Err(e) => return DaemonResponse::err(e),
            };
            if let Err(e) = validate_audio_path(&path) {
                return DaemonResponse::err(e);
            }
            state.player.enqueue(&path);
            DaemonResponse::ok_msg(format!("Added to queue: {path}"))
        }
        DaemonRequest::QueueRemove { index } => match state.player.remove_from_queue(index) {
            Some(path) => DaemonResponse::ok_msg(format!("Removed from queue: {path}")),
            None => DaemonResponse::err(format!("Invalid queue index: {index}")),
        },
        DaemonRequest::QueueInsertNext { track_id } => {
            let path = match resolve_track_path(&state.library, &track_id) {
                Ok(p) => p,
                Err(e) => return DaemonResponse::err(e),
            };
            if let Err(e) = validate_audio_path(&path) {
                return DaemonResponse::err(e);
            }
            state.player.insert_next(&path);
            DaemonResponse::ok_msg(format!("Will play next: {path}"))
        }
        DaemonRequest::QueueShuffle { enable } => {
            let on = match enable {
                Some(v) => v,
                None => !state.player.queue.is_shuffled(),
            };
            state.player.queue.set_shuffle(on);
            DaemonResponse::ok_msg(format!("Shuffle: {}", if on { "ON" } else { "OFF" }))
        }
        DaemonRequest::QueueRepeat { mode } => {
            let repeat = match mode.as_str() {
                "off" => RepeatMode::Off,
                "one" => RepeatMode::One,
                "all" => RepeatMode::All,
                _ => return DaemonResponse::err(format!("Invalid repeat mode: {mode}")),
            };
            state.player.repeat = repeat;
            DaemonResponse::ok_msg(format!("Repeat: {mode}"))
        }
        DaemonRequest::QueueClear => {
            state.player.clear_upcoming();
            DaemonResponse::ok_msg("Queue cleared (current track kept).")
        }
        DaemonRequest::Volume { level } => match state.player.set_volume(level) {
            Ok(()) => DaemonResponse::ok_msg(format!("Volume set to {:.0}%.", level * 100.0)),
            Err(e) => DaemonResponse::err(e.to_string()),
        },
        DaemonRequest::SetDevice { name } => match rebuild_player_on_device(state, &name) {
            Ok(()) => {
                sync_media_current_track(state);
                DaemonResponse::ok_msg(format!("Switched to output device: {name}"))
            }
            Err(e) => DaemonResponse::err(e),
        },
    }
}

fn daemon_start(state: &mut DaemonState, id: &str) -> DaemonResponse {
    let is_playlist = uuid::Uuid::parse_str(id).is_ok()
        && state
            .library
            .get_playlist_info(id)
            .ok()
            .flatten()
            .is_some();

    if is_playlist {
        match state.library.get_playlist_tracks(id) {
            Ok(tracks) if tracks.is_empty() => DaemonResponse::err("Playlist is empty."),
            Ok(tracks) => {
                let paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();
                state.player.queue.set_tracks(paths);
                if state.player.queue.jump(0).is_none() {
                    return DaemonResponse::err("Failed to set queue.");
                }
                let first_path = tracks[0].path.clone();
                if let Err(e) = state.player.play(&first_path) {
                    return DaemonResponse::err(e.to_string());
                }
                sync_media_for_track(state, &tracks[0]);
                let msg = format!(
                    "Playing playlist \"{}\" — {} track(s), starting with: {} — {}",
                    tracks[0].album, tracks.len(), tracks[0].artist, tracks[0].title
                );
                DaemonResponse::ok_msg(msg)
            }
            Err(e) => DaemonResponse::err(e),
        }
    } else {
        let path = match resolve_track_path(&state.library, id) {
            Ok(p) => p,
            Err(e) => return DaemonResponse::err(e),
        };
        if let Err(e) = validate_audio_path(&path) {
            return DaemonResponse::err(e);
        }
        state.player.enqueue(&path);
        state.player.queue.jump(0);
        if let Err(e) = state.player.play(&path) {
            return DaemonResponse::err(e.to_string());
        }
        sync_media_for_path(state, &path);
        DaemonResponse::ok_msg(format_now_playing(&state.library, &path))
    }
}

fn rebuild_player_on_device(state: &mut DaemonState, name: &str) -> Result<(), String> {
    let old = std::mem::replace(
        &mut state.player,
        AudioPlayer::new_with_device(name).map_err(|e| e.to_string())?,
    );
    state.player.queue = old.queue.clone();
    state.player.repeat = old.repeat.clone();
    let vol = old.volume();
    state.player.set_volume(vol).map_err(|e| e.to_string())?;
    if let Some(path) = old.get_current_path() {
        let path = path.to_string_lossy().to_string();
        let pos = old.position_seconds();
        let was_paused = old.is_paused();
        state.player.play(&path).map_err(|e| e.to_string())?;
        if pos > 0.0 {
            state.player.seek(pos).ok();
        }
        if was_paused {
            state.player.pause().ok();
        }
    }
    Ok(())
}

// ── Playback tick ─────────────────────────────────────────────────────────────

fn playback_tick_loop(state: Arc<Mutex<DaemonState>>, tooltip: Arc<Mutex<String>>) {
    loop {
        std::thread::sleep(Duration::from_millis(500));

        let should_exit = state.lock().map(|g| g.shutdown).unwrap_or(false);
        if should_exit {
            break;
        }

        let mut guard = match state.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Daemon mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if !guard.player.should_auto_advance() {
            // keep waiting
        } else if let Ok(Some(path)) = guard.player.play_next() {
            sync_media_for_path(&mut guard, &path);
        } else {
            let _ = guard.player.stop();
            guard.media.clear();
        }

        sync_media_playback_state(&mut guard);

        if let Ok(mut tip) = tooltip.lock() {
            *tip = current_tooltip(&guard.player, &guard.library);
        }
    }
}

// ── Tray icon ─────────────────────────────────────────────────────────────────

#[cfg(not(target_os = "android"))]
fn run_tray_loop(state: Arc<Mutex<DaemonState>>, tooltip: Arc<Mutex<String>>) {
    let icon = load_tray_icon().unwrap_or_else(|e| {
        eprintln!("Failed to load tray icon: {e}");
        std::process::exit(1);
    });

    let menu = build_tray_menu(&state);
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Wave")
        .with_icon(icon)
        .with_icon_as_template(cfg!(target_os = "macos"))
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Failed to create tray icon: {e}");
            std::process::exit(1);
        });

    // Right-click opens the context menu; left-click toggles play/pause.
    let _ = tray.set_show_menu_on_left_click(false);

    let shared = SharedState(Arc::clone(&state));
    let mut last_play_pause_label = state
        .lock()
        .map(|g| play_pause_label(&g))
        .unwrap_or("Play");
    let mut last_playlist_refresh = std::time::Instant::now();

    loop {
        pump_tray_events();

        let mut menu_needs_refresh = false;

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            handle_menu_event(&shared, event.id.as_ref());
            menu_needs_refresh = true;
        }

        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: tray_icon::MouseButton::Left,
                ..
            } = event
            {
                toggle_play_pause(&shared);
                menu_needs_refresh = true;
            }
        }

        let current_label = state
            .lock()
            .map(|g| play_pause_label(&g))
            .unwrap_or("Play");
        if current_label != last_play_pause_label {
            last_play_pause_label = current_label;
            menu_needs_refresh = true;
        }

        if menu_needs_refresh || last_playlist_refresh.elapsed() > Duration::from_secs(30) {
            refresh_tray_menu(&tray, &state);
            last_play_pause_label = current_label;
            if last_playlist_refresh.elapsed() > Duration::from_secs(30) {
                last_playlist_refresh = std::time::Instant::now();
            }
        }

        if let Ok(tip) = tooltip.lock() {
            let _ = tray.set_tooltip(Some(tip.as_str()));
        }

        if state.lock().map(|g| g.shutdown).unwrap_or(false) {
            break;
        }

        std::thread::sleep(Duration::from_millis(16));
    }

    remove_daemon_state();
}

/// Dispatch OS events required for tray context menus (especially on Windows/macOS).
#[cfg(not(target_os = "android"))]
fn pump_tray_events() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoopRunInMode};
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 1);
        }
    }
}

#[cfg(not(target_os = "android"))]
fn handle_menu_event(shared: &SharedState, id: &str) {
    if id == "play_pause" {
        toggle_play_pause(shared);
        return;
    }
    if id == "stop" {
        request_shutdown(&shared.0);
        return;
    }
    if id == "next" {
        let _ = shared.0.lock().map(|mut g| g.player.play_next());
        return;
    }
    if id == "prev" {
        let _ = shared.0.lock().map(|mut g| g.player.play_previous());
        return;
    }
    if id == "quit" {
        request_shutdown(&shared.0);
    }
    if let Some(playlist_id) = id.strip_prefix("playlist:") {
        let mut guard = match shared.0.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("Daemon mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        let _ = daemon_start(&mut guard, playlist_id);
    }
}

fn toggle_play_pause(shared: &SharedState) {
    let mut guard = match shared.0.lock() {
        Ok(g) => g,
        Err(poisoned) => {
            tracing::warn!("Daemon mutex was poisoned, recovering");
            poisoned.into_inner()
        }
    };
    if guard.player.is_playing() {
        let _ = guard.player.pause();
    } else if guard.player.is_paused() {
        let _ = guard.player.resume();
    } else if let Some(path) = guard
        .player
        .get_current_path()
        .map(|p| p.to_string_lossy().to_string())
    {
        let _ = guard.player.play(&path);
    }
}

fn play_pause_label(state: &DaemonState) -> &'static str {
    if state.player.is_playing() {
        "Pause"
    } else {
        "Play"
    }
}

#[cfg(not(target_os = "android"))]
fn refresh_tray_menu(tray: &tray_icon::TrayIcon, state: &Arc<Mutex<DaemonState>>) {
    let menu = build_tray_menu(state);
    let _ = tray.set_menu(Some(Box::new(menu)));
}

#[cfg(not(target_os = "android"))]
fn build_tray_menu(state: &Arc<Mutex<DaemonState>>) -> Menu {
    let menu = Menu::new();

    let playlists_sub = Submenu::new("Playlists", true);
    if let Ok(guard) = state.lock() {
        if let Ok(playlists) = guard.library.list_playlists(None) {
            for pl in playlists.iter().take(30) {
                let _ = playlists_sub.append(&MenuItem::with_id(
                    format!("playlist:{}", pl.id),
                    &pl.name,
                    true,
                    None,
                ));
            }
        }
    }
    let _ = menu.append(&playlists_sub);
    let _ = menu.append(&PredefinedMenuItem::separator());

    let label = state
        .lock()
        .map(|g| play_pause_label(&g))
        .unwrap_or("Play");
    let _ = menu.append(&MenuItem::with_id("play_pause", label, true, None));
    let _ = menu.append(&MenuItem::with_id("prev", "Previous", true, None));
    let _ = menu.append(&MenuItem::with_id("next", "Next", true, None));
    let _ = menu.append(&MenuItem::with_id("stop", "Stop", true, None));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("quit", "Quit Wave", true, None));

    menu
}

#[cfg(not(target_os = "android"))]
fn load_tray_icon() -> Result<tray_icon::Icon, String> {
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../icons/tray-template.png");
    #[cfg(not(target_os = "macos"))]
    let bytes = include_bytes!("../icons/32x32.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("PNG decode: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("PNG frame: {e}"))?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((buf.len() / 3) * 4);
            for chunk in buf.chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        other => return Err(format!("Unsupported PNG color type: {other:?}")),
    };
    tray_icon::Icon::from_rgba(rgba, info.width, info.height).map_err(|e| e.to_string())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn resolve_track_path(library: &Library, id_or_path: &str) -> Result<String, String> {
    if uuid::Uuid::parse_str(id_or_path).is_ok() {
        if let Some(track) = library.get_track_by_id(id_or_path)? {
            return Ok(track.path);
        }
    }
    validate_audio_path(id_or_path).map(|p| p.to_string_lossy().into_owned())
}

fn track_for_path(library: &Library, path: &str) -> Option<Track> {
    library
        .get_tracks_by_paths(&[path.to_string()])
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
}

fn track_to_metadata(track: &Track) -> TrackMetadata {
    TrackMetadata {
        title: Some(track.title.clone()),
        artist: Some(track.artist.clone()),
        album: Some(track.album.clone()),
        duration_seconds: track.duration_seconds,
        cover_url: track.cover_art_data_url.clone(),
    }
}

fn sync_media_for_track(state: &mut DaemonState, track: &Track) {
    state.media.set_metadata(&track_to_metadata(track));
    sync_media_playback_state(state);
}

fn sync_media_for_path(state: &mut DaemonState, path: &str) {
    if let Some(track) = track_for_path(&state.library, path) {
        sync_media_for_track(state, &track);
    } else {
        sync_media_playback_state(state);
    }
}

fn sync_media_current_track(state: &mut DaemonState) {
    let current = state
        .player
        .get_current_path()
        .map(|p| p.to_string_lossy().into_owned());
    if let Some(path) = current {
        sync_media_for_path(state, &path);
    } else {
        state.media.clear();
    }
}

fn sync_media_playback_state(state: &mut DaemonState) {
    state.media.set_playback(
        state.player.is_playing(),
        state.player.position_seconds(),
        !state.player.is_playing() && !state.player.is_paused(),
    );
}

fn format_now_playing(library: &Library, path: &str) -> String {
    if let Some(t) = track_for_path(library, path) {
        format!("Now playing: {} — {}", t.artist, t.title)
    } else {
        format!("Now playing: {path}")
    }
}

fn build_status(player: &AudioPlayer, library: &Library) -> PlaybackStatus {
    let state = if player.is_playing() {
        "Playing"
    } else if player.is_paused() {
        "Paused"
    } else {
        "Stopped"
    }
    .to_string();

    let path = player
        .get_current_path()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let (title, artist) = track_for_path(library, &path)
        .map(|t| (Some(t.title), Some(t.artist)))
        .unwrap_or((None, None));

    let file = player
        .get_current_path()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("None")
        .to_string();

    PlaybackStatus {
        state,
        file,
        position_seconds: player.position_seconds(),
        duration_seconds: player.duration_seconds().unwrap_or(0.0),
        volume: player.volume(),
        device: AudioPlayer::current_output_name(),
        repeat: format!("{:?}", player.repeat),
        shuffle: player.queue.is_shuffled(),
        queue_index: player.queue.current_index().map_or(0, |i| i + 1),
        queue_total: player.queue.tracks().len(),
        title,
        artist,
    }
}

fn current_tooltip(player: &AudioPlayer, library: &Library) -> String {
    if let Some(path) = player.get_current_path().map(|p| p.to_string_lossy().to_string()) {
        if let Some(t) = track_for_path(library, &path) {
            let state = if player.is_playing() {
                "Playing"
            } else if player.is_paused() {
                "Paused"
            } else {
                "Stopped"
            };
            return format!("{state}: {} — {}", t.artist, t.title);
        }
    }
    "Wave".to_string()
}

// ── Daemon lifecycle files ────────────────────────────────────────────────────

#[derive(Clone)]
struct DaemonConnection {
    port: u16,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct DaemonStateFile {
    pid: u32,
    port: u16,
    token: String,
}

fn write_daemon_state(port: u16, token: &str) -> Result<(), String> {
    let path = daemon_state_path();
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(crate::app_paths::data_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let state = DaemonStateFile {
        pid: std::process::id(),
        port,
        token: token.to_string(),
    };
    let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn remove_daemon_state() {
    let _ = std::fs::remove_file(daemon_state_path());
}

fn read_daemon_connection() -> Option<DaemonConnection> {
    let path = daemon_state_path();
    let contents = std::fs::read_to_string(&path).ok()?;
    let state: DaemonStateFile = serde_json::from_str(&contents).ok()?;
    if !crate::single_instance::is_process_alive(state.pid) {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let addr = format!("127.0.0.1:{}", state.port);
    TcpStream::connect_timeout(&addr.parse().ok()?, Duration::from_millis(300)).ok()?;
    Some(DaemonConnection {
        port: state.port,
        token: state.token,
    })
}

fn spawn_daemon() -> Result<(), String> {
    if crate::single_instance::gui_is_running() {
        return Err(crate::single_instance::already_running_message());
    }
    if read_daemon_connection().is_some() {
        return Ok(());
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--playback-daemon");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    #[cfg(unix)]
    {
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to spawn playback daemon: {e}"))?;

    for _ in 0..100 {
        if read_daemon_connection().is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Playback daemon failed to start within 10 seconds.".to_string())
}

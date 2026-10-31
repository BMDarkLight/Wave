// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Playing on another paired Wave.
//!
//! The Wave that casts keeps its queue and stays the remote control: its
//! player hands each track to a [`CastSender`] instead of its own output.
//! The sender tells the other Wave what to play and asks every second how
//! it is going, which is what the casting Wave's player then reports.
//!
//! The other Wave plays one track at a time, as it would any of its own:
//! one of its own songs from its library, a song it already has a copy of
//! (by fingerprint), or else the sender's file, streamed and cached like any
//! remote song but kept out of its library.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use super::api::{PeerService, SharedTrack};
use super::client::{self, CallError};
use super::PeerRecord;
use crate::audio::cast::{CastOutput, CastStatus};
use crate::remote::uri::RemoteUri;

/// Play this, from here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastPlay {
    /// Counts up with every track sent, so a report about an older one is
    /// not taken for news about this one.
    pub session: u64,
    pub item: CastItem,
    pub position: f64,
    pub playing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum CastItem {
    /// A song of the Wave that sends it, streamed from there.
    Sender { track: Box<SharedTrack> },
    /// One of the receiving Wave's own songs, by its track id.
    Receiver { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CastControl {
    Pause,
    Resume,
    Seek { position: f64 },
    Volume { volume: f32 },
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastControlBody {
    pub session: u64,
    #[serde(flatten)]
    pub control: CastControl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CastPhase {
    /// Nothing from this sender is playing.
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    /// Someone at the other device played something else.
    Taken,
    /// The track could not be played there.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastState {
    pub session: u64,
    pub phase: CastPhase,
    pub position: f64,
    pub duration: Option<f64>,
}

// ── Sending ─────────────────────────────────────────────────────────────────

const POLL_EVERY: Duration = Duration::from_secs(1);
/// No answer for this long and the other device counts as gone.
const LOST_AFTER: Duration = Duration::from_secs(15);
/// The other device answers, but never about the track it was sent.
const FORGOT_AFTER: Duration = Duration::from_secs(10);

/// What the casting side knows of the other device's playback.
#[derive(Debug, Clone)]
struct Heard {
    session: u64,
    position: f64,
    /// When `position` was true.
    at: Instant,
    playing: bool,
    paused: bool,
    ended: bool,
    buffering: bool,
    duration: Option<f64>,
    answered_at: Instant,
    /// Since when it answers about some other session.
    other_session_since: Option<Instant>,
    lost: Option<String>,
}

impl Heard {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            session: 0,
            position: 0.0,
            at: now,
            playing: false,
            paused: false,
            ended: false,
            buffering: false,
            duration: None,
            answered_at: now,
            other_session_since: None,
            lost: None,
        }
    }

    fn status(&self) -> CastStatus {
        let mut position = self.position;
        if self.playing && !self.buffering {
            position += self.at.elapsed().as_secs_f64();
        }
        if let Some(duration) = self.duration {
            position = position.min(duration);
        }
        CastStatus {
            position,
            duration: self.duration,
            playing: self.playing,
            paused: self.paused,
            ended: self.ended,
            buffering: self.buffering,
        }
    }

    fn sent(&mut self, position: f64, playing: bool, duration: Option<f64>) -> u64 {
        self.session += 1;
        self.position = position;
        self.at = Instant::now();
        self.playing = playing;
        self.paused = !playing;
        self.ended = false;
        self.buffering = true;
        self.duration = duration;
        self.other_session_since = None;
        self.session
    }

    /// Takes in a report from the other device.
    fn hear(&mut self, state: &CastState, name: &str) {
        let now = Instant::now();
        self.answered_at = now;
        if state.session != self.session {
            let since = *self.other_session_since.get_or_insert(now);
            if self.session > 0 && since.elapsed() >= FORGOT_AFTER {
                self.lost = Some(format!("{name} stopped playing"));
            }
            return;
        }
        self.other_session_since = None;
        if state.duration.is_some() {
            self.duration = state.duration;
        }
        match state.phase {
            CastPhase::Loading => self.buffering = true,
            CastPhase::Playing | CastPhase::Paused => {
                let playing = state.phase == CastPhase::Playing;
                self.buffering = false;
                self.playing = playing;
                self.paused = !playing;
                self.ended = false;
                self.position = state.position;
                self.at = now;
            }
            // A track that can't play there is skipped, like an unreadable
            // file here.
            CastPhase::Ended | CastPhase::Failed => {
                self.buffering = false;
                self.playing = false;
                self.paused = false;
                self.ended = true;
            }
            CastPhase::Taken => {
                self.lost = Some(format!("{name} is playing something else now"));
            }
            CastPhase::Idle => {}
        }
    }

    fn check_reachable(&mut self, name: &str) {
        if self.lost.is_none() && self.answered_at.elapsed() >= LOST_AFTER {
            self.lost = Some(format!("{name} can't be reached"));
        }
    }
}

enum Command {
    Play(Box<CastPlay>),
    Control(CastControl),
    Stop,
}

/// Plays on a paired Wave in place of this device's output.
pub struct CastSender {
    peer_id: String,
    name: String,
    service: Arc<PeerService>,
    heard: Arc<Mutex<Heard>>,
    commands: Sender<Command>,
}

impl CastSender {
    pub fn start(service: Arc<PeerService>, peer: &PeerRecord) -> Self {
        let heard = Arc::new(Mutex::new(Heard::new()));
        let (commands, inbox) = mpsc::channel();
        let worker_heard = heard.clone();
        let peer_id = peer.id.clone();
        let name = peer.name.clone();
        let spawned = std::thread::Builder::new()
            .name("wave-cast".into())
            .spawn(move || run(&peer_id, &name, &worker_heard, &inbox));
        if let Err(e) = spawned {
            heard.lock().lost = Some(format!("Couldn't play on {}: {e}", peer.name));
        }
        Self {
            peer_id: peer.id.clone(),
            name: peer.name.clone(),
            service,
            heard,
            commands,
        }
    }

    fn item_for(&self, path: &str) -> Result<CastItem, String> {
        let theirs = crate::remote::commands::wave_source_id(&self.peer_id);
        if let Some(uri) = RemoteUri::parse(path).filter(|uri| uri.source_id == theirs) {
            return Ok(CastItem::Receiver { id: uri.remote_id });
        }
        let Some((track, fingerprint)) = crate::share::shared_track_at(path) else {
            return Err(format!(
                "This song isn't stored on this device, so it can't play on {}",
                self.name
            ));
        };
        self.service.grant_cast(&self.peer_id, &track.id);
        Ok(CastItem::Sender {
            track: Box::new(SharedTrack::of(&track, fingerprint)),
        })
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl CastOutput for CastSender {
    fn device(&self) -> (String, String) {
        (self.peer_id.clone(), self.name.clone())
    }

    fn load(&mut self, path: &str, position: f64, playing: bool) -> Result<(), String> {
        let item = self.item_for(path)?;
        let duration = match &item {
            CastItem::Sender { track } => track.duration,
            CastItem::Receiver { .. } => None,
        };
        let session = self.heard.lock().sent(position, playing, duration);
        self.send(Command::Play(Box::new(CastPlay {
            session,
            item,
            position,
            playing,
        })));
        Ok(())
    }

    fn pause(&mut self) {
        {
            let mut heard = self.heard.lock();
            heard.position = heard.status().position;
            heard.at = Instant::now();
            heard.playing = false;
            heard.paused = true;
        }
        self.send(Command::Control(CastControl::Pause));
    }

    fn resume(&mut self) {
        {
            let mut heard = self.heard.lock();
            heard.at = Instant::now();
            heard.playing = true;
            heard.paused = false;
        }
        self.send(Command::Control(CastControl::Resume));
    }

    fn seek(&mut self, seconds: f64) {
        {
            let mut heard = self.heard.lock();
            heard.position = seconds;
            heard.at = Instant::now();
            heard.ended = false;
        }
        self.send(Command::Control(CastControl::Seek { position: seconds }));
    }

    fn set_volume(&mut self, volume: f32) {
        self.send(Command::Control(CastControl::Volume { volume }));
    }

    fn stop(&mut self) {
        self.send(Command::Stop);
    }

    fn status(&self) -> CastStatus {
        self.heard.lock().status()
    }

    fn lost(&self) -> Option<String> {
        self.heard.lock().lost.clone()
    }
}

/// Sends what the player asks for, in order, and asks how it is going.
fn run(peer_id: &str, name: &str, heard: &Mutex<Heard>, inbox: &Receiver<Command>) {
    let mut last_poll: Option<Instant> = None;
    loop {
        let command = match inbox.recv_timeout(POLL_EVERY) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Some((peer, own_id)) = super::lookup(peer_id) else {
            heard.lock().lost = Some(format!("{name} is no longer paired"));
            return;
        };
        let session = heard.lock().session;
        let sent = match command {
            Some(Command::Play(play)) => Some(client::cast_play(&peer, &own_id, &play)),
            Some(Command::Control(control)) => Some(client::cast_control(
                &peer,
                &own_id,
                &CastControlBody { session, control },
            )),
            Some(Command::Stop) => {
                let stop = CastControlBody {
                    session,
                    control: CastControl::Stop,
                };
                let _ = client::cast_control(&peer, &own_id, &stop);
                return;
            }
            None => None,
        };
        match sent {
            Some(Err(CallError::Forgotten)) => {
                heard.lock().lost = Some(format!("{name} no longer knows this Wave"));
            }
            // It got the request and can't do it: move on to the next track.
            Some(Err(CallError::Refused(_))) => {
                let mut heard = heard.lock();
                heard.ended = true;
                heard.playing = false;
                heard.buffering = false;
            }
            _ => {}
        }
        if last_poll.is_none_or(|at| at.elapsed() >= POLL_EVERY) {
            last_poll = Some(Instant::now());
            let state = client::cast_state(&peer, &own_id);
            let mut heard = heard.lock();
            match state {
                Ok(state) => heard.hear(&state, name),
                Err(CallError::Forgotten) => {
                    heard.lost = Some(format!("{name} no longer knows this Wave"));
                }
                Err(_) => heard.check_reachable(name),
            }
        }
    }
}

// ── Receiving ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Loading,
    Started,
    Failed,
}

/// What a paired Wave is playing here.
#[derive(Debug, Clone)]
struct Receiving {
    from_id: String,
    from_name: String,
    session: u64,
    stage: Stage,
    path: Option<String>,
}

fn receiving() -> &'static Mutex<Option<Receiving>> {
    static RECEIVING: Mutex<Option<Receiving>> = Mutex::new(None);
    &RECEIVING
}

/// How the cast is going, from what the player here is doing.
fn phase_of(
    receiving: &Receiving,
    current: Option<&str>,
    playing: bool,
    paused: bool,
) -> CastPhase {
    match receiving.stage {
        Stage::Loading => return CastPhase::Loading,
        Stage::Failed => return CastPhase::Failed,
        Stage::Started => {}
    }
    let Some(path) = receiving.path.as_deref() else {
        return CastPhase::Loading;
    };
    match current {
        Some(current) if current != path => CastPhase::Taken,
        _ if playing => CastPhase::Playing,
        Some(_) if paused => CastPhase::Paused,
        _ => CastPhase::Ended,
    }
}

/// The repeat mode this Wave had before it was played to. Repeat is off
/// meanwhile: each track has to end for the sender to send the next one.
fn held_repeat() -> &'static Mutex<Option<crate::audio::player::RepeatMode>> {
    static HELD: Mutex<Option<crate::audio::player::RepeatMode>> = Mutex::new(None);
    &HELD
}

/// Puts back the repeat mode a cast turned off, unless it was changed since.
fn release_repeat(app: &AppHandle) {
    use crate::audio::player::RepeatMode;
    let Some(before) = held_repeat().lock().take() else {
        return;
    };
    let _ = crate::commands::with_app_player(app, |player| {
        if player.repeat == RepeatMode::Off {
            player.set_repeat(before);
        }
        Ok(())
    });
}

fn changed(app: &AppHandle) {
    let _ = app.emit("wave-cast-changed", ());
}

pub fn receive_play(app: &AppHandle, from: &PeerRecord, play: CastPlay) -> Result<(), String> {
    *receiving().lock() = Some(Receiving {
        from_id: from.id.clone(),
        from_name: from.name.clone(),
        session: play.session,
        stage: Stage::Loading,
        path: None,
    });
    let app = app.clone();
    let from = from.clone();
    std::thread::Builder::new()
        .name("wave-cast-play".into())
        .spawn(move || {
            let played = resolve(&app, &from, &play.item).and_then(|path| {
                start(&app, &path, &play)?;
                Ok(path)
            });
            {
                let mut receiving = receiving().lock();
                let Some(current) = receiving
                    .as_mut()
                    .filter(|r| r.from_id == from.id && r.session == play.session)
                else {
                    return;
                };
                match &played {
                    Ok(path) => {
                        current.path = Some(path.clone());
                        current.stage = Stage::Started;
                    }
                    Err(e) => {
                        tracing::warn!("Couldn't play what {} sent: {e}", from.name);
                        current.stage = Stage::Failed;
                    }
                }
            }
            changed(&app);
            if let Ok(path) = played {
                if fetch_cover(&path) {
                    changed(&app);
                }
            }
        })
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The path that plays `item` here.
fn resolve(app: &AppHandle, from: &PeerRecord, item: &CastItem) -> Result<String, String> {
    let library = app
        .try_state::<crate::commands::LibraryState>()
        .ok_or("The library isn't open")?;
    match item {
        CastItem::Receiver { id } => library
            .0
            .lock()
            .map_err(|_| "The library is busy")?
            .get_track_by_id(id)?
            .map(|track| track.path)
            .ok_or_else(|| "That song isn't on this device anymore".to_string()),
        CastItem::Sender { track } => {
            if let Some(print) = &track.fingerprint {
                let library = library.0.lock().map_err(|_| "The library is busy")?;
                let own = library.local_fingerprints()?.get(print).cloned();
                if let Some(own) = own.and_then(|id| library.get_track_by_id(&id).ok().flatten()) {
                    return Ok(own.path);
                }
            }
            let rt = crate::remote::RemoteRuntime::global().ok_or("Remote music isn't set up")?;
            let source = crate::remote::commands::wave_source_id(&from.id);
            let meta = crate::remote::wave::to_meta((**track).clone());
            let stored = rt.with_library(|l| l.cast_track_path(&source, "wave", &meta));
            match stored {
                Ok(path) if rt.backend(&source).is_some() => Ok(path),
                // Paired before this Wave read from paired devices: set the
                // source up now.
                _ => {
                    crate::remote::commands::add_wave_source(app, from);
                    rt.with_library(|l| l.cast_track_path(&source, "wave", &meta))
                }
            }
        }
    }
}

/// Plays `path` here as a queue of its own, from where the sender was.
fn start(app: &AppHandle, path: &str, play: &CastPlay) -> Result<(), String> {
    use crate::commands;
    // Playing to a third device stops when this one is played to.
    let _ = commands::with_app_player(app, |player| {
        let mut held = held_repeat().lock();
        if held.is_none() {
            *held = Some(player.repeat.clone());
        }
        player.set_repeat(crate::audio::player::RepeatMode::Off);
        player.end_cast(false).map_err(|e| e.to_string())
    });
    tauri::async_runtime::block_on(commands::start_queue(
        vec![path.to_string()],
        0,
        false,
        app.clone(),
    ))?;
    if play.position > 1.0 {
        commands::control_playback(app, &format!("seek:{}", play.position))?;
    }
    if !play.playing {
        commands::control_playback(app, "pause")?;
    }
    Ok(())
}

/// Fetches the cover of a song streamed from the sender, when it has none
/// yet. Returns whether one was stored.
fn fetch_cover(path: &str) -> bool {
    let Some(uri) = RemoteUri::parse(path) else {
        return false;
    };
    let Some(rt) = crate::remote::RemoteRuntime::global() else {
        return false;
    };
    let Some((Some(art_ref), None)) = rt.with_library(|l| l.remote_track_art(&uri)) else {
        return false;
    };
    let Some(image) = rt
        .backend(&uri.source_id)
        .and_then(|backend| backend.fetch_artwork(&art_ref).ok())
    else {
        return false;
    };
    rt.with_library(|l| l.set_remote_track_art(&uri, &image))
        .unwrap_or(false)
}

/// The cast `from` sent, when `session` is still the one playing.
fn current_from(from: &PeerRecord, session: u64) -> Option<Receiving> {
    receiving()
        .lock()
        .clone()
        .filter(|r| r.from_id == from.id && r.session == session)
}

pub fn receive_control(
    app: &AppHandle,
    from: &PeerRecord,
    body: CastControlBody,
) -> Result<(), String> {
    use crate::commands;
    if current_from(from, body.session).is_none() {
        return Err("Nothing of yours is playing here".to_string());
    }
    match body.control {
        CastControl::Pause => commands::control_playback(app, "pause"),
        CastControl::Resume => commands::control_playback(app, "play"),
        CastControl::Seek { position } => {
            commands::control_playback(app, &format!("seek:{position}"))
        }
        // Android plays at the phone's own volume.
        CastControl::Volume { volume } => {
            if cfg!(target_os = "android") {
                return Ok(());
            }
            commands::with_app_player(app, |player| {
                player
                    .set_volume(volume.clamp(0.0, 1.0))
                    .map_err(|e| e.to_string())
            })
        }
        CastControl::Stop => {
            *receiving().lock() = None;
            release_repeat(app);
            commands::control_playback(app, "pause")
        }
    }?;
    changed(app);
    Ok(())
}

pub fn receive_state(app: &AppHandle, from: &PeerRecord) -> CastState {
    let Some(current) = receiving().lock().clone().filter(|r| r.from_id == from.id) else {
        return CastState {
            session: 0,
            phase: CastPhase::Idle,
            position: 0.0,
            duration: None,
        };
    };
    let now = crate::commands::with_app_player(app, |player| {
        Ok((
            player
                .get_current_path()
                .and_then(|p| p.to_str())
                .map(str::to_string),
            player.is_playing(),
            player.is_paused(),
            player.position_seconds(),
            player.duration_seconds(),
        ))
    });
    let (path, playing, paused, position, duration) =
        now.unwrap_or((None, false, false, 0.0, None));
    let phase = phase_of(&current, path.as_deref(), playing, paused);
    if phase == CastPhase::Taken {
        *receiving().lock() = None;
        release_repeat(app);
    }
    CastState {
        session: current.session,
        phase,
        position,
        duration,
    }
}

/// The paired Wave playing `current_path` here, when one is.
pub fn playing_from(current_path: Option<&str>) -> Option<String> {
    let receiving = receiving().lock();
    let current = receiving.as_ref()?;
    (current.stage == Stage::Started && current.path.as_deref() == current_path)
        .then(|| current.from_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(session: u64, phase: CastPhase, position: f64) -> CastState {
        CastState {
            session,
            phase,
            position,
            duration: Some(200.0),
        }
    }

    #[test]
    fn reports_about_an_older_track_are_ignored() {
        let mut heard = Heard::new();
        let first = heard.sent(0.0, true, Some(180.0));
        let second = heard.sent(0.0, true, Some(180.0));
        assert_eq!((first, second), (1, 2));
        heard.hear(&state(1, CastPhase::Ended, 179.0), "Phone");
        assert!(!heard.ended);
        assert!(heard.buffering);
        heard.hear(&state(2, CastPhase::Playing, 3.0), "Phone");
        assert!(heard.playing && !heard.buffering);
        assert_eq!(heard.duration, Some(200.0));
        let position = heard.status().position;
        assert!((3.0..3.5).contains(&position), "{position}");
        heard.hear(&state(2, CastPhase::Ended, 200.0), "Phone");
        assert!(heard.status().ended);
        assert!(heard.lost.is_none());
    }

    #[test]
    fn a_device_that_plays_something_else_or_goes_quiet_is_lost() {
        let mut heard = Heard::new();
        heard.sent(0.0, true, None);
        heard.hear(&state(1, CastPhase::Taken, 0.0), "Phone");
        assert_eq!(
            heard.lost.as_deref(),
            Some("Phone is playing something else now")
        );

        let mut heard = Heard::new();
        heard.sent(0.0, true, None);
        heard.answered_at = Instant::now() - LOST_AFTER;
        heard.check_reachable("Phone");
        assert_eq!(heard.lost.as_deref(), Some("Phone can't be reached"));

        let mut heard = Heard::new();
        heard.sent(0.0, true, None);
        heard.other_session_since = Some(Instant::now() - FORGOT_AFTER);
        heard.hear(&state(0, CastPhase::Idle, 0.0), "Phone");
        assert_eq!(heard.lost.as_deref(), Some("Phone stopped playing"));
    }

    #[test]
    fn a_failed_track_moves_on() {
        let mut heard = Heard::new();
        heard.sent(0.0, true, None);
        heard.hear(&state(1, CastPhase::Failed, 0.0), "Phone");
        assert!(heard.status().ended);
    }

    #[test]
    fn the_phase_follows_the_player_here() {
        let started = Receiving {
            from_id: "pc".into(),
            from_name: "Wave on pc".into(),
            session: 4,
            stage: Stage::Started,
            path: Some("/music/a.mp3".into()),
        };
        let phase =
            |current: Option<&str>, playing, paused| phase_of(&started, current, playing, paused);
        assert_eq!(phase(Some("/music/a.mp3"), true, false), CastPhase::Playing);
        assert_eq!(phase(Some("/music/a.mp3"), false, true), CastPhase::Paused);
        assert_eq!(phase(Some("/music/a.mp3"), false, false), CastPhase::Ended);
        assert_eq!(phase(None, false, false), CastPhase::Ended);
        assert_eq!(phase(Some("/music/b.mp3"), true, false), CastPhase::Taken);
        let loading = Receiving {
            stage: Stage::Loading,
            ..started.clone()
        };
        assert_eq!(phase_of(&loading, None, false, false), CastPhase::Loading);
        let failed = Receiving {
            stage: Stage::Failed,
            ..started
        };
        assert_eq!(phase_of(&failed, None, false, false), CastPhase::Failed);
    }

    #[test]
    fn requests_read_back_as_sent() {
        let play = CastPlay {
            session: 3,
            item: CastItem::Receiver { id: "t1".into() },
            position: 12.5,
            playing: true,
        };
        let json = serde_json::to_value(&play).unwrap();
        assert_eq!(json["item"]["origin"], "receiver");
        assert_eq!(serde_json::from_value::<CastPlay>(json).unwrap(), play);
        let seek = CastControlBody {
            session: 3,
            control: CastControl::Seek { position: 40.0 },
        };
        let json = serde_json::to_value(&seek).unwrap();
        assert_eq!(json["action"], "seek");
        assert_eq!(
            serde_json::from_value::<CastControlBody>(json).unwrap(),
            seek
        );
    }
}

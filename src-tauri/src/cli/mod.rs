// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

pub mod banner;
pub mod bar;
pub mod cmd;
pub mod json;
pub mod now;
pub mod render;
pub mod table;
pub mod ui;

use std::io::IsTerminal;
use std::path::Path;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::library::Library;
use crate::playback_daemon::{daemon_request_if_running, DaemonRequest};
use crate::tag_edit::TagEdit;

// ── Top-level CLI ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "wave", version, about = "Lightweight Music Player CLI")]
pub struct Cli {
    /// Open the command line landing screen instead of the app
    #[arg(long, global = true)]
    pub cli: bool,

    /// Same as --cli
    #[arg(long, global = true)]
    pub headless: bool,

    /// Skip the Wave mark on the landing screen
    #[arg(long, global = true)]
    pub no_banner: bool,

    /// When to colour the output
    #[arg(long, global = true, value_enum, default_value_t = ui::ColorChoice::Auto)]
    pub color: ui::ColorChoice,

    /// Print machine-readable JSON instead of formatted text
    #[arg(long, global = true)]
    pub json: bool,

    /// Include per-item detail, such as why a file was skipped on import
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Play a track or playlist: id, short id, file path, or playlist name
    Play {
        /// Track id or path, or playlist id or name
        id: String,
    },
    /// Manage tracks in the library
    #[command(subcommand)]
    Tracks(TracksCmd),
    /// Manage playlists
    #[command(subcommand, visible_alias = "playlist")]
    Playlists(PlaylistsCmd),
    /// Control audio playback
    #[command(subcommand)]
    Playback(PlaybackCmd),
    /// Manage the playback queue
    #[command(subcommand)]
    Queue(QueueCmd),
    /// List and switch audio output devices
    #[command(subcommand)]
    Devices(DevicesCmd),
    /// Manage favorite tracks
    #[command(subcommand)]
    Favorite(FavoriteCmd),
    /// Inspect and manipulate track metadata
    #[command(subcommand)]
    Metadata(MetadataCmd),
    /// DSP / equalizer, gapless, and crossfade controls
    #[command(subcommand)]
    Dsp(DspCmd),
    /// Print a shell completion script
    Completions {
        /// Shell to generate for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Live now-playing dashboard
    Now {
        /// Print one frame and exit instead of redrawing
        #[arg(long)]
        once: bool,
        /// Seconds between redraws (0.1 to 10)
        #[arg(long, default_value_t = 0.5)]
        interval: f64,
    },
    /// Listening stats (play time, top tracks / artists / albums / genres)
    #[command(subcommand, visible_alias = "listen")]
    Stats(StatsCmd),
}

// ── Subcommand structs ───────────────────────────────────────────────────────

#[derive(clap::Subcommand)]
pub enum TracksCmd {
    /// List all tracks, or tracks in a playlist
    List {
        /// Optional playlist ID to filter tracks
        playlist_id: Option<String>,
    },
    /// Import audio file(s) or a directory into the library
    Import {
        /// One or more file or directory paths
        paths: Vec<String>,
    },
    /// Show detailed metadata for a track
    Info {
        /// Track ID (UUID) or file path
        track_id: String,
    },
    /// Search tracks by title, artist, or album
    Query {
        /// Search query string
        query: String,
    },
    /// Add a track to a playlist (Library if --playlist-id is omitted)
    Add {
        /// Track ID (UUID) or file path
        track_id: String,
        /// Playlist ID (defaults to Library)
        #[arg(long)]
        playlist_id: Option<String>,
    },
    /// Remove a track from a playlist, or from the Library if --playlist-id is omitted
    Remove {
        /// Track ID (UUID) or file path
        track_id: String,
        /// Playlist ID. Omit to remove the track from the Library entirely.
        #[arg(long)]
        playlist_id: Option<String>,
    },
    /// Delete every track and all playlists except Library and Favorites
    Reset {
        /// Skip the interactive confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum PlaylistsCmd {
    /// List all playlists
    List,
    /// Import a playlist from a file (M3U or Wave JSON)
    Import {
        /// Path to the playlist file
        file: String,
        /// Optional name for the imported playlist
        name: Option<String>,
    },
    /// Export a playlist to a file
    Export {
        /// Playlist ID
        id: String,
        /// Export format (m3u or json)
        format: String,
        /// Output file path
        output: String,
    },
    /// Show playlist info and its track IDs
    Info {
        /// Playlist ID
        id: String,
    },
    /// Search playlists by name
    Query {
        /// Search query string
        query: String,
    },
    /// Create an empty playlist
    Create {
        /// Playlist name
        name: String,
    },
    /// Delete a playlist
    Delete {
        /// Playlist ID
        id: String,
    },
    /// Rename a playlist
    Rename {
        /// Playlist ID
        id: String,
        /// New name
        name: String,
    },
    /// Remove all tracks from a playlist (blocked for synced playlists)
    Clear {
        /// Playlist ID
        id: String,
    },
    /// Add a track to a playlist
    AddTrack {
        /// Playlist ID
        id: String,
        /// Track ID (UUID) or file path
        track_id: String,
    },
    /// Remove a track from a playlist (does not delete it from the Library)
    RemoveTrack {
        /// Playlist ID
        id: String,
        /// Track ID (UUID) or file path
        track_id: String,
    },
    /// Sync a folder-linked playlist with its folder on disk
    Sync {
        /// Playlist ID (UUID)
        id: String,
    },
}

#[derive(clap::Subcommand)]
pub enum PlaybackCmd {
    /// Start playing a track or playlist
    Start {
        /// Track path/ID or playlist ID
        id: String,
    },
    /// Pause playback
    Pause,
    /// Resume playback
    Resume,
    /// Stop playback
    Stop,
    /// Skip to the next track
    Next,
    /// Go back to the previous track
    Previous,
    /// Seek to a position (in seconds)
    Seek {
        /// Position in seconds
        seconds: f64,
    },
    /// Show current playback status
    Status,
    /// Shut down the background playback daemon
    Shutdown,
}

#[derive(clap::Subcommand)]
pub enum QueueCmd {
    /// List the current queue
    List,
    /// Add a track to the end of the queue
    Add {
        /// Track file path
        track_id: String,
    },
    /// Remove a track from the queue by index
    Remove {
        /// Queue index (0-based)
        index: usize,
    },
    /// Insert a track to play next
    Next {
        /// Track file path
        track_id: String,
    },
    /// Toggle or set shuffle mode
    Shuffle {
        /// on, off, or omit to toggle
        state: Option<String>,
    },
    /// Set repeat mode
    Repeat {
        /// off, one, or all
        mode: String,
    },
    /// Clear the queue (keeps current track)
    Clear,
}

#[derive(clap::Subcommand)]
pub enum DevicesCmd {
    /// List available audio output devices
    List,
    /// Switch to a different audio output device
    Switch {
        /// Device name
        name: String,
    },
    /// Set playback volume (0.0 to 1.0)
    Volume {
        /// Volume level (0.0–1.0)
        level: f32,
    },
}

#[derive(clap::Subcommand)]
pub enum FavoriteCmd {
    /// Add a track to favorites
    Add {
        /// Track file path
        track_id: String,
    },
    /// Remove a track from favorites
    Remove {
        /// Track file path
        track_id: String,
    },
    /// List all favorite tracks
    List,
    /// Clear all favorites
    Clear,
}

#[derive(clap::Subcommand)]
pub enum DspCmd {
    /// Show current EQ, gapless, and crossfade settings
    EqShow,
    /// Set EQ band gains (10 values in dB, one per ISO band)
    EqSet {
        /// 10 gain values in dB for bands 31, 62, 125, 250, 500,
        /// 1000, 2000, 4000, 8000, 16000 Hz
        #[arg(allow_hyphen_values = true)]
        bands: Vec<f32>,
    },
    /// Set one EQ band and leave the others as they are
    EqBand {
        /// Band number 1-10, or its frequency (e.g. 125, 1k, 16kHz)
        #[arg(value_hint = clap::ValueHint::Other)]
        band: String,
        /// Gain in dB (-12 to +12)
        #[arg(allow_hyphen_values = true)]
        db: f32,
    },
    /// Enable the equalizer
    EqEnable,
    /// Disable the equalizer
    EqDisable,
    /// Reset all bands to 0 dB and enable EQ
    EqReset,
    /// Apply a named EQ preset
    Preset {
        /// Preset name
        #[arg(value_hint = clap::ValueHint::Other)]
        name: String,
    },
    /// List available EQ presets
    Presets,
    /// Export current EQ settings to a JSON file
    Export {
        /// Output file path (e.g. my-eq.json)
        output: String,
        /// Optional name for the preset
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Import EQ settings from a JSON file
    Import {
        /// Input file path (e.g. my-eq.json)
        input: String,
    },
    /// Show or set gapless playback (seamless queue transitions)
    Gapless {
        /// `on` / `off`, or omit to show the current value
        state: Option<String>,
    },
    /// Show or set crossfade duration in seconds (0 = off, max 8)
    Crossfade {
        /// Duration in seconds, or omit to show the current value
        seconds: Option<f32>,
    },
    /// Show or set the bass dial (−12…+12 dB); rebuilds the 10-band curve
    Bass {
        /// Gain in dB, or omit to show the current bass dial
        #[arg(allow_hyphen_values = true)]
        db: Option<f32>,
    },
    /// Show or set the treble dial (−12…+12 dB); rebuilds the 10-band curve
    Treble {
        /// Gain in dB, or omit to show the current treble dial
        #[arg(allow_hyphen_values = true)]
        db: Option<f32>,
    },
}

#[derive(clap::Subcommand)]
pub enum StatsCmd {
    /// Overview: totals plus top tracks, artists, albums, and genres
    Summary {
        /// How many entries per top list (1–20, default 5)
        #[arg(short, long, default_value_t = 5)]
        limit: u32,
    },
    /// Recently played tracks
    Recent {
        /// Max tracks to show (default 25)
        #[arg(short, long, default_value_t = 25)]
        limit: u32,
    },
    /// Most played tracks by listen time
    Most {
        /// Max tracks to show (default 25)
        #[arg(short, long, default_value_t = 25)]
        limit: u32,
    },
    /// Top artists by listen time
    Artists {
        /// Max artists to show (default 10)
        #[arg(short, long, default_value_t = 10)]
        limit: u32,
    },
    /// Top albums by listen time
    Albums {
        /// Max albums to show (default 10)
        #[arg(short, long, default_value_t = 10)]
        limit: u32,
    },
    /// Top genres by listen time
    Genres {
        /// Max genres to show (default 10)
        #[arg(short, long, default_value_t = 10)]
        limit: u32,
    },
}

#[derive(clap::Subcommand)]
pub enum MetadataCmd {
    /// Show full metadata for a track
    Get {
        /// Track ID (UUID) or file path
        track_id: String,
    },
    /// Export a track's album cover art to an image file
    CoverExport {
        /// Track ID (UUID) or file path
        track_id: String,
        /// Output image file path (e.g. cover.jpg)
        output: String,
    },
    /// Set a track's album cover from an image file
    CoverSet {
        /// Track ID (UUID) or file path
        track_id: String,
        /// Image file path (e.g. cover.jpg)
        image: String,
    },
    /// Write tag values back to a track's file
    ///
    /// Only the fields you pass are touched. Pass an empty string to clear
    /// one, for example --genre "".
    Set {
        /// Track ID (UUID) or file path
        track_id: String,
        #[command(flatten)]
        fields: TagFields,
    },
}

#[derive(clap::Args)]
pub struct TagFields {
    /// Track title
    #[arg(long)]
    title: Option<String>,
    /// Track artist
    #[arg(long)]
    artist: Option<String>,
    /// Album name
    #[arg(long)]
    album: Option<String>,
    /// Album artist
    #[arg(long)]
    album_artist: Option<String>,
    /// Genre
    #[arg(long)]
    genre: Option<String>,
    /// Release year (1-9999)
    #[arg(long)]
    year: Option<String>,
    /// Track number (1-9999)
    #[arg(long)]
    track_number: Option<String>,
    /// Disc number (1-9999)
    #[arg(long)]
    disc_number: Option<String>,
}

impl From<TagFields> for TagEdit {
    fn from(fields: TagFields) -> Self {
        Self {
            title: fields.title,
            artist: fields.artist,
            album: fields.album,
            album_artist: fields.album_artist,
            genre: fields.genre,
            year: fields.year,
            track_number: fields.track_number,
            disc_number: fields.disc_number,
            cover: None,
        }
    }
}

// ── Library path resolution ─────────────────────────────────────────────────

/// Return the default database path for CLI mode.
fn default_db_path() -> std::path::PathBuf {
    crate::app_paths::library_db_path()
}

// ── Track resolution helpers ────────────────────────────────────────────────

/// Resolve what the user typed to a file path.
///
/// Accepts a full UUID, a file path, or an unambiguous id prefix. The prefix
/// needs at least four characters, below which almost anything in a real
/// library is ambiguous and the error is more useful than a guess.
fn resolve_track_path(library: &Library, id_or_path: &str) -> Result<String, String> {
    const MIN_PREFIX: usize = 4;

    if uuid::Uuid::parse_str(id_or_path).is_ok() {
        if let Some(track) = library.get_track_by_id(id_or_path)? {
            return Ok(track.path);
        }
    }

    if Path::new(id_or_path).exists() {
        return Ok(library_path_spelling(id_or_path));
    }

    let looks_like_id = id_or_path.len() >= MIN_PREFIX
        && id_or_path
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-');
    if looks_like_id {
        let hits = library.find_tracks_by_id_prefix(id_or_path)?;
        match hits.as_slice() {
            [only] => return Ok(only.path.clone()),
            [] => {}
            many => {
                let candidates: Vec<String> = many
                    .iter()
                    .take(5)
                    .map(|t| format!("{} {} - {}", render::short_id(&t.id), t.artist, t.title))
                    .collect();
                // The query is capped, so a full page means there may be more.
                let count = if many.len() >= 10 {
                    "10 or more".to_string()
                } else {
                    many.len().to_string()
                };
                ui::fail(
                    format!("Track id \"{id_or_path}\" is ambiguous."),
                    Some(&format!(
                        "{count} tracks match. Narrow it: {}",
                        candidates.join("; ")
                    )),
                    ui::EXIT_NOT_FOUND,
                );
            }
        }
    }

    Err(format!(
        "Track not found: {id_or_path} (not a track id, id prefix, or existing file path)"
    ))
}

/// Resolve a track or exit with a hint pointing at `tracks query`.
pub(crate) fn track_path_or_exit(library: &Library, id_or_path: &str) -> String {
    resolve_track_path(library, id_or_path).unwrap_or_else(|e| {
        ui::fail(
            e,
            Some(&format!(
                "list candidates with: wave tracks query \"{id_or_path}\""
            )),
            ui::EXIT_NOT_FOUND,
        )
    })
}

/// The library stores whichever spelling a track was imported with. Canonicalize
/// what the user typed so an equivalent path written differently (forward
/// slashes, a relative segment, another case) still lands on the same row.
fn library_path_spelling(path: &str) -> String {
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return path.to_string();
    };
    let text = canonical.to_string_lossy();
    // Windows canonicalization returns an extended-length path; nothing else
    // in Wave writes that prefix, so drop it again.
    text.strip_prefix("\\\\?\\")
        .unwrap_or(text.as_ref())
        .to_string()
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// True when argv targets an existing playback daemon (ephemeral CLI client).
pub fn is_daemon_ipc_client(args: &[String]) -> bool {
    if !crate::playback_daemon::daemon_is_running() {
        return false;
    }

    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(_) => return false,
    };

    match cli.command {
        Some(Commands::Playback(_)) => true,
        Some(Commands::Play { .. }) => true,
        Some(Commands::Queue(_)) => true,
        Some(Commands::Now { .. }) => true,
        Some(Commands::Devices(DevicesCmd::Volume { .. } | DevicesCmd::Switch { .. })) => true,
        // Live DSP when the CLI playback daemon owns the audio engine.
        Some(Commands::Dsp(ref cmd)) => !matches!(cmd, DspCmd::Presets),
        _ => false,
    }
}

/// Commands that spawn or control the CLI playback daemon while the GUI is open.
pub fn conflicts_with_gui(args: &[String]) -> bool {
    let cli = match Cli::try_parse_from(args) {
        Ok(c) => c,
        Err(_) => return false,
    };

    matches!(
        cli.command,
        Some(Commands::Playback(_))
            | Some(Commands::Play { .. })
            | Some(Commands::Queue(_))
            | Some(Commands::Devices(_))
            | Some(Commands::Now { .. })
    )
}

pub fn run() {
    let ui = ui::Ui::resolve(ui::ColorChoice::Auto, false, false);

    // The mark heads the top-level help, but only on a terminal: help piped
    // into a file or grep should stay plain reference text. It is drawn
    // without colour because clap styles the rest of the page itself.
    let mut command = Cli::command();
    if std::io::stdout().is_terminal() {
        let plain = ui::Ui {
            color: false,
            ..ui.clone()
        };
        // clap adds its own blank line after this, so drop the mark's.
        command = command.before_help(banner::mark(&plain).trim_end().to_string());
    }
    let cli = Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|e| e.exit());

    // Resolved again now that the flags are known; the Ui above only had the
    // environment to go on.
    ui::install(ui::Ui::resolve(cli.color, cli.json, cli.verbose));

    match cli.command {
        Some(Commands::Play { id }) => cmd::playback::play(id),
        Some(Commands::Tracks(cmd)) => cmd::tracks::run(cmd),
        Some(Commands::Playlists(cmd)) => cmd::playlists::run(cmd),
        Some(Commands::Playback(cmd)) => cmd::playback::run(cmd),
        Some(Commands::Queue(cmd)) => cmd::queue::run(cmd),
        Some(Commands::Devices(cmd)) => cmd::devices::run(cmd),
        Some(Commands::Favorite(cmd)) => cmd::favorite::run(cmd),
        Some(Commands::Metadata(cmd)) => cmd::metadata::run(cmd),
        Some(Commands::Dsp(cmd)) => cmd::dsp::run(cmd),
        Some(Commands::Stats(cmd)) => cmd::stats::run(cmd),
        Some(Commands::Now { once, interval }) => now::run(once, interval),
        Some(Commands::Completions { shell }) => {
            clap_complete::generate(shell, &mut Cli::command(), "wave", &mut std::io::stdout())
        }
        // --cli, --headless, or only global flags: the landing screen. The
        // mark is decoration, so it stays out of piped output.
        None if cli.json => json::emit(&banner::LandingFacts::gather()),
        None if cli.no_banner || !std::io::stdout().is_terminal() => {
            print!("{}", banner::landing_body(ui::current()))
        }
        None => print!("{}", banner::landing(ui::current())),
    }
}

pub(crate) fn daemon_cmd(request: DaemonRequest) {
    match daemon_request_if_running(request) {
        Ok(Some(resp)) if resp.ok => {
            let msg = resp.message.unwrap_or_else(|| "Done.".to_string());
            ui::done(msg, serde_json::json!({ "status": resp.status }));
        }
        Ok(Some(resp)) => {
            ui::fail(
                resp.error.unwrap_or_else(|| "Unknown error".to_string()),
                None,
                ui::EXIT_GENERAL,
            );
        }
        Ok(None) => {
            ui::no_daemon();
        }
        Err(e) => {
            ui::fail(e, None, ui::EXIT_GENERAL);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

pub(crate) fn open_library() -> Library {
    let db_path = default_db_path();
    Library::new_with_path(&db_path).unwrap_or_else(|e| {
        ui::fail(
            format!("Failed to open library at {}: {e}", db_path.display()),
            None,
            ui::EXIT_GENERAL,
        );
    })
}

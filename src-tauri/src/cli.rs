use std::path::Path;

use clap::{CommandFactory, Parser, Subcommand};

use crate::audio::player::AudioPlayer;
use crate::library::Library;
use crate::metadata::{extract_track, Track};
use crate::playback_daemon::{daemon_request, daemon_request_if_running, DaemonRequest, PlaybackStatus};

// ── Top-level CLI ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "wave", version, about = "Lightweight Music Player — CLI")]
pub struct Cli {
    /// Show CLI help (runs in CLI mode without a command)
    #[arg(long, global = true)]
    pub cli: bool,

    /// Run in headless mode (same as --cli)
    #[arg(long, global = true)]
    pub headless: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
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
    /// DSP / equalizer controls
    #[command(subcommand)]
    Dsp(DspCmd),
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
    /// Show current EQ settings (band gains and enabled state)
    EqShow,
    /// Set EQ band gains (10 values in dB, one per ISO band)
    EqSet {
        /// 10 gain values in dB for bands 31, 62, 125, 250, 500,
        /// 1000, 2000, 4000, 8000, 16000 Hz
        bands: Vec<f32>,
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
}

// ── Library path resolution ─────────────────────────────────────────────────

/// Return the default database path for CLI mode.
fn default_db_path() -> std::path::PathBuf {
    crate::app_paths::library_db_path()
}

// ── Track resolution helpers ────────────────────────────────────────────────

/// Try to resolve a user-supplied track identifier to a file path.
/// Accepts either a UUID stored in the database or a direct file path.
fn resolve_track_path(library: &Library, id_or_path: &str) -> Result<String, String> {
    // Check if it's a UUID (track ID) in the database
    if uuid::Uuid::parse_str(id_or_path).is_ok() {
        if let Some(track) = library.get_track_by_id(id_or_path)? {
            return Ok(track.path);
        }
    }
    // Treat as a file path; verify it exists
    let path = Path::new(id_or_path);
    if path.exists() {
        Ok(id_or_path.to_string())
    } else {
        Err(format!(
            "Track not found: {id_or_path} (not a valid UUID or existing file path)"
        ))
    }
}

/// Pretty-print a track in a human-readable one-line format.
fn print_track(track: &Track) {
    let duration = track
        .duration_seconds
        .map(|s| format_duration(s as u64))
        .unwrap_or_else(|| "--:--".to_string());
    println!(
        "  {:36}  {:6}  {:4}  {:30}  {:30}  {:40}",
        track.id,
        track.format,
        duration,
        truncate(&track.artist, 28),
        truncate(&track.album, 28),
        truncate(&track.title, 38),
    );
}

fn print_track_header() {
    println!(
        "  {:36}  {:6}  {:4}  {:30}  {:30}  {:40}",
        "ID", "FORMAT", "DUR", "ARTIST", "ALBUM", "TITLE"
    );
    println!(
        "  {}  {}  {}  {}  {}  {}",
        "-".repeat(36),
        "-".repeat(6),
        "-".repeat(4),
        "-".repeat(30),
        "-".repeat(30),
        "-".repeat(40),
    );
}

fn format_duration(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    format!("{m:02}:{s:02}")
}

fn truncate(s: &str, max: usize) -> String {
    let mut idx = 0;
    let mut count = 0;
    for c in s.chars() {
        if count >= max.saturating_sub(1) {
            return format!("{}…", &s[..idx]);
        }
        count += 1;
        idx += c.len_utf8();
    }
    s.to_string()
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
        Some(Commands::Queue(_)) => true,
        Some(Commands::Devices(DevicesCmd::Volume { .. } | DevicesCmd::Switch { .. })) => true,
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
        Some(Commands::Playback(_)) | Some(Commands::Queue(_)) | Some(Commands::Devices(_))
    )
}

pub fn run() {
    // Handle --cli/--headless flags: show CLI help then exit
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 {
        let flag = args[1].as_str();
        if flag == "--cli" || flag == "--headless" {
            Cli::command().print_help().unwrap();
            println!();
            return;
        }
    }

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Tracks(cmd)) => run_tracks(cmd),
        Some(Commands::Playlists(cmd)) => run_playlists(cmd),
        Some(Commands::Playback(cmd)) => run_playback(cmd),
        Some(Commands::Queue(cmd)) => run_queue(cmd),
        Some(Commands::Devices(cmd)) => run_devices(cmd),
        Some(Commands::Favorite(cmd)) => run_favorite(cmd),
        Some(Commands::Metadata(cmd)) => run_metadata(cmd),
        Some(Commands::Dsp(cmd)) => run_dsp(cmd),
        None => {
            // No subcommand — shouldn't reach here since main.rs checks args > 1.
        }
    }
}

// ── Tracks commands ─────────────────────────────────────────────────────────

fn run_tracks(cmd: TracksCmd) {
    match cmd {
        TracksCmd::List { playlist_id } => cmd_tracks_list(playlist_id),
        TracksCmd::Import { paths } => cmd_tracks_import(paths),
        TracksCmd::Info { track_id } => cmd_tracks_info(track_id),
        TracksCmd::Query { query } => cmd_tracks_query(query),
        TracksCmd::Add {
            track_id,
            playlist_id,
        } => cmd_tracks_add(track_id, playlist_id),
        TracksCmd::Remove {
            track_id,
            playlist_id,
        } => cmd_tracks_remove(track_id, playlist_id),
        TracksCmd::Reset { yes } => cmd_tracks_reset(yes),
    }
}

fn cmd_tracks_list(playlist_id: Option<String>) {
    let library = open_library();
    let tracks = if let Some(pid) = &playlist_id {
        library.get_playlist_tracks(pid)
    } else {
        // Return all tracks from the library
        let conn = library.lock_connection().unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {} FROM {} ORDER BY t.artist, t.album, t.track_number",
                crate::library::TRACK_SELECT_COLUMNS,
                crate::library::TRACK_FROM
            ))
            .map_err(|e| format!("Failed to prepare query: {e}"))
            .unwrap();
        let cover_root = library.cover_root().to_path_buf();
        let rows = stmt
            .query_map([], |row| {
                crate::library::row_to_track(row, &cover_root)
            })
            .map_err(|e| format!("Failed to query tracks: {e}"))
            .unwrap();
        let tracks: Vec<Track> = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read tracks: {e}"))
            .unwrap();
        Ok(tracks)
    };
    match tracks {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("No tracks found.");
                return;
            }
            println!("Found {} track(s):", tracks.len());
            print_track_header();
            for track in &tracks {
                print_track(track);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_tracks_import(paths: Vec<String>) {
    let library = open_library();
    let mut total = 0;
    for path in &paths {
        let p = Path::new(path);
        if p.is_dir() {
            match library.index_directory(None, None, path.clone()) {
                Ok(tracks) => {
                    println!("Imported {} track(s) from {}", tracks.len(), path);
                    total += tracks.len();
                }
                Err(e) => eprintln!("Error importing directory {path}: {e}"),
            }
        } else if p.is_file() {
            match library.add_track_to_default_playlist(path.clone()) {
                Ok(track) => {
                    println!("Imported: {} — {}", track.artist, track.title);
                    total += 1;
                }
                Err(e) => eprintln!("Error importing {path}: {e}"),
            }
        } else {
            eprintln!("Path not found: {path}");
        }
    }
    if total > 0 {
        println!("Successfully imported {total} track(s).");
    }
}

fn cmd_tracks_info(track_id: String) {
    let library = open_library();
    let path = match resolve_track_path(&library, &track_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    // Try library first, then extract directly
    let track = library
        .get_tracks_by_paths(&[path.clone()])
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .or_else(|| extract_track(None, &path).ok());

    match track {
        Some(t) => print_full_metadata(&t),
        None => {
            eprintln!("Could not read track: {path}");
            std::process::exit(1);
        }
    }
}

fn cmd_tracks_query(query: String) {
    let library = open_library();
    match library.search_tracks(&query) {
        Ok(tracks) => {
            if tracks.is_empty() {
                println!("No tracks matching \"{query}\".");
                return;
            }
            println!(
                "Found {} track(s) matching \"{}\":",
                tracks.len(),
                query
            );
            print_track_header();
            for track in &tracks {
                print_track(track);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_tracks_add(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let result = if let Some(pid) = playlist_id {
        library.add_track_to_playlist(&pid, path)
    } else {
        library.add_track_to_default_playlist(path)
    };
    match result {
        Ok(track) => println!("Added: {} — {}", track.artist, track.title),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_tracks_remove(track_id: String, playlist_id: Option<String>) {
    let library = open_library();
    let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let result = if let Some(pid) = playlist_id {
        library
            .remove_track_from_playlist_by_path(&pid, &path)
            .map(|_| "Track removed from playlist.")
    } else {
        library
            .remove_track_from_library(&path)
            .map(|_| "Track removed from library.")
    };
    match result {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_tracks_reset(yes: bool) {
    if !yes {
        let confirmed = confirm_prompt(
            "This will delete ALL tracks and ALL playlists except Library and Favorites.\n\
             Type 'reset' to confirm: ",
            "reset",
        );
        if !confirmed {
            eprintln!("Aborted.");
            std::process::exit(1);
        }
    }

    let library = open_library();
    match library.reset_library() {
        Ok((tracks, playlists)) => {
            println!(
                "Library reset: removed {tracks} track(s) and deleted {playlists} playlist(s)."
            );
            println!("Library and Favorites were kept (empty).");
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn confirm_prompt(prompt: &str, expected: &str) -> bool {
    use std::io::{self, Write};
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => line.trim().eq_ignore_ascii_case(expected),
        Err(_) => false,
    }
}

// ── Playlists commands ──────────────────────────────────────────────────────

fn run_playlists(cmd: PlaylistsCmd) {
    match cmd {
        PlaylistsCmd::List => cmd_playlists_list(),
        PlaylistsCmd::Import { file, name } => cmd_playlists_import(file, name),
        PlaylistsCmd::Export { id, format, output } => cmd_playlists_export(id, format, output),
        PlaylistsCmd::Info { id } => cmd_playlists_info(id),
        PlaylistsCmd::Query { query } => cmd_playlists_query(query),
        PlaylistsCmd::Create { name } => cmd_playlists_create(name),
        PlaylistsCmd::Delete { id } => cmd_playlists_delete(id),
        PlaylistsCmd::Rename { id, name } => cmd_playlists_rename(id, name),
        PlaylistsCmd::Clear { id } => cmd_playlists_clear(id),
        PlaylistsCmd::AddTrack { id, track_id } => cmd_playlists_add_track(id, track_id),
        PlaylistsCmd::RemoveTrack { id, track_id } => cmd_playlists_remove_track(id, track_id),
        PlaylistsCmd::Sync { id } => cmd_playlists_sync(id),
    }
}

fn cmd_playlists_list() {
    let library = open_library();
    match library.list_playlists(None) {
        Ok(playlists) => {
            if playlists.is_empty() {
                println!("No playlists found.");
                return;
            }
            println!("Found {} playlist(s):", playlists.len());
            for pl in &playlists {
                let sync_tag = match &pl.sync_folder {
                    Some(folder) => format!("  [synced → {folder}]"),
                    None => String::new(),
                };
                println!(
                    "  {:36}  {:5} tracks  {}{}",
                    pl.id, pl.track_count, pl.name, sync_tag
                );
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_import(file: String, name: Option<String>) {
    let library = open_library();
    if let Err(e) = crate::path_validation::validate_playlist_import_path(&file) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    let ext = Path::new(&file)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    let result = match ext.as_str() {
        "json" => library.import_playlist_json(&file, name.as_deref()),
        "m3u" | "m3u8" => library.import_playlist_m3u(&file, name.as_deref()),
        _ => Err(format!("Unsupported playlist format: .{ext} (use .m3u, .m3u8, or .json)")),
    };
    match result {
        Ok((id, tracks)) => {
            let name = library
                .get_playlist_info(&id)
                .ok()
                .flatten()
                .map(|info| info.name)
                .unwrap_or_else(|| "Unknown".to_string());
            println!("Imported playlist \"{name}\" (ID: {id}) with {} track(s).", tracks.len());
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_export(id: String, format: String, output: String) {
    let library = open_library();
    let expected_ext = match format.as_str() {
        "m3u" => "m3u",
        "json" => "json",
        _ => {
            eprintln!("Unknown export format: {format} (use m3u or json)");
            std::process::exit(1);
        }
    };
    if let Err(e) = crate::path_validation::validate_safe_output_path(&output, expected_ext) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
    match format.as_str() {
        "m3u" => library.export_playlist_m3u(&id, &output),
        "json" => library.export_playlist_json(&id, &output),
        _ => unreachable!(),
    }
    .unwrap_or_else(|e| {
        eprintln!("Error: {e}");
        std::process::exit(1);
    });
    println!("Exported playlist {id} to {output}");
}

fn cmd_playlists_info(id: String) {
    let library = open_library();
    match library.get_playlist_info(&id) {
        Ok(Some(info)) => {
            println!("Playlist: {} (ID: {})", info.name, info.id);
            println!("  Track count: {}", info.track_count);
            match &info.sync_folder {
                Some(folder) => println!("  Synced folder: {folder}"),
                None => println!("  Synced folder: (none)"),
            }
            println!();
            match library.get_playlist_tracks(&id) {
                Ok(tracks) => {
                    for (i, track) in tracks.iter().enumerate() {
                        println!(
                            "  {:4}. {}  {:30}  {:30}  {:40}",
                            i + 1,
                            track.id,
                            truncate(&track.artist, 28),
                            truncate(&track.album, 28),
                            truncate(&track.title, 38),
                        );
                    }
                }
                Err(e) => eprintln!("Error fetching tracks: {e}"),
            }
        }
        Ok(None) => {
            eprintln!("Playlist not found: {id}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_query(query: String) {
    let library = open_library();
    match library.search_playlists(&query) {
        Ok(playlists) => {
            if playlists.is_empty() {
                println!("No playlists matching \"{query}\".");
                return;
            }
            println!("Found {} playlist(s) matching \"{}\":", playlists.len(), query);
            for pl in &playlists {
                let sync_tag = match &pl.sync_folder {
                    Some(_) => "  [synced]",
                    None => "",
                };
                println!(
                    "  {:36}  {:5} tracks  {}{}",
                    pl.id, pl.track_count, pl.name, sync_tag
                );
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_create(name: String) {
    let library = open_library();
    match library.create_playlist(&name, None) {
        Ok(info) => println!("Created playlist \"{}\" (ID: {})", info.name, info.id),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_delete(id: String) {
    let library = open_library();
    match library.delete_playlist(&id) {
        Ok(()) => println!("Deleted playlist {id}."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_rename(id: String, name: String) {
    let library = open_library();
    match library.rename_playlist(&id, &name) {
        Ok(()) => println!("Renamed playlist {id} to \"{name}\"."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_clear(id: String) {
    let library = open_library();
    match library.clear_playlist(&id) {
        Ok(()) => println!("Cleared all tracks from playlist {id}."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_add_track(id: String, track_id: String) {
    let library = open_library();
    let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    match library.add_track_to_playlist(&id, path) {
        Ok(track) => println!("Added to playlist: {} — {}", track.artist, track.title),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_remove_track(id: String, track_id: String) {
    let library = open_library();
    let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    match library.remove_track_from_playlist_by_path(&id, &path) {
        Ok(()) => println!("Removed track from playlist {id}."),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playlists_sync(id: String) {
    use crate::metadata::is_supported_audio_file;
    use walkdir::WalkDir;

    let library = open_library();
    let info = match library.get_playlist_info(&id) {
        Ok(Some(info)) => info,
        Ok(None) => {
            eprintln!("Error: Playlist not found: {id}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let Some(folder) = info.sync_folder.as_deref() else {
        eprintln!(
            "Error: Playlist \"{}\" is not linked to a sync folder.\n\
             Link one in the app (Create playlist → Sync with folder, or Add folder as playlist).",
            info.name
        );
        std::process::exit(1);
    };

    let dir_path = Path::new(folder);
    if !dir_path.is_dir() {
        eprintln!("Error: Sync folder is missing or not a directory: {folder}");
        std::process::exit(1);
    }

    let paths: Vec<String> = WalkDir::new(dir_path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| is_supported_audio_file(e.path()))
        .filter_map(|e| e.path().to_str().map(str::to_string))
        .collect();

    println!(
        "Syncing \"{}\" with {} ({} audio file(s) found)…",
        info.name,
        folder,
        paths.len()
    );

    match library.sync_playlist_to_paths(&id, &paths) {
        Ok((added, removed)) => {
            println!("Done. Added {added}, removed {removed}.");
            if let Ok(Some(updated)) = library.get_playlist_info(&id) {
                println!("Playlist now has {} track(s).", updated.track_count);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ── Playback commands ───────────────────────────────────────────────────────

fn run_playback(cmd: PlaybackCmd) {
    match cmd {
        PlaybackCmd::Start { id } => cmd_playback_start(id),
        PlaybackCmd::Pause => daemon_cmd(DaemonRequest::Pause),
        PlaybackCmd::Resume => daemon_cmd(DaemonRequest::Resume),
        PlaybackCmd::Stop => daemon_cmd(DaemonRequest::Stop),
        PlaybackCmd::Next => daemon_cmd(DaemonRequest::Next),
        PlaybackCmd::Previous => daemon_cmd(DaemonRequest::Previous),
        PlaybackCmd::Seek { seconds } => daemon_cmd(DaemonRequest::Seek { seconds }),
        PlaybackCmd::Status => cmd_playback_status(),
        PlaybackCmd::Shutdown => cmd_playback_shutdown(),
    }
}

fn daemon_cmd(request: DaemonRequest) {
    match daemon_request_if_running(request) {
        Ok(Some(resp)) if resp.ok => {
            if let Some(msg) = resp.message {
                println!("{msg}");
            }
        }
        Ok(Some(resp)) => {
            eprintln!("{}", resp.error.unwrap_or_else(|| "Unknown error".to_string()));
            std::process::exit(1);
        }
        Ok(None) => {
            eprintln!("Playback daemon is not running. Use `wave playback start` first.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playback_start(id: String) {
    match daemon_request(DaemonRequest::Start { id }) {
        Ok(resp) if resp.ok => {
            if let Some(msg) = resp.message {
                println!("{msg}");
            }
            println!("Playback running in background. Use `wave playback` subcommands to control.");
        }
        Ok(resp) => {
            eprintln!("{}", resp.error.unwrap_or_else(|| "Failed to start playback".to_string()));
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playback_shutdown() {
    match daemon_request_if_running(DaemonRequest::Shutdown) {
        Ok(Some(resp)) if resp.ok => {
            if let Some(msg) = resp.message {
                println!("{msg}");
            }
        }
        Ok(Some(resp)) => {
            eprintln!("{}", resp.error.unwrap_or_else(|| "Failed to shut down daemon".to_string()));
            std::process::exit(1);
        }
        Ok(None) => println!("Playback daemon is not running."),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn cmd_playback_status() {
    match daemon_request_if_running(DaemonRequest::Status) {
        Ok(Some(resp)) if resp.ok => {
            if let Some(status) = resp.status {
                print_playback_status(&status);
            }
        }
        Ok(Some(resp)) => {
            eprintln!("{}", resp.error.unwrap_or_else(|| "Daemon error".to_string()));
            std::process::exit(1);
        }
        Ok(None) => println!("Playback daemon is not running."),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn print_playback_status(status: &PlaybackStatus) {
    println!("  State:      {}", status.state);
    if let (Some(artist), Some(title)) = (&status.artist, &status.title) {
        println!("  Track:      {artist} — {title}");
    }
    println!("  File:       {}", status.file);
    println!(
        "  Position:   {} / {}",
        format_duration(status.position_seconds as u64),
        format_duration(status.duration_seconds as u64)
    );
    println!("  Volume:     {:.0}%", status.volume * 100.0);
    println!("  Device:     {}", status.device);
    println!("  Repeat:     {}", status.repeat);
    println!("  Shuffle:    {}", status.shuffle);
    println!(
        "  Queue:      track {} of {}",
        status.queue_index, status.queue_total
    );
}

// ── Queue commands ──────────────────────────────────────────────────────────

fn run_queue(cmd: QueueCmd) {
    match cmd {
        QueueCmd::List => {
            match daemon_request_if_running(DaemonRequest::QueueList) {
                Ok(Some(resp)) if resp.ok => {
                    let tracks = resp.queue.unwrap_or_default();
                    let current = resp
                        .status
                        .and_then(|s| if s.queue_index > 0 { Some(s.queue_index - 1) } else { None });
                    if tracks.is_empty() {
                        println!("Queue is empty.");
                        return;
                    }
                    println!("Queue ({} track(s)):", tracks.len());
                    for (i, path) in tracks.iter().enumerate() {
                        let marker = if Some(i) == current { ">" } else { " " };
                        let name = Path::new(path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(path);
                        println!("  {marker} {:4}. {name}", i);
                    }
                }
                Ok(Some(resp)) => {
                    eprintln!("{}", resp.error.unwrap_or_else(|| "Daemon error".to_string()));
                    std::process::exit(1);
                }
                Ok(None) => println!("Playback daemon is not running."),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        QueueCmd::Add { track_id } => daemon_cmd(DaemonRequest::QueueAdd { track_id }),
        QueueCmd::Remove { index } => daemon_cmd(DaemonRequest::QueueRemove { index }),
        QueueCmd::Next { track_id } => daemon_cmd(DaemonRequest::QueueInsertNext { track_id }),
        QueueCmd::Shuffle { state } => {
            let enable = match state.as_deref() {
                Some("on") => Some(true),
                Some("off") => Some(false),
                None => None,
                Some(other) => {
                    eprintln!("Invalid shuffle value: {other} (use on, off, or omit to toggle)");
                    std::process::exit(1);
                }
            };
            daemon_cmd(DaemonRequest::QueueShuffle { enable });
        }
        QueueCmd::Repeat { mode } => daemon_cmd(DaemonRequest::QueueRepeat { mode }),
        QueueCmd::Clear => daemon_cmd(DaemonRequest::QueueClear),
    }
}

// ── Devices commands ────────────────────────────────────────────────────────

fn run_devices(cmd: DevicesCmd) {
    match cmd {
        DevicesCmd::List => cmd_devices_list(),
        DevicesCmd::Switch { name } => cmd_devices_switch(name),
        DevicesCmd::Volume { level } => cmd_devices_volume(level),
    }
}

fn cmd_devices_list() {
    let devices = AudioPlayer::list_output_devices();
    if devices.is_empty() {
        println!("No audio output devices found.");
        return;
    }
    let current = AudioPlayer::current_output_name();
    println!("Available output devices:");
    for device in &devices {
        let marker = if *device == current { "* " } else { "  " };
        println!("  {marker}{device}");
    }
    println!("  (* = default)");
}

fn cmd_devices_switch(name: String) {
    daemon_cmd(DaemonRequest::SetDevice { name });
}

fn cmd_devices_volume(level: f32) {
    daemon_cmd(DaemonRequest::Volume { level });
}

// ── Favorite commands ───────────────────────────────────────────────────────

fn run_favorite(cmd: FavoriteCmd) {
    let library = open_library();
    match cmd {
        FavoriteCmd::Add { track_id } => {
            let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            match library.add_track_to_favorites(path) {
                Ok(track) => println!("Added to favorites: {} — {}", track.artist, track.title),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        FavoriteCmd::Remove { track_id } => {
            let path = resolve_track_path(&library, &track_id).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            match library.remove_track_from_favorites(&path) {
                Ok(()) => println!("Removed from favorites."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        FavoriteCmd::List => match library.get_favorites() {
            Ok(tracks) => {
                if tracks.is_empty() {
                    println!("No favorites.");
                    return;
                }
                println!("Favorites ({} track(s)):", tracks.len());
                print_track_header();
                for track in &tracks {
                    print_track(track);
                }
            }
            Err(e) => eprintln!("Error: {e}"),
        },
        FavoriteCmd::Clear => match library.clear_favorites() {
            Ok(()) => println!("Favorites cleared."),
            Err(e) => eprintln!("Error: {e}"),
        },
    }
}

// ── Metadata commands ───────────────────────────────────────────────────────

fn run_metadata(cmd: MetadataCmd) {
    match cmd {
        MetadataCmd::Get { track_id } => cmd_metadata_get(track_id),
        MetadataCmd::CoverExport { track_id, output } => cmd_metadata_cover_export(track_id, output),
        MetadataCmd::CoverSet { track_id, image } => cmd_metadata_cover_set(track_id, image),
    }
}

fn cmd_metadata_get(track_id: String) {
    let library = open_library();
    let path = match resolve_track_path(&library, &track_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let track = library
        .get_tracks_by_paths(&[path.clone()])
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .or_else(|| extract_track(None, &path).ok());
    match track {
        Some(t) => print_full_metadata(&t),
        None => {
            eprintln!("Could not read track: {path}");
            std::process::exit(1);
        }
    }
}

fn cmd_metadata_cover_export(track_id: String, output: String) {
    let library = open_library();
    let path = match resolve_track_path(&library, &track_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let track = library
        .get_tracks_by_paths(&[path.clone()])
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .or_else(|| extract_track(None, &path).ok());
    match track {
        Some(t) => {
            if let Some(data_url) = &t.cover_art_data_url {
                // data:image/jpeg;base64,/9j...
                if let Some(comma_pos) = data_url.find(',') {
                    let b64 = &data_url[comma_pos + 1..];
                    use base64::Engine;
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => {
                            std::fs::write(&output, &bytes).unwrap_or_else(|e| {
                                eprintln!("Failed to write cover art: {e}");
                                std::process::exit(1);
                            });
                            println!("Cover art exported to {output}");
                        }
                        Err(e) => {
                            eprintln!("Failed to decode cover art: {e}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    eprintln!("Invalid cover art data URL.");
                    std::process::exit(1);
                }
            } else {
                eprintln!("No cover art available for this track.");
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("Could not read track: {path}");
            std::process::exit(1);
        }
    }
}

fn cmd_metadata_cover_set(track_id: String, image: String) {
    let library = open_library();

    // Resolve track
    let path = match resolve_track_path(&library, &track_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Read the image file
    let image_data = std::fs::read(&image).unwrap_or_else(|e| {
        eprintln!("Failed to read image file {image}: {e}");
        std::process::exit(1);
    });

    // Determine MIME type from extension
    let mime = match Path::new(&image)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        other => {
            eprintln!("Unsupported image format: {:?} (use jpg, png, webp, gif, or bmp)", other);
            std::process::exit(1);
        }
    };

    // Look up the track ID in the database
    let track_id_uuid = library
        .get_tracks_by_paths(&[path.clone()])
        .ok()
        .and_then(|v| v.into_iter().next().flatten())
        .map(|t| t.id)
        .unwrap_or_else(|| {
            library.add_track_to_default_playlist(path.clone()).unwrap_or_else(|e| {
                eprintln!("Failed to add track to library: {e}");
                std::process::exit(1);
            }).id
        });

    library.set_track_cover(&track_id_uuid, &image_data, mime).unwrap_or_else(|e| {
        eprintln!("Failed to set cover art: {e}");
        std::process::exit(1);
    });

    println!("Cover art set for track {track_id_uuid}");
}

// ── DSP commands ────────────────────────────────────────────────────────────

fn run_dsp(cmd: DspCmd) {
    use crate::audio::dsp::EqConfig;

    match cmd {
        DspCmd::EqShow => {
            let player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            let eq = player.eq_settings();
            println!("Equalizer: {}", if eq.enabled { "ON" } else { "OFF" });
            println!();
            println!("  Band   Frequency      Gain (dB)");
            println!("  ----   ---------      ---------");
            for (i, (freq, gain)) in
                crate::audio::dsp::EQ_BANDS_HZ.iter().zip(eq.bands.iter()).enumerate()
            {
                let bar = if eq.enabled {
                    let steps = (*gain as i32).clamp(-12, 12);
                    let ch = if steps >= 0 { '+' } else { '-' };
                    let bar_str: String = (0..steps.unsigned_abs()).map(|_| ch).collect();
                    format!(" {:>12}", bar_str)
                } else {
                    String::new()
                };
                println!(
                    "  {i:>4}   {:>9} Hz    {:>+6.1} dB{}",
                    freq, gain, bar
                );
            }
            println!();
            if eq.enabled {
                print!("  Curve: ");
                for gain in &eq.bands {
                    let c = if *gain > 1.0 {
                        '▁'
                    } else if *gain > 0.0 {
                        '▂'
                    } else if *gain == 0.0 {
                        '▄'
                    } else if *gain > -1.0 {
                        '▆'
                    } else {
                        '█'
                    };
                    print!("{c} ");
                }
                println!();
            }
        }
        DspCmd::EqSet { bands } => {
            if bands.len() != 10 {
                eprintln!("Error: expected exactly 10 EQ band values, got {}", bands.len());
                std::process::exit(1);
            }
            let mut arr = [0.0f32; 10];
            arr.copy_from_slice(&bands);
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.set_eq_bands(arr);
            player.set_eq_enabled(true);
            println!("EQ bands set and enabled.");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        DspCmd::EqEnable => {
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.set_eq_enabled(true);
            println!("Equalizer enabled.");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        DspCmd::EqDisable => {
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.set_eq_enabled(false);
            println!("Equalizer disabled.");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        DspCmd::EqReset => {
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.set_eq_bands([0.0; 10]);
            player.set_eq_enabled(true);
            println!("Equalizer reset to flat and enabled.");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        DspCmd::Preset { name } => {
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.apply_eq_preset(&name).unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            println!("Applied EQ preset: {name}");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        DspCmd::Presets => {
            println!("Available EQ presets:");
            for (name, desc) in EqConfig::list_presets() {
                println!("  {name:16}  {desc}");
            }
        }
        DspCmd::Export { output, name } => {
            let player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            let eq = player.eq_settings();
            crate::audio::dsp::EqPresetFile::save_to(&output, &eq, name).unwrap_or_else(|e| {
                eprintln!("Failed to export EQ: {e}");
                std::process::exit(1);
            });
            println!("EQ settings exported to {output}");
        }
        DspCmd::Import { input } => {
            let eq = crate::audio::dsp::EqPresetFile::load_from(&input).unwrap_or_else(|e| {
                eprintln!("Failed to import EQ: {e}");
                std::process::exit(1);
            });
            let mut player = AudioPlayer::new().unwrap_or_else(|e| {
                eprintln!("Failed to initialize audio player: {e}");
                std::process::exit(1);
            });
            player.set_eq_bands(eq.bands);
            player.set_eq_enabled(eq.enabled);
            println!("EQ settings imported from {input} and applied.");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn open_library() -> Library {
    let db_path = default_db_path();
    Library::new_with_path(&db_path).unwrap_or_else(|e| {
        eprintln!("Failed to open library at {}: {e}", db_path.display());
        std::process::exit(1);
    })
}

fn print_full_metadata(track: &Track) {
    println!("ID:              {}", track.id);
    println!("Path:            {}", track.path);
    println!("Name:            {}", track.name);
    println!("Title:           {}", track.title);
    println!("Artist:          {}", track.artist);
    println!("Album:           {}", track.album);
    println!("Album Artist:    {}", track.album_artist.as_deref().unwrap_or("(none)"));
    println!("Genre:           {}", track.genre.as_deref().unwrap_or("(none)"));
    println!("Year:            {}", track.year.map(|y| y.to_string()).unwrap_or_else(|| "(none)".to_string()));
    println!("Track Number:    {}", track.track_number.map(|n| n.to_string()).unwrap_or_else(|| "(none)".to_string()));
    println!("Disc Number:     {}", track.disc_number.map(|n| n.to_string()).unwrap_or_else(|| "(none)".to_string()));
    println!("Format:          {}", track.format);
    println!("Duration:        {}", track.duration_seconds.map(|s| format_duration(s as u64)).unwrap_or_else(|| "Unknown".to_string()));
    println!("Sample Rate:     {}", track.sample_rate.map(|r| format!("{} Hz", r)).unwrap_or_else(|| "Unknown".to_string()));
    println!("Channels:        {}", track.channels.map(|c| c.to_string()).unwrap_or_else(|| "Unknown".to_string()));
    println!("Bit Depth:       {}", track.bit_depth.map(|b| format!("{} bit", b)).unwrap_or_else(|| "Unknown".to_string()));
    println!("File Size:       {} bytes", track.file_size);
    println!("Modified:        {}", track.modified_at);
    println!("Indexed:         {}", track.indexed_at);
    println!("Lyrics Source:   {}", track.lyrics_source.as_deref().unwrap_or("(none)"));
    println!("Cover Art MIME:  {}", track.cover_art_mime.as_deref().unwrap_or("(none)"));
    println!("Cover Art Src:   {}", track.cover_art_source.as_deref().unwrap_or("(none)"));
    println!("Fingerprint:     {}", track.fingerprint_sha256.as_deref().unwrap_or("(none)"));
    println!("MusicBrainz ID:  {}", track.musicbrainz_recording_id.as_deref().unwrap_or("(none)"));
    if let Some(lyrics) = &track.lyrics {
        println!("\nLyrics:\n{}", lyrics);
    }
}



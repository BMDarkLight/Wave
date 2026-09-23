// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Turning Wave's types into text.
//!
//! Nothing here prints. Every function returns a String, which is what lets
//! the layout be tested without capturing stdout.

use crate::cli::bar;
use crate::cli::table::{Column, Table};
use crate::cli::ui::{display_width, truncate, Align, Ui};
use crate::dto::{ListenRankDto, ListeningStatsDto};
use crate::library::PlaylistInfo;
use crate::metadata::Track;
use crate::playback_daemon::{DspStatus, PlaybackStatus};

/// Everything in the playback block hangs off this indent.
const INDENT: &str = "     ";

/// Below this width the volume and device share no line.
const SPLIT_BELOW: usize = 60;

/// The leading chunk of a UUID, which is what listings show.
pub fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

pub fn format_duration(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn format_listen_duration(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn track_duration(track: &Track) -> String {
    track
        .duration_seconds
        .map(|s| format_duration(s as u64))
        .unwrap_or_else(|| "--:--".to_string())
}

pub fn state_glyph(ui: &Ui, state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "playing" => ui.glyphs.playing,
        "paused" => ui.glyphs.paused,
        _ => ui.glyphs.stopped,
    }
}

fn col(header: &'static str, min: usize, weight: u16, align: Align) -> Column {
    Column {
        header,
        min,
        weight,
        align,
    }
}

pub fn track_table(ui: &Ui, tracks: &[Track]) -> String {
    let mut t = Table::new(vec![
        col("ID", 8, 0, Align::Left),
        col("DUR", 5, 0, Align::Right),
        col("ARTIST", 10, 2, Align::Left),
        col("ALBUM", 10, 2, Align::Left),
        col("TITLE", 12, 3, Align::Left),
    ]);
    for track in tracks {
        t.push(vec![
            short_id(&track.id).to_string(),
            track_duration(track),
            track.artist.clone(),
            track.album.clone(),
            track.title.clone(),
        ]);
    }
    t.render(ui)
}

pub fn playlist_table(ui: &Ui, playlists: &[PlaylistInfo]) -> String {
    let mut t = Table::new(vec![
        // Full ids: playlist commands take the whole id, unlike tracks,
        // which also resolve a prefix.
        col("ID", 36, 0, Align::Left),
        col("TRACKS", 6, 0, Align::Right),
        col("NAME", 12, 2, Align::Left),
        col("SYNCED FOLDER", 13, 3, Align::Left),
    ]);
    for pl in playlists {
        t.push(vec![
            pl.id.clone(),
            pl.track_count.to_string(),
            pl.name.clone(),
            pl.sync_folder.clone().unwrap_or_default(),
        ]);
    }
    t.render(ui)
}

/// A playlist's details followed by its tracks.
pub fn playlist_info(ui: &Ui, info: &PlaylistInfo, tracks: &[Track]) -> String {
    let mut out = format!("{}\n", ui.heading(&info.name));
    out.push_str(&ui.kv("ID", &info.id, 14));
    out.push('\n');
    out.push_str(&ui.kv("Tracks", &info.track_count.to_string(), 14));
    out.push('\n');
    let folder = info.sync_folder.as_deref().unwrap_or("(none)");
    out.push_str(&ui.kv("Synced folder", folder, 14));
    out.push_str("\n\n");
    out.push_str(&track_table(ui, tracks));
    out
}

/// A line of dim text cut to what is left of the terminal after the indent.
fn dim_line(ui: &Ui, text: &str) -> String {
    let room = ui.width.saturating_sub(display_width(INDENT));
    format!(
        "{INDENT}{}\n",
        ui.dim(&truncate(text, room, ui.glyphs.ellipsis))
    )
}

pub fn playback_status(ui: &Ui, status: &PlaybackStatus, album: Option<&str>) -> String {
    let room = ui.width.saturating_sub(display_width(INDENT));
    let mut out = String::new();

    let title = status.title.as_deref().unwrap_or_else(|| {
        std::path::Path::new(&status.file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&status.file)
    });
    let glyph = state_glyph(ui, &status.state);
    // The ASCII paused glyph is two columns, so measure rather than assume.
    let title_room = ui.width.saturating_sub(4 + display_width(glyph));
    out.push_str(&format!(
        "  {glyph}  {}\n",
        ui.heading(&truncate(title, title_room, ui.glyphs.ellipsis))
    ));

    // Emitted even when empty. The dashboard rewinds by a line count to
    // redraw, so the height must not depend on whether a track happens to
    // carry an artist or an album.
    let byline: Vec<&str> = [status.artist.as_deref(), album]
        .into_iter()
        .flatten()
        .collect();
    let dot = format!(" {} ", ui.glyphs.dot);
    out.push_str(&dim_line(ui, &byline.join(&dot)));
    out.push('\n');

    // A stopped daemon reports a zero duration, so guard the division.
    let fraction = if status.duration_seconds > 0.0 {
        status.position_seconds / status.duration_seconds
    } else {
        0.0
    };
    let remaining = (status.duration_seconds - status.position_seconds).max(0.0);
    let timing = format!(
        "  {} / {}  -{}",
        format_duration(status.position_seconds as u64),
        format_duration(status.duration_seconds as u64),
        format_duration(remaining as u64)
    );
    let bar_width = room.saturating_sub(display_width(&timing)).min(60);
    out.push_str(&format!(
        "{INDENT}{}{}\n",
        bar::progress(ui, fraction, bar_width),
        ui.dim(&timing)
    ));
    out.push('\n');

    let volume = format!(
        "{} {} {}%",
        ui.dim("Volume"),
        bar::meter(ui, status.volume as f64, 10),
        (status.volume * 100.0).round() as i32
    );
    // Width of the volume segment as printed, without its escape codes.
    let volume_width = display_width("Volume ")
        + 10
        + 1
        + display_width(&format!("{}%", (status.volume * 100.0).round() as i32));
    if ui.width >= SPLIT_BELOW {
        let gap = "     ";
        let label = "Device ";
        let device_room = room.saturating_sub(volume_width + gap.len() + label.len());
        out.push_str(&format!(
            "{INDENT}{volume}{gap}{}{}\n",
            ui.dim(label),
            truncate(&status.device, device_room, ui.glyphs.ellipsis)
        ));
    } else {
        out.push_str(&format!("{INDENT}{volume}\n"));
        let device_room = room.saturating_sub("Device ".len());
        out.push_str(&format!(
            "{INDENT}{}{}\n",
            ui.dim("Device "),
            truncate(&status.device, device_room, ui.glyphs.ellipsis)
        ));
    }

    out.push_str(&dim_line(
        ui,
        &format!(
            "Shuffle {}{dot}Repeat {}{dot}Track {} of {}",
            if status.shuffle { "on" } else { "off" },
            status.repeat,
            status.queue_index,
            status.queue_total
        ),
    ));

    out
}

pub fn dsp_status(ui: &Ui, dsp: &DspStatus) -> String {
    let on_off = |b: bool| if b { "ON" } else { "OFF" };
    let crossfade = if dsp.crossfade_duration <= 0.0 {
        "OFF".to_string()
    } else {
        format!("{:.1}s", dsp.crossfade_duration)
    };

    let mut out = String::new();
    for (label, value) in [
        ("Equalizer", on_off(dsp.eq_enabled).to_string()),
        ("Gapless", on_off(dsp.gapless_enabled).to_string()),
        ("Crossfade", crossfade),
        ("Bass", format!("{:+.1} dB", dsp.bass)),
        ("Treble", format!("{:+.1} dB", dsp.treble)),
    ] {
        out.push_str(&ui.kv(label, &value, 10));
        out.push('\n');
    }
    out.push('\n');

    // The scale labels sit over the ends and the middle of the band bars.
    let half = 11;
    out.push_str(&format!(
        "  {}\n",
        ui.dim(&format!(
            "Band   Freq        Gain       {:<half$}0{:>half$}",
            "-12", "+12"
        ))
    ));
    for (i, (freq, gain)) in crate::audio::dsp::EQ_BANDS_HZ
        .iter()
        .zip(dsp.bands.iter())
        .enumerate()
    {
        let row = format!(
            "  {:>4}   {:>6.0} Hz   {:>+6.1} dB   {}",
            i + 1,
            freq,
            gain,
            bar::eq_band(ui, *gain, half)
        );
        // The empty half of a bar is spaces, which should not trail the line.
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

/// The ten band gains on one line, for commands that only nudged the curve.
pub fn eq_bands_brief(ui: &Ui, dsp: &DspStatus) -> String {
    let gains: Vec<String> = dsp.bands.iter().map(|g| format!("{g:+.1}")).collect();
    format!("{} {}", ui.dim("Bands"), gains.join(", "))
}

pub fn metadata_block(ui: &Ui, track: &Track) -> String {
    fn or_none<T: ToString>(value: Option<T>) -> String {
        value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "(none)".into())
    }
    fn or_unknown(value: Option<String>) -> String {
        value.unwrap_or_else(|| "Unknown".into())
    }

    let rows: Vec<(&str, String)> = vec![
        ("ID", track.id.clone()),
        ("Path", track.path.clone()),
        ("Name", track.name.clone()),
        ("Title", track.title.clone()),
        ("Artist", track.artist.clone()),
        ("Album", track.album.clone()),
        ("Album Artist", or_none(track.album_artist.as_deref())),
        ("Genre", or_none(track.genre.as_deref())),
        ("Year", or_none(track.year)),
        ("Track Number", or_none(track.track_number)),
        ("Disc Number", or_none(track.disc_number)),
        ("Format", track.format.clone()),
        (
            "Duration",
            or_unknown(track.duration_seconds.map(|s| format_duration(s as u64))),
        ),
        (
            "Sample Rate",
            or_unknown(track.sample_rate.map(|r| format!("{r} Hz"))),
        ),
        (
            "Channels",
            or_unknown(track.channels.map(|c| c.to_string())),
        ),
        (
            "Bit Depth",
            or_unknown(track.bit_depth.map(|b| format!("{b} bit"))),
        ),
        ("File Size", format!("{} bytes", track.file_size)),
        ("Modified", track.modified_at.to_string()),
        ("Indexed", track.indexed_at.to_string()),
        ("Lyrics Source", or_none(track.lyrics_source.as_deref())),
        ("Cover Art MIME", or_none(track.cover_art_mime.as_deref())),
        ("Cover Art Src", or_none(track.cover_art_source.as_deref())),
        ("Fingerprint", or_none(track.fingerprint_sha256.as_deref())),
        (
            "MusicBrainz ID",
            or_none(track.musicbrainz_recording_id.as_deref()),
        ),
    ];

    let mut out = String::new();
    for (label, value) in rows {
        out.push_str(&ui.kv(label, &value, 16));
        out.push('\n');
    }
    if let Some(lyrics) = &track.lyrics {
        out.push('\n');
        out.push_str(&ui.heading("Lyrics"));
        out.push('\n');
        out.push_str(lyrics);
        out.push('\n');
    }
    out
}

pub fn track_rank_list(ui: &Ui, title: &str, tracks: &[Track]) -> String {
    let mut out = format!("{}\n", ui.heading(title));
    if tracks.is_empty() {
        out.push_str(&format!("  {}\n", ui.dim("(none)")));
        return out;
    }
    let mut t = Table::new(vec![
        col("#", 3, 0, Align::Right),
        col("ARTIST", 10, 2, Align::Left),
        col("TITLE", 12, 3, Align::Left),
        col("DUR", 5, 0, Align::Right),
    ]);
    for (i, track) in tracks.iter().enumerate() {
        t.push(vec![
            (i + 1).to_string(),
            track.artist.clone(),
            track.title.clone(),
            track_duration(track),
        ]);
    }
    out.push_str(&t.render(ui));
    out
}

pub fn named_rank_list(ui: &Ui, title: &str, rows: &[ListenRankDto]) -> String {
    let mut out = format!("{}\n", ui.heading(title));
    if rows.is_empty() {
        out.push_str(&format!("  {}\n", ui.dim("(none)")));
        return out;
    }
    let mut t = Table::new(vec![
        col("#", 3, 0, Align::Right),
        col("NAME", 12, 3, Align::Left),
        col("LISTENED", 10, 0, Align::Right),
        col("PLAYS", 5, 0, Align::Right),
    ]);
    for (i, row) in rows.iter().enumerate() {
        t.push(vec![
            (i + 1).to_string(),
            row.name.clone(),
            format_listen_duration(row.listen_seconds),
            row.play_count.to_string(),
        ]);
    }
    out.push_str(&t.render(ui));
    out
}

/// Totals followed by every top list, for `stats summary`.
pub fn listening_overview(ui: &Ui, stats: &ListeningStatsDto) -> String {
    let mut out = format!("{}\n", ui.heading("Listening overview"));
    for (label, value) in [
        (
            "Listened",
            format_listen_duration(stats.total_listen_seconds),
        ),
        ("Plays", stats.total_plays.to_string()),
        ("Tracks played", stats.tracks_played.to_string()),
    ] {
        out.push_str(&ui.kv(label, &value, 14));
        out.push('\n');
    }
    for block in [
        track_rank_list(ui, "Top tracks", &stats.top_tracks),
        named_rank_list(ui, "Top artists", &stats.top_artists),
        named_rank_list(ui, "Top albums", &stats.top_albums),
        named_rank_list(ui, "Top genres", &stats.top_genres),
    ] {
        out.push('\n');
        out.push_str(&block);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ui::{display_width, Ui};

    fn status() -> PlaybackStatus {
        PlaybackStatus {
            state: "playing".into(),
            file: "/music/a.flac".into(),
            position_seconds: 72.0,
            duration_seconds: 260.0,
            volume: 0.62,
            device: "MacBook Pro Speakers".into(),
            repeat: "all".into(),
            shuffle: false,
            queue_index: 1,
            queue_total: 12,
            title: Some("Once in a Lifetime".into()),
            artist: Some("Talking Heads".into()),
        }
    }

    fn wide(width: usize) -> Ui {
        Ui {
            width,
            ..Ui::plain()
        }
    }

    #[test]
    fn short_id_takes_eight_characters() {
        assert_eq!(short_id("3f2a91c4-1111-2222-3333-444444444444"), "3f2a91c4");
        // Anything already shorter is returned whole rather than panicking.
        assert_eq!(short_id("abc"), "abc");
        assert_eq!(short_id(""), "");
    }

    #[test]
    fn durations_are_mmss() {
        assert_eq!(format_duration(0), "00:00");
        assert_eq!(format_duration(61), "01:01");
        assert_eq!(format_duration(3599), "59:59");
        // Past an hour the minute field keeps counting rather than wrapping.
        assert_eq!(format_duration(3661), "61:01");
    }

    #[test]
    fn listen_durations_drop_empty_leading_units() {
        assert_eq!(format_listen_duration(0.0), "0s");
        assert_eq!(format_listen_duration(45.0), "45s");
        assert_eq!(format_listen_duration(125.0), "2m 5s");
        assert_eq!(format_listen_duration(3725.0), "1h 2m 5s");
        // A negative reading is a clock problem, not a reason to print junk.
        assert_eq!(format_listen_duration(-10.0), "0s");
    }

    #[test]
    fn an_empty_track_list_renders_nothing() {
        assert_eq!(track_table(&Ui::plain(), &[]), "");
    }

    #[test]
    fn state_glyph_covers_every_daemon_state() {
        let ui = Ui::plain();
        assert_eq!(state_glyph(&ui, "playing"), ui.glyphs.playing);
        assert_eq!(state_glyph(&ui, "Paused"), ui.glyphs.paused);
        assert_eq!(state_glyph(&ui, "stopped"), ui.glyphs.stopped);
        // An unknown state must not panic; it falls back to stopped.
        assert_eq!(state_glyph(&ui, "wedged"), ui.glyphs.stopped);
    }

    #[test]
    fn dsp_status_draws_one_row_per_band() {
        let ui = Ui::plain();
        let dsp = DspStatus {
            eq_enabled: true,
            bands: [4.0, 2.5, 0.0, -1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            crossfade_duration: 0.0,
            gapless_enabled: true,
            bass: 4.0,
            treble: 0.0,
        };
        let out = dsp_status(&ui, &dsp);
        for hz in ["31", "62", "125", "250", "16000"] {
            assert!(out.contains(hz), "missing band {hz}");
        }
        // The axis appears once per band row and nowhere else.
        assert_eq!(out.matches(ui.glyphs.axis).count(), 10);
    }

    #[test]
    fn dsp_status_reports_a_disabled_equalizer() {
        let ui = Ui::plain();
        let dsp = DspStatus {
            eq_enabled: false,
            bands: [0.0; 10],
            crossfade_duration: 0.0,
            gapless_enabled: false,
            bass: 0.0,
            treble: 0.0,
        };
        let out = dsp_status(&ui, &dsp);
        let eq_line = out.lines().find(|l| l.contains("Equalizer")).unwrap();
        assert!(eq_line.contains("OFF"), "{eq_line}");
    }

    #[test]
    fn playback_status_fits_the_terminal() {
        for width in [40, 60, 80, 120] {
            let out = playback_status(&wide(width), &status(), Some("Remain in Light"));
            for line in out.lines() {
                assert!(
                    display_width(line) <= width,
                    "width {width} produced a {} column line: {line:?}",
                    display_width(line)
                );
            }
        }
        let out = playback_status(&wide(80), &status(), Some("Remain in Light"));
        assert!(out.contains("01:12"));
        assert!(out.contains("04:20"));
        assert!(out.contains("Remain in Light"));
    }

    #[test]
    fn playback_status_survives_a_stopped_daemon() {
        let mut s = status();
        s.state = "stopped".into();
        s.position_seconds = 0.0;
        s.duration_seconds = 0.0;
        s.title = None;
        s.artist = None;
        let out = playback_status(&wide(80), &s, None);
        // A zero duration must not divide by zero or print a NaN bar.
        assert!(!out.contains("NaN"));
        assert!(out.contains("00:00"));
        // With no title, the file name stands in for it.
        assert!(out.contains("a.flac"));
    }

    #[test]
    fn playback_status_height_does_not_depend_on_its_content() {
        let ui = wide(80);
        let tall = playback_status(&ui, &status(), Some("Remain in Light"))
            .lines()
            .count();
        let mut bare = status();
        bare.artist = None;
        bare.title = None;
        let short = playback_status(&ui, &bare, None).lines().count();
        // The dashboard rewinds by a line count, so a missing artist or album
        // must not shrink the block.
        assert_eq!(tall, short);
    }

    #[test]
    fn eq_bands_brief_lists_all_ten_signed() {
        let dsp = DspStatus {
            eq_enabled: true,
            bands: [1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.5],
            crossfade_duration: 0.0,
            gapless_enabled: true,
            bass: 0.0,
            treble: 0.0,
        };
        let out = eq_bands_brief(&Ui::plain(), &dsp);
        assert!(out.contains("+1.0"));
        assert!(out.contains("-1.0"));
        assert!(out.contains("+2.5"));
        assert_eq!(out.matches(", ").count(), 9);
    }
}

// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Shared output styling for the CLI.
//!
//! Every decision here is a pure function taking explicit inputs. The wrappers
//! that read the real environment are thin on purpose, so the interesting
//! behaviour stays testable without mutating env vars.

use std::io::IsTerminal;

use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// Decide whether to emit escape codes.
///
/// An explicit `--color` wins outright. Failing that, `NO_COLOR` beats
/// `CLICOLOR_FORCE`, which in turn beats what the terminal says. JSON output
/// is never coloured, whatever anything else asks for.
pub fn want_color(
    choice: ColorChoice,
    json: bool,
    no_color: bool,
    clicolor_force: bool,
    is_tty: bool,
) -> bool {
    if json {
        return false;
    }
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if no_color {
                false
            } else if clicolor_force {
                true
            } else {
                is_tty
            }
        }
    }
}

pub mod style {
    use anstyle::{AnsiColor, Color, Style};

    pub const HEADING: Style = Style::new().bold();
    pub const DIM: Style = Style::new().dimmed();
    pub const OK: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
    pub const ERR: Style = Style::new()
        .bold()
        .fg_color(Some(Color::Ansi(AnsiColor::Red)));
    pub const WARN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
    pub const HINT: Style = Style::new().dimmed().italic();
}

#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub playing: &'static str,
    pub paused: &'static str,
    pub stopped: &'static str,
    pub favorite: &'static str,
    pub ok: &'static str,
    pub err: &'static str,
    pub rule: &'static str,
    pub axis: &'static str,
    pub bar_full: &'static str,
    pub bar_empty: &'static str,
    pub meter_on: &'static str,
    pub meter_off: &'static str,
    pub ellipsis: &'static str,
    pub hint: &'static str,
    /// Separator between short facts on one line.
    pub dot: &'static str,
    /// Frames of the busy indicator, drawn in turn.
    pub spinner: &'static [&'static str],
}

pub const UNICODE: Glyphs = Glyphs {
    playing: "\u{25b6}",
    paused: "\u{23f8}",
    stopped: "\u{25a0}",
    favorite: "\u{2665}",
    ok: "\u{2713}",
    err: "\u{2715}",
    rule: "\u{2500}",
    axis: "\u{2503}",
    bar_full: "\u{2593}",
    bar_empty: "\u{2591}",
    meter_on: "\u{25ae}",
    meter_off: "\u{25af}",
    ellipsis: "\u{2026}",
    hint: "\u{2192}",
    dot: "\u{b7}",
    spinner: &[
        "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
        "\u{2827}", "\u{2807}", "\u{280f}",
    ],
};

pub const ASCII: Glyphs = Glyphs {
    playing: ">",
    paused: "||",
    stopped: "#",
    favorite: "*",
    ok: "+",
    err: "x",
    rule: "-",
    axis: "|",
    bar_full: "#",
    bar_empty: ".",
    meter_on: "#",
    meter_off: ".",
    ellipsis: "...",
    hint: "->",
    dot: "-",
    spinner: &["|", "/", "-", "\\"],
};

pub fn glyphs_for(ascii_override: bool, unicode_locale: bool) -> Glyphs {
    if ascii_override || !unicode_locale {
        ASCII
    } else {
        UNICODE
    }
}

/// True when the console can be trusted with the block drawing characters.
///
/// Windows reports its code page directly. Elsewhere the locale vars are the
/// only signal, and an unset locale is treated as UTF-8 because macOS
/// terminals routinely leave it unset while still being UTF-8.
fn unicode_locale() -> bool {
    #[cfg(windows)]
    {
        // 65001 is UTF-8.
        unsafe { windows_sys::Win32::System::Console::GetConsoleOutputCP() == 65001 }
    }
    #[cfg(not(windows))]
    {
        for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
            if let Ok(value) = std::env::var(key) {
                if !value.is_empty() {
                    let upper = value.to_uppercase();
                    return upper.contains("UTF-8") || upper.contains("UTF8");
                }
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct Ui {
    pub color: bool,
    pub glyphs: Glyphs,
    pub width: usize,
    pub json: bool,
    pub verbose: bool,
}

impl Ui {
    /// A Ui that emits no escape codes, for tests and for piped output.
    pub fn plain() -> Ui {
        Ui {
            color: false,
            glyphs: UNICODE,
            width: 80,
            json: false,
            verbose: false,
        }
    }

    pub fn resolve(choice: ColorChoice, json: bool, verbose: bool) -> Ui {
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let clicolor_force = std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty());
        let is_tty = std::io::stdout().is_terminal();
        Ui {
            color: want_color(choice, json, no_color, clicolor_force, is_tty),
            glyphs: glyphs_for(std::env::var_os("WAVE_ASCII").is_some(), unicode_locale()),
            width: terminal_width(),
            json,
            verbose,
        }
    }

    pub fn paint(&self, style: anstyle::Style, text: &str) -> String {
        if self.color {
            format!("{style}{text}{style:#}")
        } else {
            text.to_string()
        }
    }

    pub fn heading(&self, text: &str) -> String {
        self.paint(style::HEADING, text)
    }

    pub fn dim(&self, text: &str) -> String {
        self.paint(style::DIM, text)
    }

    /// A label and value on one line, with the label padded to `pad` columns.
    pub fn kv(&self, label: &str, value: &str, pad_to: usize) -> String {
        format!("{}  {}", self.dim(&pad(label, pad_to, Align::Left)), value)
    }
}

/// A number with the noun that agrees with it: "1 track", "2 tracks".
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cut `s` to at most `max` terminal columns, marking the cut with `ellipsis`.
///
/// Counting chars here would break alignment for CJK and emoji titles, which
/// occupy two columns each.
pub fn truncate(s: &str, max: usize, ellipsis: &str) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    let marker = display_width(ellipsis);
    if max < marker {
        return String::new();
    }
    let budget = max - marker;
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = display_width(&c.to_string());
        if used + w > budget {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

pub fn pad(s: &str, width: usize, align: Align) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    let fill = " ".repeat(width - w);
    match align {
        Align::Left => format!("{s}{fill}"),
        Align::Right => format!("{fill}{s}"),
    }
}

/// Terminal width, falling back to `COLUMNS` and then to 80.
pub fn terminal_width() -> usize {
    if let Some((terminal_size::Width(w), _)) = terminal_size::terminal_size() {
        if w > 0 {
            return w as usize;
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|w| *w > 0)
        .unwrap_or(80)
}

pub const EXIT_GENERAL: i32 = 1;
pub const EXIT_NOT_FOUND: i32 = 3;
pub const EXIT_NO_DAEMON: i32 = 4;

/// Render an error and its optional next step.
///
/// Kept apart from `fail` so the shape can be tested without exiting.
pub fn format_error(ui: &Ui, message: &str, hint: Option<&str>) -> String {
    let head = format!("{} {}", ui.glyphs.err, message);
    let mut out = ui.paint(style::ERR, &head);
    if let Some(hint) = hint {
        let line = format!("  {} {}", ui.glyphs.hint, hint);
        out.push('\n');
        out.push_str(&ui.paint(style::HINT, &line));
    }
    out
}

/// Print an error to stderr without exiting, for commands that work through
/// several items and should not stop at the first bad one.
pub fn report(message: impl std::fmt::Display, hint: Option<&str>) {
    let ui = current();
    let message = message.to_string();
    if ui.json {
        eprintln!(
            "{}",
            crate::cli::json::error_payload(&message, EXIT_GENERAL)
        );
    } else {
        eprintln!("{}", format_error(ui, &message, hint));
    }
}

/// Print an error to stderr and exit.
///
/// Under `--json` the error goes out as a JSON envelope instead, so a script
/// reading stderr gets the same shape either way.
pub fn fail(message: impl std::fmt::Display, hint: Option<&str>, code: i32) -> ! {
    let ui = current();
    let message = message.to_string();
    if ui.json {
        crate::cli::json::emit_error(&message, code);
    }
    eprintln!("{}", format_error(ui, &message, hint));
    std::process::exit(code)
}

/// Report a change that went through: a tick line for people, or an ok
/// envelope carrying `extra` under `--json`, which also ends the process
/// since that envelope is the command's whole output.
pub fn done(message: impl std::fmt::Display, extra: serde_json::Value) {
    let ui = current();
    let message = message.to_string();
    if ui.json {
        let mut payload = extra;
        if !payload.is_object() {
            payload = serde_json::json!({ "result": payload });
        }
        payload["message"] = serde_json::Value::String(message);
        crate::cli::json::emit_ok(payload);
    }
    let line = format!("{} {}", ui.glyphs.ok, message);
    println!("{}", ui.paint(style::OK, &line));
}

pub fn no_daemon() -> ! {
    fail(
        "Playback daemon is not running.",
        Some("start it with: wave playback start <id or path>"),
        EXIT_NO_DAEMON,
    )
}

static CURRENT: std::sync::OnceLock<Ui> = std::sync::OnceLock::new();

/// Install the process-wide Ui. Called once from `cli::run`.
pub fn install(ui: Ui) {
    let _ = CURRENT.set(ui);
}

/// The process-wide Ui. Falls back to plain output when nothing installed
/// one, which is what unit tests get.
pub fn current() -> &'static Ui {
    CURRENT.get_or_init(Ui::plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_agree_with_their_noun() {
        assert_eq!(count(0, "track", "tracks"), "0 tracks");
        assert_eq!(count(1, "track", "tracks"), "1 track");
        assert_eq!(count(2, "playlist", "playlists"), "2 playlists");
    }

    #[test]
    fn json_always_wins_over_color_flags() {
        assert!(!want_color(ColorChoice::Always, true, false, true, true));
    }

    #[test]
    fn explicit_flag_beats_environment() {
        assert!(want_color(ColorChoice::Always, false, true, false, false));
        assert!(!want_color(ColorChoice::Never, false, false, true, true));
    }

    #[test]
    fn no_color_beats_clicolor_force() {
        assert!(!want_color(ColorChoice::Auto, false, true, true, true));
    }

    #[test]
    fn clicolor_force_beats_a_pipe() {
        assert!(want_color(ColorChoice::Auto, false, false, true, false));
    }

    #[test]
    fn auto_follows_the_terminal() {
        assert!(want_color(ColorChoice::Auto, false, false, false, true));
        assert!(!want_color(ColorChoice::Auto, false, false, false, false));
    }

    #[test]
    fn ascii_override_beats_a_utf8_locale() {
        assert_eq!(glyphs_for(true, true).playing, ASCII.playing);
        assert_eq!(glyphs_for(false, true).playing, UNICODE.playing);
        assert_eq!(glyphs_for(false, false).playing, ASCII.playing);
    }

    #[test]
    fn width_counts_columns_not_chars() {
        assert_eq!(display_width("abc"), 3);
        // Each CJK ideograph occupies two terminal columns.
        assert_eq!(display_width("\u{4f60}\u{597d}"), 4);
    }

    #[test]
    fn truncate_respects_display_width() {
        assert_eq!(truncate("abcdef", 4, "\u{2026}"), "abc\u{2026}");
        assert_eq!(truncate("abc", 4, "\u{2026}"), "abc");
        // Three CJK chars are six columns, so a four column budget keeps one
        // char plus the ellipsis and must not split a char in half.
        let cut = truncate("\u{4f60}\u{597d}\u{4e16}", 4, "\u{2026}");
        assert!(display_width(&cut) <= 4);
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_handles_a_budget_smaller_than_the_ellipsis() {
        assert_eq!(truncate("abcdef", 0, "\u{2026}"), "");
        assert_eq!(truncate("abcdef", 1, "\u{2026}"), "\u{2026}");
    }

    #[test]
    fn pad_uses_display_width() {
        assert_eq!(pad("ab", 4, Align::Left), "ab  ");
        assert_eq!(pad("ab", 4, Align::Right), "  ab");
        // Two columns of content in a four column cell leaves two spaces.
        assert_eq!(pad("\u{4f60}", 4, Align::Left), "\u{4f60}  ");
        // Content wider than the cell is returned unchanged, never negative.
        assert_eq!(pad("abcdef", 3, Align::Left), "abcdef");
    }

    #[test]
    fn plain_ui_emits_no_escape_codes() {
        let ui = Ui::plain();
        assert_eq!(ui.heading("Tracks"), "Tracks");
        assert!(!ui.dim("x").contains('\u{1b}'));
    }

    #[test]
    fn colored_ui_wraps_text_in_escape_codes() {
        let ui = Ui {
            color: true,
            ..Ui::plain()
        };
        let painted = ui.heading("Tracks");
        assert!(painted.starts_with('\u{1b}'));
        assert!(painted.contains("Tracks"));
        assert!(painted.ends_with("\u{1b}[0m"));
    }

    #[test]
    fn an_error_without_a_hint_is_one_line() {
        let ui = Ui::plain();
        let out = format_error(&ui, "Track not found: 3f2a91", None);
        assert_eq!(out, "\u{2715} Track not found: 3f2a91");
    }

    #[test]
    fn a_hint_goes_on_its_own_indented_line() {
        let ui = Ui::plain();
        let out = format_error(
            &ui,
            "Playback daemon is not running.",
            Some("start it with: wave playback start <id>"),
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('\u{2715}'));
        assert!(lines[1].starts_with("  \u{2192} "));
        assert!(lines[1].contains("wave playback start"));
    }

    #[test]
    fn errors_use_the_ascii_glyph_when_unicode_is_off() {
        let ui = Ui {
            glyphs: ASCII,
            ..Ui::plain()
        };
        let out = format_error(&ui, "boom", Some("try again"));
        assert!(out.starts_with("x boom"));
        assert!(out.contains("  -> try again"));
    }
}

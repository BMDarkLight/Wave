// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Terminal handling for the screens that redraw in place: `wave now` and
//! `wave lyrics show --live`.
//!
//! They redraw with cursor-up and erase-line rather than the alternate
//! screen, so scrollback survives and the terminal is left as it was found.
//! When stdin is a terminal it goes into raw mode so single keys can be read
//! without Enter; otherwise the screen only listens for Ctrl-C.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

const SHOW_CURSOR: &str = "\x1b[?25h";
const HIDE_CURSOR: &str = "\x1b[?25l";

/// Set while raw mode is on, so an exit from anywhere can undo it.
static RAW: AtomicBool = AtomicBool::new(false);
/// Set while the cursor is hidden.
static HIDDEN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Quit,
    Char(char),
    Left,
    Right,
    Up,
    Down,
    /// Space.
    PlayPause,
    /// The window changed size or focus; draw again straight away.
    Redraw,
}

/// Put the terminal back: raw mode off, cursor shown. Safe to call more than
/// once and from any exit path. `process::exit` neither runs destructors nor
/// flushes stdout, so the callers that leave that way must call this first.
pub fn restore() {
    if RAW.swap(false, Ordering::SeqCst) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    if HIDDEN.swap(false, Ordering::SeqCst) {
        print!("{SHOW_CURSOR}");
        let _ = std::io::stdout().flush();
    }
}

/// A screen that redraws in place until the user leaves it.
pub struct Live {
    raw: bool,
    height: usize,
}

impl Live {
    pub fn start() -> Self {
        // Ctrl-C only raises a signal outside raw mode; inside it, it comes
        // through as a key and `wait` handles it.
        let _ = ctrlc::set_handler(|| {
            restore();
            println!();
            std::process::exit(0);
        });
        let raw = std::io::stdin().is_terminal() && crossterm::terminal::enable_raw_mode().is_ok();
        RAW.store(raw, Ordering::SeqCst);
        HIDDEN.store(true, Ordering::SeqCst);
        print!("{HIDE_CURSOR}");
        let _ = std::io::stdout().flush();
        Self { raw, height: 0 }
    }

    /// Whether single keys can be read, so a key hint is worth showing.
    pub fn reads_keys(&self) -> bool {
        self.raw
    }

    /// Replace the previous frame with `text`. A shorter frame clears what
    /// the longer one left below it.
    pub fn draw(&mut self, text: &str) {
        let mut out = String::new();
        if self.height > 0 {
            out.push_str(&format!("\x1b[{}A\r", self.height));
        }
        for line in text.lines() {
            out.push_str(line);
            // Overwrite in place: erase whatever the last frame left on this
            // line past the new text. Raw mode stops the terminal turning a
            // newline into a carriage return as well, so send both.
            out.push_str("\x1b[K\r\n");
        }
        out.push_str("\x1b[J");
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
        self.height = text.lines().count();
    }

    /// Wait up to `timeout` for a key. Returns early when one is pressed.
    pub fn wait(&self, timeout: Duration) -> Option<Key> {
        if !self.raw {
            std::thread::sleep(timeout);
            return None;
        }
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !event::poll(left).unwrap_or(false) {
                return None;
            }
            let Ok(Event::Key(key)) = event::read() else {
                // A resize or focus change: redraw now, at the new size.
                return Some(Key::Redraw);
            };
            // Windows reports releases too; act on the press only.
            if key.kind == KeyEventKind::Release {
                continue;
            }
            return match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Key::Quit)
                }
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => Some(Key::Quit),
                KeyCode::Char(' ') => Some(Key::PlayPause),
                KeyCode::Char(c) => Some(Key::Char(c.to_ascii_lowercase())),
                KeyCode::Left => Some(Key::Left),
                KeyCode::Right => Some(Key::Right),
                KeyCode::Up => Some(Key::Up),
                KeyCode::Down => Some(Key::Down),
                _ => continue,
            };
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        restore();
    }
}

/// Terminal rows, or a guess when there is no terminal to ask.
pub fn terminal_height() -> usize {
    terminal_size::terminal_size()
        .map(|(_, terminal_size::Height(h))| h as usize)
        .filter(|h| *h > 0)
        .unwrap_or(24)
}

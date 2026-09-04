//! Lyrics parsing: plain text, LRC, Enhanced LRC, and TTML.
//!
//! The point of this module is one distinction — a *line* timing versus a
//! *word* timing. Line timings are what LRCLIB serves and what Wave has always
//! rendered: highlight the current line. Word timings are what makes karaoke
//! possible: wipe through a line syllable by syllable as it is sung.
//!
//! Three input shapes are recognised, and the parser reports which one it
//! found so the UI can pick a rendering rather than guess:
//!
//! - **Plain** — no timings at all. Rendered as a block of text.
//! - **LRC** — `[mm:ss.xx]` at the start of a line.
//! - **Enhanced LRC** — the same, plus `<mm:ss.xx>` before individual words.
//! - **TTML** — the format Apple-style syllable lyrics use, which additionally
//!   carries voice attribution (duets) and background vocals.
//!
//! Every parser degrades rather than fails: unparseable input comes back as
//! `Plain`, so a malformed file still shows its words on screen.

use serde::{Deserialize, Serialize};

/// Fallback span for a word with no resolvable end, e.g. the last word of the
/// last line in a file that never states a line end.
const DEFAULT_WORD_SECONDS: f64 = 0.4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsKind {
    /// No timings — render as text.
    Plain,
    /// Line timings only — highlight the active line.
    Line,
    /// Word timings present — karaoke rendering is possible.
    Word,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsWord {
    pub time: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsLine {
    pub time: f64,
    /// End of the line when known. `None` means "until the next line".
    pub end: Option<f64>,
    /// The whole line as text, always populated — the UI needs it for the
    /// line-level rendering path and for copying.
    pub text: String,
    /// Empty for line-level sources.
    pub words: Vec<LyricsWord>,
    /// TTML voice attribution (`v1`, `v2`, …). Lets a duet render two sides.
    pub agent: Option<String>,
    /// TTML background vocals (`ttm:role="x-bg"`), usually shown smaller.
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsSheet {
    pub kind: LyricsKind,
    pub lines: Vec<LyricsLine>,
    /// Original text, kept so the UI can fall back to showing it verbatim.
    pub plain: String,
}

impl LyricsSheet {
    fn plain(raw: &str) -> Self {
        Self {
            kind: LyricsKind::Plain,
            lines: Vec::new(),
            plain: raw.to_string(),
        }
    }
}

/// Parse any supported lyrics text into a sheet.
///
/// Detection is by content, not by file extension or provider name, because
/// lyrics reach Wave from tags, sidecar files, and network providers alike —
/// and any of them may hold any of these formats.
pub fn parse_sheet(raw: &str) -> LyricsSheet {
    if raw.trim().is_empty() {
        return LyricsSheet::plain(raw);
    }
    if looks_like_ttml(raw) {
        if let Some(sheet) = parse_ttml(raw) {
            return sheet;
        }
        // Malformed XML still has words in it; showing them beats showing
        // nothing, so fall through to the text path rather than erroring.
        return LyricsSheet::plain(raw);
    }
    parse_lrc(raw)
}

fn looks_like_ttml(raw: &str) -> bool {
    let head: String = raw
        .chars()
        .take(600)
        .collect::<String>()
        .to_ascii_lowercase();
    head.contains("<tt") && (head.contains("ttml") || head.contains("<body"))
}

// ── LRC ───────────────────────────────────────────────────────────────────

/// `[mm:ss.xx]` / `[mm:ss]`, and the `<mm:ss.xx>` word form. Minutes may run
/// past 99 on long recordings, so the minute field is not width-limited.
fn parse_clock(body: &str) -> Option<f64> {
    let (minutes, rest) = body.split_once(':')?;
    let minutes: f64 = minutes.trim().parse().ok()?;
    // Seconds may carry a fraction in either centiseconds or milliseconds;
    // parsing as a decimal handles both without needing to know which.
    let seconds: f64 = rest.trim().replace(',', ".").parse().ok()?;
    if !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(minutes * 60.0 + seconds)
}

/// Split a bracketed tag into timestamps and metadata. `[00:12.34]` is a time;
/// `[ar:Someone]` is metadata and must not be mistaken for one.
fn tag_time(tag: &str) -> Option<f64> {
    let body = tag.trim();
    if !body.chars().next()?.is_ascii_digit() {
        return None;
    }
    parse_clock(body)
}

fn parse_lrc(raw: &str) -> LyricsSheet {
    let mut offset_seconds = 0.0_f64;
    let mut entries: Vec<LyricsLine> = Vec::new();
    let mut any_words = false;

    for source_line in raw.lines() {
        let line = source_line.trim();
        if line.is_empty() {
            continue;
        }

        // Leading `[...]` groups: one or more timestamps, or a metadata tag.
        let mut times: Vec<f64> = Vec::new();
        let mut rest = line;
        while rest.starts_with('[') {
            let Some(close) = rest.find(']') else { break };
            let tag = &rest[1..close];
            match tag_time(tag) {
                Some(time) => times.push(time),
                None => {
                    // `[offset:+250]` shifts every timestamp, in milliseconds.
                    if let Some(value) = tag
                        .split_once(':')
                        .filter(|(key, _)| key.trim().eq_ignore_ascii_case("offset"))
                        .and_then(|(_, value)| value.trim().replace('+', "").parse::<f64>().ok())
                    {
                        offset_seconds = value / 1000.0;
                    }
                }
            }
            rest = rest[close + 1..].trim_start();
        }

        if times.is_empty() {
            continue;
        }

        let (text, words) = parse_lrc_words(rest);
        if !words.is_empty() {
            any_words = true;
        }

        // A line may carry several timestamps when the same words repeat.
        for time in times {
            entries.push(LyricsLine {
                time,
                end: None,
                text: text.clone(),
                words: words.clone(),
                agent: None,
                background: false,
            });
        }
    }

    if entries.is_empty() {
        return LyricsSheet::plain(raw);
    }

    entries.sort_by(|a, b| a.time.total_cmp(&b.time));

    // Apply offset after sorting; it shifts the whole sheet uniformly.
    if offset_seconds != 0.0 {
        for entry in &mut entries {
            entry.time = (entry.time - offset_seconds).max(0.0);
            for word in &mut entry.words {
                word.time = (word.time - offset_seconds).max(0.0);
                word.end = (word.end - offset_seconds).max(0.0);
            }
        }
    }

    close_open_ends(&mut entries);

    LyricsSheet {
        kind: if any_words {
            LyricsKind::Word
        } else {
            LyricsKind::Line
        },
        lines: entries,
        plain: raw.to_string(),
    }
}

/// Split a line body into its display text and any `<mm:ss.xx>` word timings.
///
/// A word's end is the next word's start; the final word is closed later, once
/// the following line's start time is known.
fn parse_lrc_words(body: &str) -> (String, Vec<LyricsWord>) {
    if !body.contains('<') {
        return (body.trim().to_string(), Vec::new());
    }

    let mut words: Vec<LyricsWord> = Vec::new();
    let mut text = String::new();
    let mut rest = body;
    let mut pending: Option<(f64, String)> = None;

    while let Some(open) = rest.find('<') {
        let before = &rest[..open];
        text.push_str(before);
        if let Some((_, chunk)) = pending.as_mut() {
            chunk.push_str(before);
        }

        let Some(close_rel) = rest[open..].find('>') else {
            // Unclosed marker: treat the remainder as literal text.
            text.push_str(&rest[open..]);
            if let Some((_, chunk)) = pending.as_mut() {
                chunk.push_str(&rest[open..]);
            }
            rest = "";
            break;
        };
        let close = open + close_rel;
        let tag = &rest[open + 1..close];

        match parse_clock(tag.trim()) {
            Some(time) => {
                if let Some((start, chunk)) = pending.take() {
                    push_word(&mut words, start, time, &chunk);
                }
                pending = Some((time, String::new()));
            }
            None => {
                // Not a timestamp — keep the angle brackets as text.
                let literal = &rest[open..=close];
                text.push_str(literal);
                if let Some((_, chunk)) = pending.as_mut() {
                    chunk.push_str(literal);
                }
            }
        }
        rest = &rest[close + 1..];
    }

    text.push_str(rest);
    if let Some((start, mut chunk)) = pending.take() {
        chunk.push_str(rest);
        // End is unknown here; closed by `close_open_ends`.
        push_word(&mut words, start, f64::NAN, &chunk);
    }

    (text.trim().to_string(), words)
}

fn push_word(words: &mut Vec<LyricsWord>, time: f64, end: f64, chunk: &str) {
    if chunk.trim().is_empty() {
        return;
    }
    words.push(LyricsWord {
        time,
        end,
        text: chunk.to_string(),
    });
}

/// Fill in every end time left unknown by the line-oriented formats.
fn close_open_ends(lines: &mut [LyricsLine]) {
    for index in 0..lines.len() {
        let next_start = lines.get(index + 1).map(|line| line.time);
        let line_end = lines[index]
            .end
            .or(next_start)
            .unwrap_or(lines[index].time + DEFAULT_WORD_SECONDS);
        lines[index].end = Some(line_end);

        let count = lines[index].words.len();
        for word_index in 0..count {
            if lines[index].words[word_index].end.is_nan() {
                let end = lines[index]
                    .words
                    .get(word_index + 1)
                    .map(|word| word.time)
                    .unwrap_or(line_end);
                lines[index].words[word_index].end = end;
            }
        }
        // A trailing word can still outrun its line if the file disagrees with
        // itself; clamping keeps the wipe inside the line it belongs to.
        if let Some(last) = lines[index].words.last_mut() {
            if last.end < last.time {
                last.end = last.time + DEFAULT_WORD_SECONDS;
            }
        }
    }
}

// ── TTML ──────────────────────────────────────────────────────────────────

/// TTML time expressions: `HH:MM:SS.mmm`, `MM:SS.mmm`, `12.5s`, `250ms`,
/// `1.5m`, `2h`. Frame-based forms are not used by lyric files and are ignored.
fn parse_ttml_time(raw: &str) -> Option<f64> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    if value.contains(':') {
        let parts: Vec<&str> = value.split(':').collect();
        let mut seconds = 0.0_f64;
        for part in &parts {
            seconds = seconds * 60.0 + part.trim().replace(',', ".").parse::<f64>().ok()?;
        }
        return Some(seconds);
    }
    for (suffix, scale) in [("ms", 0.001), ("s", 1.0), ("m", 60.0), ("h", 3600.0)] {
        if let Some(number) = value.strip_suffix(suffix) {
            return number.trim().parse::<f64>().ok().map(|n| n * scale);
        }
    }
    value.parse::<f64>().ok()
}

/// Collapse insignificant XML whitespace.
///
/// Pretty-printed TTML puts newlines and indentation between `<span>`
/// elements, and those arrive as genuine text nodes. Without this, a line
/// renders with the source file's indentation embedded in it. Word text is
/// deliberately left untouched — a word's own trailing space is what separates
/// it from the next one during the karaoke wipe.
fn normalize_ws(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            in_space = true;
            continue;
        }
        if in_space && !out.is_empty() {
            out.push(' ');
        }
        in_space = false;
        out.push(ch);
    }
    out
}

fn attr<'a>(node: &roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

fn parse_ttml(raw: &str) -> Option<LyricsSheet> {
    let doc = roxmltree::Document::parse(raw).ok()?;
    let mut lines: Vec<LyricsLine> = Vec::new();

    for p in doc.descendants().filter(|n| n.has_tag_name("p")) {
        let begin = attr(&p, "begin").and_then(parse_ttml_time);
        let end = attr(&p, "end").and_then(parse_ttml_time);

        let mut words: Vec<LyricsWord> = Vec::new();
        let mut background_words: Vec<LyricsWord> = Vec::new();
        let mut text = String::new();

        collect_ttml(&p, false, &mut words, &mut background_words, &mut text);

        let line_start = begin
            .or_else(|| words.first().map(|w| w.time))
            .or_else(|| background_words.first().map(|w| w.time));
        let Some(line_start) = line_start else {
            continue;
        };
        let trimmed = normalize_ws(&text);
        if trimmed.is_empty() && words.is_empty() {
            continue;
        }

        let agent = attr(&p, "agent")
            .or_else(|| attr(&p, "ttm:agent"))
            .map(str::to_string);

        lines.push(LyricsLine {
            time: line_start,
            end,
            text: trimmed,
            words,
            agent,
            background: false,
        });

        // Background vocals ride along as their own line so the UI can style
        // them separately without inventing a nested structure.
        if !background_words.is_empty() {
            let bg_text = background_words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<String>();
            lines.push(LyricsLine {
                time: background_words[0].time,
                end,
                text: normalize_ws(&bg_text),
                words: background_words,
                agent: agent_of(&p),
                background: true,
            });
        }
    }

    if lines.is_empty() {
        return None;
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    close_open_ends(&mut lines);

    let kind = if lines.iter().any(|line| !line.words.is_empty()) {
        LyricsKind::Word
    } else {
        LyricsKind::Line
    };
    Some(LyricsSheet {
        kind,
        lines,
        plain: raw.to_string(),
    })
}

fn agent_of(p: &roxmltree::Node<'_, '_>) -> Option<String> {
    attr(p, "agent")
        .or_else(|| attr(p, "ttm:agent"))
        .map(str::to_string)
}

/// Walk a `<p>`, gathering timed spans. Spans marked `ttm:role="x-bg"` and
/// everything inside them are collected separately as background vocals.
fn collect_ttml(
    node: &roxmltree::Node<'_, '_>,
    in_background: bool,
    words: &mut Vec<LyricsWord>,
    background: &mut Vec<LyricsWord>,
    text: &mut String,
) {
    for child in node.children() {
        if child.is_text() {
            let value = child.text().unwrap_or("");
            if !in_background {
                text.push_str(value);
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }

        let role = attr(&child, "role").or_else(|| attr(&child, "ttm:role"));
        let is_background = in_background || role == Some("x-bg");

        let begin = attr(&child, "begin").and_then(parse_ttml_time);
        let end = attr(&child, "end").and_then(parse_ttml_time);
        let own_text: String = child
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();

        // A span with its own timing and no timed children is one word.
        let has_timed_children = child
            .children()
            .any(|c| c.is_element() && attr(&c, "begin").is_some());

        if let (Some(begin), false) = (begin, has_timed_children) {
            if !own_text.trim().is_empty() {
                let word = LyricsWord {
                    time: begin,
                    end: end.unwrap_or(f64::NAN),
                    text: own_text.clone(),
                };
                if is_background {
                    background.push(word);
                } else {
                    words.push(word);
                    text.push_str(&own_text);
                }
                continue;
            }
        }

        collect_ttml(&child, is_background, words, background, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All fixtures use invented placeholder words, never real song lyrics.

    #[test]
    fn plain_text_has_no_timings() {
        let sheet = parse_sheet("alpha bravo\ncharlie delta");
        assert_eq!(sheet.kind, LyricsKind::Plain);
        assert!(sheet.lines.is_empty());
        assert!(sheet.plain.contains("charlie delta"));
    }

    #[test]
    fn empty_input_is_plain_not_a_panic() {
        assert_eq!(parse_sheet("").kind, LyricsKind::Plain);
        assert_eq!(parse_sheet("   \n\n ").kind, LyricsKind::Plain);
    }

    #[test]
    fn line_level_lrc_parses_times_in_order() {
        let sheet = parse_sheet("[00:12.34]alpha\n[00:05.00]bravo\n[01:03.5]charlie");
        assert_eq!(sheet.kind, LyricsKind::Line);
        let times: Vec<f64> = sheet.lines.iter().map(|l| l.time).collect();
        assert_eq!(times, vec![5.0, 12.34, 63.5], "lines must be time-ordered");
        assert_eq!(sheet.lines[0].text, "bravo");
        assert!(sheet.lines[0].words.is_empty());
    }

    #[test]
    fn centiseconds_and_milliseconds_both_parse() {
        let sheet = parse_sheet("[00:01.5]a\n[00:02.25]b\n[00:03.125]c");
        let times: Vec<f64> = sheet.lines.iter().map(|l| l.time).collect();
        assert_eq!(times, vec![1.5, 2.25, 3.125]);
    }

    #[test]
    fn metadata_tags_are_not_mistaken_for_timings() {
        let sheet = parse_sheet("[ar:Someone]\n[ti:Something]\n[00:04.00]alpha");
        assert_eq!(sheet.lines.len(), 1, "only the timed line counts");
        assert_eq!(sheet.lines[0].text, "alpha");
    }

    #[test]
    fn offset_tag_shifts_every_timestamp() {
        let sheet = parse_sheet("[offset:+500]\n[00:10.00]alpha\n[00:20.00]bravo");
        // +500ms offset pulls the sheet half a second earlier.
        assert_eq!(sheet.lines[0].time, 9.5);
        assert_eq!(sheet.lines[1].time, 19.5);
    }

    #[test]
    fn offset_never_produces_a_negative_time() {
        let sheet = parse_sheet("[offset:+5000]\n[00:01.00]alpha");
        assert_eq!(sheet.lines[0].time, 0.0);
    }

    #[test]
    fn repeated_timestamps_expand_into_separate_lines() {
        // A chorus line is often stamped several times on one row.
        let sheet = parse_sheet("[00:10.00][00:40.00][01:10.00]alpha");
        assert_eq!(sheet.lines.len(), 3);
        assert!(sheet.lines.iter().all(|l| l.text == "alpha"));
        assert_eq!(sheet.lines[2].time, 70.0);
    }

    #[test]
    fn enhanced_lrc_yields_word_timings() {
        let sheet = parse_sheet(
            "[00:10.00]<00:10.00>alpha <00:10.50>bravo <00:11.00>charlie\n[00:12.00]delta",
        );
        assert_eq!(
            sheet.kind,
            LyricsKind::Word,
            "word tags must upgrade the kind"
        );
        let line = &sheet.lines[0];
        assert_eq!(line.words.len(), 3);
        assert_eq!(line.words[0].text.trim(), "alpha");
        assert_eq!(line.words[0].time, 10.0);
        // Each word ends where the next begins.
        assert_eq!(line.words[0].end, 10.5);
        assert_eq!(line.words[1].end, 11.0);
        // The last word runs to the end of its line, i.e. the next line's start.
        assert_eq!(line.words[2].end, 12.0);
        assert_eq!(
            line.text, "alpha bravo charlie",
            "display text keeps the words"
        );
    }

    #[test]
    fn a_line_without_word_tags_still_works_alongside_ones_that_have_them() {
        let sheet = parse_sheet("[00:01.00]<00:01.00>alpha\n[00:05.00]bravo charlie");
        assert_eq!(sheet.kind, LyricsKind::Word);
        assert_eq!(sheet.lines[1].words.len(), 0);
        assert_eq!(sheet.lines[1].text, "bravo charlie");
    }

    #[test]
    fn unclosed_word_marker_degrades_to_text() {
        let sheet = parse_sheet("[00:01.00]alpha <00:02.00 bravo");
        // Must not panic, and must not swallow the words.
        assert!(sheet.lines[0].text.contains("alpha"));
        assert!(sheet.lines[0].text.contains("bravo"));
    }

    #[test]
    fn angle_brackets_that_are_not_timings_stay_as_text() {
        let sheet = parse_sheet("[00:01.00]alpha <not a time> bravo");
        assert!(sheet.lines[0].words.is_empty());
        assert!(sheet.lines[0].text.contains("<not a time>"));
    }

    #[test]
    fn nonsense_input_falls_back_to_plain() {
        let sheet = parse_sheet("[[[[\n]]]]\nnothing here");
        assert_eq!(sheet.kind, LyricsKind::Plain);
    }

    // ── TTML ─────────────────────────────────────────────────────────────

    const TTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
  <body>
    <div>
      <p begin="00:00:10.000" end="00:00:13.000" ttm:agent="v1">
        <span begin="00:00:10.000" end="00:00:10.500">alpha </span>
        <span begin="00:00:10.500" end="00:00:11.200">bravo</span>
      </p>
      <p begin="00:00:14.000" end="00:00:16.000" ttm:agent="v2">
        <span begin="00:00:14.000" end="00:00:15.000">charlie</span>
        <span ttm:role="x-bg">
          <span begin="00:00:15.100" end="00:00:15.800">delta</span>
        </span>
      </p>
    </div>
  </body>
</tt>"#;

    #[test]
    fn ttml_produces_word_timings() {
        let sheet = parse_sheet(TTML);
        assert_eq!(sheet.kind, LyricsKind::Word);
        let first = &sheet.lines[0];
        assert_eq!(first.time, 10.0);
        assert_eq!(first.end, Some(13.0));
        assert_eq!(first.words.len(), 2);
        assert_eq!(first.words[0].text.trim(), "alpha");
        assert_eq!(first.words[0].end, 10.5);
        assert_eq!(first.text, "alpha bravo");
    }

    #[test]
    fn ttml_keeps_voice_attribution_for_duets() {
        let sheet = parse_sheet(TTML);
        assert_eq!(sheet.lines[0].agent.as_deref(), Some("v1"));
        let second = sheet.lines.iter().find(|l| l.time == 14.0).unwrap();
        assert_eq!(second.agent.as_deref(), Some("v2"));
    }

    #[test]
    fn ttml_background_vocals_become_their_own_flagged_line() {
        let sheet = parse_sheet(TTML);
        let bg: Vec<_> = sheet.lines.iter().filter(|l| l.background).collect();
        assert_eq!(bg.len(), 1, "one x-bg group in the fixture");
        assert_eq!(bg[0].text, "delta");
        assert_eq!(bg[0].time, 15.1);

        // The background words must NOT leak into the lead vocal line.
        let lead = sheet.lines.iter().find(|l| l.time == 14.0).unwrap();
        assert_eq!(lead.text, "charlie");
        assert!(!lead.text.contains("delta"));
    }

    #[test]
    fn ttml_indentation_does_not_leak_into_the_line() {
        // Pretty-printed TTML puts newlines between spans; they must not show
        // up as gaps in the rendered line.
        let sheet = parse_sheet(TTML);
        for line in &sheet.lines {
            assert!(!line.text.contains('\n'), "newline leaked: {:?}", line.text);
            assert!(
                !line.text.contains("  "),
                "double space leaked: {:?}",
                line.text
            );
        }
    }

    #[test]
    fn ttml_accepts_every_time_expression_it_may_meet() {
        assert_eq!(parse_ttml_time("00:00:12.500"), Some(12.5));
        assert_eq!(parse_ttml_time("01:02.250"), Some(62.25));
        assert_eq!(parse_ttml_time("12.5s"), Some(12.5));
        assert_eq!(parse_ttml_time("250ms"), Some(0.25));
        assert_eq!(parse_ttml_time("1.5m"), Some(90.0));
        assert_eq!(parse_ttml_time("2h"), Some(7200.0));
        assert_eq!(parse_ttml_time(""), None);
        assert_eq!(parse_ttml_time("nonsense"), None);
    }

    #[test]
    fn ttml_without_span_timings_is_line_level() {
        let doc = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="5s" end="8s">alpha bravo</p>
        </div></body></tt>"#;
        let sheet = parse_sheet(doc);
        assert_eq!(sheet.kind, LyricsKind::Line);
        assert_eq!(sheet.lines[0].text, "alpha bravo");
        assert_eq!(sheet.lines[0].time, 5.0);
    }

    #[test]
    fn malformed_xml_shows_its_text_instead_of_failing() {
        let broken = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="1s">alpha"#;
        let sheet = parse_sheet(broken);
        assert_eq!(sheet.kind, LyricsKind::Plain);
        assert!(sheet.plain.contains("alpha"));
    }

    #[test]
    fn every_word_end_is_resolved_and_ordered() {
        // No end time may survive parsing, or the renderer divides by NaN.
        for input in [TTML, "[00:01.00]<00:01.00>alpha <00:01.40>bravo"] {
            let sheet = parse_sheet(input);
            for line in &sheet.lines {
                assert!(line.end.is_some(), "line end unresolved");
                for word in &line.words {
                    assert!(word.time.is_finite(), "word start not finite");
                    assert!(word.end.is_finite(), "word end not finite");
                    assert!(word.end >= word.time, "word ends before it starts");
                }
            }
        }
    }
}

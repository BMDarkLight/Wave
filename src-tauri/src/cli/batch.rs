// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Many requests in one call: `wave batch`, and `-` in place of a track.
//!
//! Each request runs as its own `wave --json` process, so it behaves exactly
//! as the same command typed on its own would, including its exit status.
//! Commands end the process when they finish, which is why they are not run
//! in a loop here. Results come back as JSON lines; the shapes are documented
//! in docs/cli.md.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

use crate::cli::{json, ui};

/// One request: the arguments to run, and the caller's id for it, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: Option<Value>,
    pub args: Vec<String>,
}

/// Split a command line the way a POSIX shell would for plain words and
/// quotes: single quotes keep everything literally, double quotes allow
/// `\"` and `\\`, and a backslash outside quotes escapes the next character.
/// No variables, globs or operators.
pub fn shell_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("unterminated single quote".to_string()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("unterminated double quote".to_string()),
                        },
                        Some(c) => word.push(c),
                        None => return Err("unterminated double quote".to_string()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some(c) => word.push(c),
                    None => return Err("trailing backslash".to_string()),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// One request from its JSON form: an array of arguments, or an object with
/// `args` and an optional `id`.
fn request_from_json(value: Value) -> Result<Request, String> {
    let (id, args) = match value {
        Value::Array(args) => (None, args),
        Value::Object(mut fields) => {
            let id = fields.remove("id");
            match fields.remove("args") {
                Some(Value::Array(args)) => (id, args),
                _ => return Err("a request object needs \"args\": [..]".to_string()),
            }
        }
        _ => return Err("a request is an array of arguments or an object".to_string()),
    };
    let args = args
        .into_iter()
        .map(|arg| match arg {
            Value::String(s) => Ok(s),
            other => Err(format!("arguments must be strings, got {other}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Request { id, args })
}

/// Read batch input. The whole input may be one JSON array of requests;
/// otherwise each line is a request, as a JSON array or object, or as a
/// command line. Blank lines and lines starting with `#` are skipped. A line
/// that cannot be read yields an error in its place, so the rest still run
/// and every result keeps its position.
pub fn parse_requests(input: &str) -> Vec<Result<Request, String>> {
    let trimmed = input.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('[') {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed) {
            // An array of strings is a single request, not a list of them.
            if items.iter().all(Value::is_string) {
                return vec![request_from_json(Value::Array(items))];
            }
            return items.into_iter().map(request_from_json).collect();
        }
    }
    trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            if line.starts_with('[') || line.starts_with('{') {
                serde_json::from_str(line)
                    .map_err(|e| format!("not valid JSON: {e}"))
                    .and_then(request_from_json)
            } else {
                shell_words(line).map(|args| Request { id: None, args })
            }
        })
        .collect()
}

/// Read a list of items given on stdin for `-`: a JSON array of strings,
/// NUL-separated items (for paths that contain newlines), or one per line.
pub fn parse_items(input: &str) -> Result<Vec<String>, String> {
    let trimmed = input.trim_start_matches('\u{feff}');
    if trimmed.trim_start().starts_with('[') {
        if let Ok(items) = serde_json::from_str::<Vec<String>>(trimmed.trim()) {
            return Ok(items);
        }
        // A list of paths whose first one starts with '[' is still a list.
    }
    let items: Vec<String> = if trimmed.contains('\0') {
        trimmed.split('\0').map(str::to_string).collect()
    } else {
        trimmed
            .lines()
            .map(|line| line.trim_end_matches('\r').to_string())
            .collect()
    };
    Ok(items.into_iter().filter(|item| !item.is_empty()).collect())
}

/// Why a request cannot run inside a batch, if it cannot.
fn refuse(args: &[String]) -> Option<&'static str> {
    let command = args.iter().find(|arg| !arg.starts_with('-'));
    if args.is_empty() {
        Some("an empty request has nothing to run")
    } else if command.is_some_and(|c| c == "batch") {
        Some("a batch cannot contain another batch")
    } else if args.iter().any(|arg| arg == "--watch") {
        Some("--watch never ends, so it cannot run inside a batch")
    } else if args.iter().any(|arg| arg == "-") {
        Some("'-' reads stdin, which a batch request does not have")
    } else {
        None
    }
}

/// The outcome of one request, ready to print as a JSON line.
fn outcome(index: usize, id: &Option<Value>, input: Option<&str>, args: &[String]) -> Value {
    let mut line = json!({ "index": index, "schema_version": json::SCHEMA_VERSION });
    if let Some(id) = id {
        line["id"] = id.clone();
    }
    if let Some(input) = input {
        line["input"] = Value::String(input.to_string());
    }
    let fail = |mut line: Value, code: i32, error: String| {
        line["ok"] = Value::Bool(false);
        line["code"] = Value::from(code);
        line["kind"] = Value::String(ui::error_kind(code).to_string());
        line["error"] = Value::String(error);
        line
    };
    if let Some(reason) = refuse(args) {
        return fail(line, ui::EXIT_USAGE, reason.to_string());
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            return fail(
                line,
                ui::EXIT_GENERAL,
                format!("Cannot find wave itself: {e}"),
            )
        }
    };
    let output = Command::new(exe)
        .arg("--json")
        .args(args.iter().filter(|arg| *arg != "--json"))
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(e) => {
            return fail(
                line,
                ui::EXIT_GENERAL,
                format!("Could not run request: {e}"),
            )
        }
    };
    let code = output.status.code().unwrap_or(ui::EXIT_GENERAL);
    if code == 0 {
        let stdout = String::from_utf8_lossy(&output.stdout);
        line["ok"] = Value::Bool(true);
        line["code"] = Value::from(0);
        // Every JSON command prints JSON; `completions` prints a script.
        line["result"] =
            serde_json::from_str(&stdout).unwrap_or_else(|_| Value::String(stdout.into_owned()));
        return line;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let error = serde_json::from_str::<Value>(&stderr)
        .ok()
        .and_then(|e| e["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| stderr.trim().to_string());
    fail(line, code, error)
}

/// Print one outcome: a JSON line under --json, a status line otherwise.
fn print(line: &Value) {
    let ui = ui::current();
    let text = if ui.json {
        line.to_string()
    } else {
        let label = line["input"]
            .as_str()
            .map(|input| format!("{input}: "))
            .unwrap_or_default();
        if line["ok"] == Value::Bool(true) {
            let message = line["result"]["message"].as_str().unwrap_or("Done.");
            ui.paint(ui::style::OK, &format!("{} {label}{message}", ui.glyphs.ok))
        } else {
            let error = line["error"].as_str().unwrap_or("Failed.");
            ui.paint(ui::style::ERR, &format!("{} {label}{error}", ui.glyphs.err))
        }
    };
    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{text}")
        .and_then(|_| stdout.flush())
        .is_err()
    {
        // The reader went away; nothing left to tell.
        std::process::exit(crate::cli::EXIT_BROKEN_PIPE);
    }
}

fn read_stdin() -> String {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        ui::fail(format!("Could not read stdin: {e}"), None, ui::EXIT_GENERAL);
    }
    input
}

/// `wave batch`: run every request on stdin, in order, one result line each.
/// Exits 0 when every request succeeded and 1 when any failed.
pub fn run(stop_on_error: bool) -> ! {
    let mut all_ok = true;
    for (index, request) in parse_requests(&read_stdin()).into_iter().enumerate() {
        let line = match request {
            Ok(request) => outcome(index, &request.id, None, &request.args),
            Err(reason) => json!({
                "index": index,
                "schema_version": json::SCHEMA_VERSION,
                "ok": false,
                "code": ui::EXIT_USAGE,
                "kind": ui::error_kind(ui::EXIT_USAGE),
                "error": format!("Could not read request: {reason}"),
            }),
        };
        let ok = line["ok"] == Value::Bool(true);
        print(&line);
        all_ok &= ok;
        if !ok && stop_on_error {
            break;
        }
    }
    std::process::exit(if all_ok { 0 } else { ui::EXIT_GENERAL })
}

/// The commands that take `-` in place of a track, as the words that name
/// them. The `-` must be their track argument.
const LIST_COMMANDS: &[&[&str]] = &[
    &["tracks", "info"],
    &["tracks", "add"],
    &["tracks", "remove"],
    &["queue", "add"],
    &["queue", "next"],
    &["playlists", "add-track"],
    &["playlists", "remove-track"],
    &["favorite", "add"],
    &["favorite", "remove"],
    &["metadata", "get"],
    &["metadata", "set"],
];

/// Whether `args` (without the program name) is a command that takes `-`
/// for its track and was given it.
pub fn wants_stdin_list(args: &[String]) -> bool {
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    args.iter().any(|arg| arg == "-")
        && LIST_COMMANDS
            .iter()
            .any(|command| words.len() >= command.len() && words[..command.len()] == **command)
}

/// Run a command once for every item on stdin, with the item in place of
/// `-`. Results come out in input order, as for `wave batch`, each with the
/// item it was for under `input`.
pub fn run_list(args: &[String]) -> ! {
    let dashes = args.iter().filter(|arg| *arg == "-").count();
    if dashes != 1 {
        ui::fail("Only one argument can be '-'.", None, ui::EXIT_USAGE);
    }
    let items = parse_items(&read_stdin()).unwrap_or_else(|e| ui::fail(e, None, ui::EXIT_USAGE));
    let substitute = |item: &str| -> Vec<String> {
        args.iter()
            .map(|arg| {
                if arg == "-" {
                    item.to_string()
                } else {
                    arg.clone()
                }
            })
            .collect()
    };

    // Each `queue next` goes straight after the current track, so the items
    // are queued last to first to come out playing in the order given. The
    // results are still reported in input order.
    let reverse = args.iter().any(|a| a == "queue") && args.iter().any(|a| a == "next");
    let mut all_ok = true;
    if reverse {
        let mut lines: Vec<Value> = items
            .iter()
            .enumerate()
            .rev()
            .map(|(index, item)| outcome(index, &None, Some(item), &substitute(item)))
            .collect();
        lines.reverse();
        for line in &lines {
            all_ok &= line["ok"] == Value::Bool(true);
            print(line);
        }
    } else {
        for (index, item) in items.iter().enumerate() {
            let line = outcome(index, &None, Some(item), &substitute(item));
            all_ok &= line["ok"] == Value::Bool(true);
            print(&line);
        }
    }
    std::process::exit(if all_ok { 0 } else { ui::EXIT_GENERAL })
}

/// The items on stdin, for a command that takes a list itself (`tracks
/// import -`) rather than running once per item.
pub fn stdin_items() -> Vec<String> {
    parse_items(&read_stdin()).unwrap_or_else(|e| ui::fail(e, None, ui::EXIT_USAGE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        shell_words(line).unwrap()
    }

    #[test]
    fn command_lines_split_like_a_shell() {
        assert_eq!(words("queue add a.mp3"), ["queue", "add", "a.mp3"]);
        assert_eq!(
            words(r#"playlists create "Road Trip""#),
            ["playlists", "create", "Road Trip"]
        );
        assert_eq!(
            words("tracks import '/My Music/'"),
            ["tracks", "import", "/My Music/"]
        );
        assert_eq!(words(r#"a "say \"hi\"" b\ c"#), ["a", r#"say "hi""#, "b c"]);
        assert_eq!(words(r#"x """#), ["x", ""]);
        assert!(shell_words("a 'open").is_err());
        assert!(shell_words(r#"a "open"#).is_err());
    }

    #[test]
    fn requests_come_as_lines_or_one_array() {
        let lines = parse_requests(
            "# a comment\n\
             tracks list\n\
             [\"queue\", \"add\", \"a b.mp3\"]\n\
             \n\
             {\"id\": 7, \"args\": [\"playlists\", \"list\"]}\n\
             {\"args\": 3}\n",
        );
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].as_ref().unwrap().args, ["tracks", "list"]);
        assert_eq!(lines[1].as_ref().unwrap().args, ["queue", "add", "a b.mp3"]);
        assert_eq!(lines[2].as_ref().unwrap().id, Some(json!(7)));
        assert!(lines[3].is_err());

        let array = parse_requests(r#"[["tracks","list"], {"id":"x","args":["stats","summary"]}]"#);
        assert_eq!(array.len(), 2);
        assert_eq!(array[1].as_ref().unwrap().id, Some(json!("x")));

        // One array of strings is one request.
        let single = parse_requests(r#"["tracks", "list"]"#);
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].as_ref().unwrap().args, ["tracks", "list"]);
    }

    #[test]
    fn stdin_lists_come_as_lines_nul_or_json() {
        assert_eq!(
            parse_items("a.mp3\r\nb c.mp3\n\n").unwrap(),
            ["a.mp3", "b c.mp3"]
        );
        assert_eq!(
            parse_items("a\nb.mp3\0c.mp3\0").unwrap(),
            ["a\nb.mp3", "c.mp3"]
        );
        assert_eq!(
            parse_items(r#"["x.mp3", "y.mp3"]"#).unwrap(),
            ["x.mp3", "y.mp3"]
        );
        // A path that merely starts with a bracket is still a line.
        assert_eq!(
            parse_items("[Live] a.mp3\nb.mp3").unwrap(),
            ["[Live] a.mp3", "b.mp3"]
        );
    }

    #[test]
    fn only_the_track_commands_take_a_dash() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(wants_stdin_list(&args("queue add -")));
        assert!(wants_stdin_list(&args("--json playlists add-track Mix -")));
        assert!(wants_stdin_list(&args("metadata set - --genre Jazz")));
        assert!(!wants_stdin_list(&args("queue add a.mp3")));
        assert!(!wants_stdin_list(&args("playback seek -")));
        assert!(!wants_stdin_list(&args("tracks import -")));
    }

    #[test]
    fn nested_and_endless_requests_are_refused() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert!(refuse(&args("batch")).is_some());
        assert!(refuse(&args("--json batch")).is_some());
        assert!(refuse(&args("now --watch")).is_some());
        assert!(refuse(&args("queue add -")).is_some());
        assert!(refuse(&[]).is_some());
        assert!(refuse(&args("tracks list")).is_none());
    }
}

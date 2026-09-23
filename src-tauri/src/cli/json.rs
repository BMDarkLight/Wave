// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Machine-readable output.
//!
//! Read commands print their payload directly; mutations print an ok
//! envelope. Errors go to stderr in the same shape, so a script gets one
//! contract whichever way a command ends.

use serde_json::{json, Value};

pub fn ok_payload(extra: Value) -> Value {
    let mut payload = json!({ "ok": true });
    match extra {
        Value::Object(fields) => {
            for (key, value) in fields {
                // The envelope owns "ok"; a field of the same name must not
                // flip its verdict.
                if key != "ok" {
                    payload[key] = value;
                }
            }
        }
        // A bare value still needs somewhere to live.
        other => payload["result"] = other,
    }
    payload
}

pub fn error_payload(message: &str, code: i32) -> Value {
    json!({ "ok": false, "error": message, "code": code })
}

/// Print a value as JSON and exit successfully.
pub fn emit<T: serde::Serialize>(value: &T) -> ! {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => emit_error(&format!("Failed to serialize output: {e}"), 1),
    }
    std::process::exit(0)
}

/// Under `--json`, print `value` and exit; otherwise do nothing, so the
/// caller carries on to its formatted output. Called before any empty-state
/// message, so an empty list comes out as `[]` rather than prose.
pub fn maybe_emit<T: serde::Serialize>(value: &T) {
    if crate::cli::ui::current().json {
        emit(value);
    }
}

/// Print an ok envelope for a command that changed something, and exit.
pub fn emit_ok(extra: Value) -> ! {
    println!("{}", ok_payload(extra));
    std::process::exit(0)
}

/// Print a JSON error envelope to stderr and exit.
pub fn emit_error(message: &str, code: i32) -> ! {
    eprintln!("{}", error_payload(message, code));
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_envelope_merges_extra_fields() {
        let payload = ok_payload(json!({ "imported": 3 }));
        assert_eq!(payload["ok"], json!(true));
        assert_eq!(payload["imported"], json!(3));
    }

    #[test]
    fn ok_envelope_survives_a_non_object_extra() {
        let payload = ok_payload(json!("done"));
        assert_eq!(payload["ok"], json!(true));
        assert_eq!(payload["result"], json!("done"));
    }

    #[test]
    fn extra_fields_cannot_overwrite_ok() {
        // A payload that happens to carry its own "ok" must not flip the
        // envelope's verdict.
        let payload = ok_payload(json!({ "ok": false, "n": 1 }));
        assert_eq!(payload["ok"], json!(true));
        assert_eq!(payload["n"], json!(1));
    }

    #[test]
    fn error_envelope_carries_the_exit_code() {
        let payload = error_payload("nope", 3);
        assert_eq!(payload["ok"], json!(false));
        assert_eq!(payload["error"], json!("nope"));
        assert_eq!(payload["code"], json!(3));
    }
}

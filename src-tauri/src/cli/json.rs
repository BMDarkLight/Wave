// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Machine-readable output. Only the error envelope exists so far.

use serde_json::{json, Value};

pub fn error_payload(message: &str, code: i32) -> Value {
    json!({ "ok": false, "error": message, "code": code })
}

/// Print a JSON error envelope to stderr and exit.
pub fn emit_error(message: &str, code: i32) -> ! {
    eprintln!("{}", error_payload(message, code));
    std::process::exit(code)
}

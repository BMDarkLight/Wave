// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! What the desktop app does when its library database will not open.
//!
//! The CLI reports the error and leaves the file alone. The app has no
//! terminal to report to, so it asks: when SQLite says the file is damaged
//! or is not a database at all, the user can move it aside and start with an
//! empty library, or quit. Any other failure (permissions, a full disk) only
//! gets the quit choice, since moving the file would not fix it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, ErrorCode, OpenFlags};
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::library::Library;

const START_EMPTY: &str = "Start with an Empty Library";
const QUIT: &str = "Quit";

/// SQLite keeps these next to the database while it is open in WAL mode.
const SIDECAR_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Open the library, or walk the user through recovering from a database
/// that will not open. Never returns an error: if the library cannot be
/// opened and the user does not choose to start over, the process exits.
pub fn open_library_or_recover(app: &AppHandle) -> Library {
    let error = match Library::new(app) {
        Ok(library) => return library,
        Err(error) => error,
    };
    tracing::error!("{error}");

    let db_path = match crate::app_paths::app_data_dir(app) {
        Ok(dir) => dir.join("wave-library.sqlite"),
        Err(_) => quit_with_error(app, &error),
    };
    if !is_corrupt_database(&db_path) {
        quit_with_error(app, &format!("{error}\n\n{}", db_path.display()));
    }

    let message = format!(
        "Wave could not read its library database. The file is damaged or is not a \
         Wave library:\n\n{}\n\nYou can move it aside and start with an empty library. \
         Your music files are not touched, and the old file is kept next to the new one. \
         Or quit and deal with the file yourself.",
        db_path.display()
    );
    let start_empty = app
        .dialog()
        .message(message)
        .title("Wave Library Is Unreadable")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            START_EMPTY.to_string(),
            QUIT.to_string(),
        ))
        .blocking_show();
    if !start_empty {
        tracing::info!("quitting at the user's request, library database left in place");
        std::process::exit(1);
    }

    let moved_to = match move_aside(&db_path, SystemTime::now()) {
        Ok(path) => path,
        Err(move_error) => quit_with_error(app, &move_error),
    };
    tracing::warn!(
        "moved unreadable library database to {}",
        moved_to.display()
    );
    match Library::new(app) {
        Ok(library) => {
            app.dialog()
                .message(format!(
                    "Wave started with an empty library. The old database was saved as:\n\n{}",
                    moved_to.display()
                ))
                .title("Wave Library Was Reset")
                .kind(MessageDialogKind::Info)
                .blocking_show();
            library
        }
        Err(error) => quit_with_error(app, &error),
    }
}

/// Show `detail` in an error dialog with a single Quit button, then exit.
fn quit_with_error(app: &AppHandle, detail: &str) -> ! {
    tracing::error!("{detail}");
    app.dialog()
        .message(format!("Wave could not open its library.\n\n{detail}"))
        .title("Wave Cannot Start")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCustom(QUIT.to_string()))
        .blocking_show();
    std::process::exit(1);
}

/// True when SQLite rejects the file at `db_path` as damaged or as not a
/// database. False when the file is readable, missing, or fails for some
/// other reason, since moving it aside would not help with those.
pub fn is_corrupt_database(db_path: &Path) -> bool {
    // No CREATE flag: a missing file is not a corrupt one.
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = match Connection::open_with_flags(db_path, flags) {
        Ok(connection) => connection,
        Err(error) => return is_corruption(&error),
    };
    match connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0)) {
        Ok(result) => result != "ok",
        Err(error) => is_corruption(&error),
    }
}

fn is_corruption(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

/// Rename the database at `db_path` to `<name>.corrupt-<UTC timestamp>`,
/// taking its `-wal` and `-shm` files along so SQLite does not replay them
/// into the fresh database. Returns the new path of the database file.
pub fn move_aside(db_path: &Path, now: SystemTime) -> Result<PathBuf, String> {
    let file_name = db_path
        .file_name()
        .ok_or_else(|| format!("Not a file path: {}", db_path.display()))?
        .to_string_lossy()
        .into_owned();
    let stamp = utc_stamp(now);

    // A second recovery in the same second must not overwrite the first.
    let mut target_name = format!("{file_name}.corrupt-{stamp}");
    let mut attempt = 1;
    while db_path.with_file_name(&target_name).exists() {
        attempt += 1;
        target_name = format!("{file_name}.corrupt-{stamp}-{attempt}");
    }
    let target = db_path.with_file_name(&target_name);

    std::fs::rename(db_path, &target).map_err(|error| {
        format!(
            "Failed to move {} to {}: {error}",
            db_path.display(),
            target.display()
        )
    })?;
    for suffix in SIDECAR_SUFFIXES {
        let sidecar = db_path.with_file_name(format!("{file_name}{suffix}"));
        if sidecar.exists() {
            let sidecar_target = db_path.with_file_name(format!("{target_name}{suffix}"));
            std::fs::rename(&sidecar, &sidecar_target).map_err(|error| {
                format!(
                    "Failed to move {} to {}: {error}",
                    sidecar.display(),
                    sidecar_target.display()
                )
            })?;
        }
    }
    Ok(target)
}

/// `YYYYMMDD-HHMMSS` in UTC.
fn utc_stamp(now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time_of_day = seconds % 86_400;
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        time_of_day / 3600,
        time_of_day % 3600 / 60,
        time_of_day % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day), after
/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wave-test-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn stamp_is_utc_calendar_time() {
        assert_eq!(utc_stamp(at(0)), "19700101-000000");
        assert_eq!(utc_stamp(at(951_782_400)), "20000229-000000");
        assert_eq!(utc_stamp(at(1_791_900_930)), "20261013-141530");
    }

    #[test]
    fn random_bytes_are_corrupt() {
        let dir = temp_dir();
        let db = dir.join("wave-library.sqlite");
        let noise: Vec<u8> = (0..8192u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        std::fs::write(&db, noise).unwrap();
        assert!(is_corrupt_database(&db));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn healthy_and_missing_databases_are_not_corrupt() {
        let dir = temp_dir();
        let db = dir.join("wave-library.sqlite");
        assert!(!is_corrupt_database(&db));
        assert!(!db.exists(), "the check must not create a missing database");

        let library = Library::new_with_path(&db).unwrap();
        drop(library);
        assert!(!is_corrupt_database(&db));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn move_aside_renames_the_database_and_its_sidecars() {
        let dir = temp_dir();
        let db = dir.join("wave-library.sqlite");
        std::fs::write(&db, b"not a database").unwrap();
        std::fs::write(dir.join("wave-library.sqlite-wal"), b"wal").unwrap();
        std::fs::write(dir.join("wave-library.sqlite-shm"), b"shm").unwrap();

        let moved = move_aside(&db, at(1_791_900_930)).unwrap();

        assert_eq!(
            moved,
            dir.join("wave-library.sqlite.corrupt-20261013-141530")
        );
        assert_eq!(std::fs::read(&moved).unwrap(), b"not a database");
        assert_eq!(
            std::fs::read(dir.join("wave-library.sqlite.corrupt-20261013-141530-wal")).unwrap(),
            b"wal"
        );
        assert_eq!(
            std::fs::read(dir.join("wave-library.sqlite.corrupt-20261013-141530-shm")).unwrap(),
            b"shm"
        );
        assert!(!db.exists());
        assert!(!dir.join("wave-library.sqlite-wal").exists());
        assert!(!dir.join("wave-library.sqlite-shm").exists());

        // A fresh library opens where the old one was.
        Library::new_with_path(&db).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn move_aside_never_overwrites_an_earlier_copy() {
        let dir = temp_dir();
        let db = dir.join("wave-library.sqlite");
        std::fs::write(&db, b"first").unwrap();
        let first = move_aside(&db, at(1_791_900_930)).unwrap();
        std::fs::write(&db, b"second").unwrap();
        let second = move_aside(&db, at(1_791_900_930)).unwrap();

        assert_ne!(first, second);
        assert_eq!(
            second,
            dir.join("wave-library.sqlite.corrupt-20261013-141530-2")
        );
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

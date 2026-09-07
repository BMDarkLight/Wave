// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app_paths::{data_dir, instance_lock_path};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstanceMode {
    Gui,
    Daemon,
}

#[derive(Debug, Serialize, Deserialize)]
struct InstanceLockData {
    pid: u32,
    mode: InstanceMode,
}

/// Held for the lifetime of the primary process; releases the lock on drop.
pub struct InstanceGuard {
    _file: std::fs::File,
}

fn lock_path() -> PathBuf {
    instance_lock_path()
}

/// Acquire the primary-instance lock, or return an error if another live process holds it.
pub fn try_acquire(mode: InstanceMode) -> Result<InstanceGuard, String> {
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path())
        .map_err(|e| format!("Failed to open instance lock: {e}"))?;

    file.try_lock_exclusive()
        .map_err(|_| already_running_message())?;

    let data = InstanceLockData {
        pid: std::process::id(),
        mode,
    };
    file.set_len(0).map_err(|e| e.to_string())?;
    write!(
        file,
        "{}",
        serde_json::to_string(&data).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;

    Ok(InstanceGuard { _file: file })
}

/// Whether a primary Wave instance (GUI or daemon) appears to be running.
pub fn primary_is_running() -> bool {
    read_lock_info()
        .map(|(pid, _)| is_process_alive(pid))
        .unwrap_or(false)
}

/// Whether the Tauri GUI instance holds the primary lock.
pub fn gui_is_running() -> bool {
    read_lock_info()
        .map(|(pid, mode)| mode == InstanceMode::Gui && is_process_alive(pid))
        .unwrap_or(false)
}

/// Whether only the CLI playback daemon holds the primary lock.
pub fn daemon_is_running() -> bool {
    read_lock_info()
        .map(|(pid, mode)| mode == InstanceMode::Daemon && is_process_alive(pid))
        .unwrap_or(false)
}

pub fn already_running_message() -> String {
    if let Some((pid, mode)) = read_lock_info() {
        if is_process_alive(pid) {
            let kind = match mode {
                InstanceMode::Gui => "desktop app",
                InstanceMode::Daemon => "CLI playback daemon",
            };
            return format!(
                "Wave is already running ({kind}, pid {pid}). Quit the existing instance first."
            );
        }
    }
    "Wave is already running. Quit the existing instance first.".to_string()
}

fn read_lock_info() -> Option<(u32, InstanceMode)> {
    let contents = std::fs::read_to_string(lock_path()).ok()?;
    let data: InstanceLockData = serde_json::from_str(contents.trim()).ok()?;
    Some((data.pid, data.mode))
}

trait FileLockExclusive {
    fn try_lock_exclusive(&self) -> Result<(), std::io::Error>;
}

impl FileLockExclusive for std::fs::File {
    fn try_lock_exclusive(&self) -> Result<(), std::io::Error> {
        use fs4::fs_std::FileExt;
        FileExt::try_lock_exclusive(self)
    }
}

pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc_ext::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
mod libc_ext {
    extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

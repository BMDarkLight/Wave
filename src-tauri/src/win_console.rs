// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Console attachment for the Windows GUI-subsystem build.
//!
//! Release builds are linked as `windows_subsystem = "windows"` so launching the
//! app from Explorer or a shortcut never flashes a console window. The trade-off
//! is that such a process starts with no standard handles at all, so a CLI run
//! from a terminal would print into the void.
//!
//! [`attach_parent_terminal`] re-binds the standard handles to the terminal that
//! launched us. It is a no-op on every other platform.

/// Re-attach stdio to the terminal that spawned this process.
///
/// Call once, before anything writes to stdout/stderr.
#[cfg(not(target_os = "windows"))]
pub fn attach_parent_terminal() {}

/// Re-attach stdio to the terminal that spawned this process.
///
/// Call once, before anything writes to stdout/stderr.
///
/// Handles that the parent already supplied — a pipe from `wave … | more`, a
/// file from `wave … > out.txt` — are left untouched. Overwriting those would
/// silently redirect the output back to the console and break redirection, so
/// each handle is rebound only when it is genuinely missing.
#[cfg(target_os = "windows")]
pub fn attach_parent_terminal() {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    /// A NUL-terminated UTF-16 device name, e.g. `CONOUT$`.
    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// True when the parent already handed us a usable handle (console, pipe or file).
    fn inherited(slot: STD_HANDLE) -> bool {
        let handle = unsafe { GetStdHandle(slot) };
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }

    /// Point `slot` at a console device, unless the parent already redirected it.
    fn bind(slot: STD_HANDLE, device: &str, access: u32) {
        if inherited(slot) {
            return;
        }
        let device = wide(device);
        let handle = unsafe {
            CreateFileW(
                device.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut::<c_void>(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            unsafe { SetStdHandle(slot, handle) };
        }
    }

    let missing = !inherited(STD_OUTPUT_HANDLE) || !inherited(STD_ERROR_HANDLE);
    if !missing {
        return;
    }

    // Fails when there is no parent console — a shortcut, a service, `start`ed
    // detached — in which case there is nothing to print to and we leave the
    // handles as they are.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }

    bind(STD_OUTPUT_HANDLE, "CONOUT$", GENERIC_WRITE);
    bind(STD_ERROR_HANDLE, "CONOUT$", GENERIC_WRITE);
    bind(STD_INPUT_HANDLE, "CONIN$", GENERIC_READ);
}

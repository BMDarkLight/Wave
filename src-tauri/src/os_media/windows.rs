//! Windows media integration: AppUserModelID, SMTC flyout, taskbar thumbnail controls.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};
use windows::core::{HSTRING, PCWSTR};
use windows::Foundation::{EventRegistrationToken, TimeSpan, TypedEventHandler};
use windows::Media::*;
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject,
    DrawTextW, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    FW_NORMAL, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
use windows::Win32::UI::Shell::THBF_DISABLED;
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, RemoveWindowSubclass, SetCurrentProcessExplicitAppUserModelID,
    SetWindowSubclass, TaskbarList, THBF_ENABLED, THBN_CLICKED, THB_FLAGS, THB_ICON, THB_TOOLTIP,
    THUMBBUTTON, THUMBBUTTONMASK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, PostMessageW,
    RegisterWindowMessageW, SM_CXICON, WM_COMMAND, WM_NCDESTROY, WM_USER,
};

use crate::media_controls::TrackMetadata;

// ── App identity ──────────────────────────────────────────────────────────────

pub fn set_app_user_model_id(app_id: &str) {
    let id = HSTRING::from(app_id);
    let result = unsafe { SetCurrentProcessExplicitAppUserModelID(&id) };
    if result.is_err() {
        tracing::warn!("Failed to set AppUserModelID: {result:?}");
    }
}

// ── Public facade used by media_controls.rs ───────────────────────────────────

pub struct WindowsMedia {
    smtc: SmtcSession,
    taskbar: TaskbarControls,
    app: AppHandle,
}

impl WindowsMedia {
    pub fn new(app: &AppHandle) -> Result<Self, String> {
        let hwnd = window_hwnd(app)?;
        let smtc = SmtcSession::new(hwnd, app)?;
        let mut taskbar = TaskbarControls::new();
        taskbar.attach(app)?;
        Ok(Self {
            smtc,
            taskbar,
            app: app.clone(),
        })
    }

    pub fn set_metadata(&mut self, meta: &TrackMetadata, cover_path: Option<&str>) {
        self.smtc.set_metadata(meta, cover_path);
    }

    pub fn set_playback(&mut self, playing: bool, position_secs: f64, stopped: bool) {
        self.smtc.set_playback(playing, position_secs, stopped);
        if !stopped {
            self.taskbar.set_playback_state(&self.app, playing);
            if playing {
                self.taskbar.set_navigation_enabled(&self.app, true, true);
            }
        }
    }
}

fn window_hwnd(app: &AppHandle) -> Result<isize, String> {
    let window = app
        .get_webview_window("main")
        .ok_or("Main window not found")?;
    window
        .hwnd()
        .map(|h| h.0 as isize)
        .map_err(|e| format!("Failed to get window HWND: {e}"))
}

// ── SMTC (media flyout + media keys) ──────────────────────────────────────────

struct SmtcSession {
    controls: SystemMediaTransportControls,
    display_updater: SystemMediaTransportControlsDisplayUpdater,
    timeline_properties: SystemMediaTransportControlsTimelineProperties,
    _button_token: EventRegistrationToken,
    _position_token: EventRegistrationToken,
}

#[repr(i32)]
enum SmtcStatus {
    Stopped = 2,
    Playing = 3,
    Paused = 4,
}

impl SmtcSession {
    fn new(hwnd: isize, app: &AppHandle) -> Result<Self, String> {
        let interop: ISystemMediaTransportControlsInterop = windows::core::factory::<
            SystemMediaTransportControls,
            ISystemMediaTransportControlsInterop,
        >()
        .map_err(|e| format!("SMTC factory failed: {e}"))?;

        let controls: SystemMediaTransportControls =
            unsafe { interop.GetForWindow(HWND(hwnd)) }
                .map_err(|e| format!("SMTC GetForWindow failed: {e}"))?;
        let display_updater = controls
            .DisplayUpdater()
            .map_err(|e| format!("SMTC DisplayUpdater failed: {e}"))?;
        let timeline_properties = SystemMediaTransportControlsTimelineProperties::new()
            .map_err(|e| format!("SMTC timeline failed: {e}"))?;

        display_updater
            .SetType(MediaPlaybackType::Music)
            .map_err(|e| format!("SMTC SetType failed: {e}"))?;

        for enable in [
            controls.SetIsEnabled(true),
            controls.SetIsPlayEnabled(true),
            controls.SetIsPauseEnabled(true),
            controls.SetIsStopEnabled(true),
            controls.SetIsNextEnabled(true),
            controls.SetIsPreviousEnabled(true),
        ] {
            enable.map_err(|e| format!("SMTC capability enable failed: {e}"))?;
        }

        let app_handle = app.clone();
        let button_handler = TypedEventHandler::new({
            let app_handle = app_handle.clone();
            move |_, args: &Option<_>| {
                let args: &SystemMediaTransportControlsButtonPressedEventArgs =
                    args.as_ref().unwrap();
                let button = args.Button()?;
                let event = match button {
                    SystemMediaTransportControlsButton::Play => Some("media-control-play"),
                    SystemMediaTransportControlsButton::Pause => Some("media-control-pause"),
                    SystemMediaTransportControlsButton::Stop => Some("media-control-stop"),
                    SystemMediaTransportControlsButton::Next => Some("media-control-next"),
                    SystemMediaTransportControlsButton::Previous => Some("media-control-previous"),
                    _ => None,
                };
                if let Some(name) = event {
                    let _ = app_handle.emit(name, ());
                }
                Ok(())
            }
        });
        let button_token = controls
            .ButtonPressed(&button_handler)
            .map_err(|e| format!("SMTC button handler failed: {e}"))?;

        let app_handle = app.clone();
        let position_handler = TypedEventHandler::new({
            move |_, args: &Option<_>| {
                let args: &PlaybackPositionChangeRequestedEventArgs = args.as_ref().unwrap();
                let position = Duration::from(args.RequestedPlaybackPosition()?);
                let _ = app_handle.emit("media-control-set-position", position.as_secs_f64());
                Ok(())
            }
        });
        let position_token = controls
            .PlaybackPositionChangeRequested(&position_handler)
            .map_err(|e| format!("SMTC position handler failed: {e}"))?;

        tracing::info!("Windows SMTC session ready");
        Ok(Self {
            controls,
            display_updater,
            timeline_properties,
            _button_token: button_token,
            _position_token: position_token,
        })
    }

    fn set_playback(&mut self, playing: bool, position_secs: f64, stopped: bool) {
        let status = if stopped {
            SmtcStatus::Stopped as i32
        } else if playing {
            SmtcStatus::Playing as i32
        } else {
            SmtcStatus::Paused as i32
        };

        if self.controls.SetPlaybackStatus(MediaPlaybackStatus(status)).is_err() {
            return;
        }
        if !stopped {
            let _ = self
                .timeline_properties
                .SetPosition(TimeSpan::from(Duration::from_secs_f64(position_secs)));
        }
        let _ = self
            .controls
            .UpdateTimelineProperties(&self.timeline_properties);
    }

    fn set_metadata(&mut self, meta: &TrackMetadata, cover_path: Option<&str>) {
        let Ok(properties) = self.display_updater.MusicProperties() else {
            return;
        };

        let title = meta.title.as_deref().unwrap_or("Unknown Track");
        let artist = meta.artist.as_deref().unwrap_or("Unknown Artist");
        let album = meta.album.as_deref().unwrap_or("");

        if properties.SetTitle(&HSTRING::from(title)).is_err()
            || properties.SetArtist(&HSTRING::from(artist)).is_err()
            || properties.SetAlbumTitle(&HSTRING::from(album)).is_err()
        {
            return;
        }

        let duration = meta.duration_seconds.unwrap_or(0.0);
        let _ = self.timeline_properties.SetStartTime(TimeSpan::default());
        let _ = self
            .timeline_properties
            .SetMinSeekTime(TimeSpan::default());
        let _ = self.timeline_properties.SetEndTime(TimeSpan::from(Duration::from_secs_f64(
            duration,
        )));
        let _ = self.timeline_properties.SetMaxSeekTime(TimeSpan::from(
            Duration::from_secs_f64(duration),
        ));
        let _ = self
            .controls
            .UpdateTimelineProperties(&self.timeline_properties);

        if self.display_updater.Update().is_err() {
            return;
        }

        if let Some(path) = cover_path {
            if let Ok(op) = windows::Storage::StorageFile::GetFileFromPathAsync(&HSTRING::from(path))
            {
                if let Ok(file) = op.get() {
                    if let Ok(stream) = RandomAccessStreamReference::CreateFromFile(&file) {
                        let _ = self.display_updater.SetThumbnail(&stream);
                        let _ = self.display_updater.Update();
                    }
                }
            }
        }
    }
}

// ── Taskbar thumbnail icons (system Segoe MDL2 glyphs, white on transparent) ──

// Segoe MDL2 Assets — same code points Windows uses for media transport UI.
const GLYPH_PREVIOUS: u16 = 0xE100;
const GLYPH_PLAY: u16 = 0xE102;
const GLYPH_PAUSE: u16 = 0xE103;
const GLYPH_NEXT: u16 = 0xE101;

const FALLBACK_ICON_SIZE: i32 = 32;

struct ThumbIcons {
    previous: HICON,
    play: HICON,
    pause: HICON,
    next: HICON,
}

impl ThumbIcons {
    fn new() -> Result<Self, String> {
        Ok(Self {
            previous: load_media_icon(GLYPH_PREVIOUS, draw_previous)?,
            play: load_media_icon(GLYPH_PLAY, draw_play)?,
            pause: load_media_icon(GLYPH_PAUSE, draw_pause)?,
            next: load_media_icon(GLYPH_NEXT, draw_next)?,
        })
    }
}

fn icon_pixel_len(size_usize: usize) -> Result<usize, String> {
    size_usize
        .checked_mul(size_usize)
        .ok_or_else(|| "Icon dimensions overflow".to_string())
}

fn icon_byte_len(size_usize: usize) -> Result<usize, String> {
    icon_pixel_len(size_usize)?
        .checked_mul(4)
        .ok_or_else(|| "Bitmap size overflow".to_string())
}

fn load_media_icon(glyph: u16, draw: fn(&mut [u32; 1024])) -> Result<HICON, String> {
    match unsafe { mdl2_glyph_icon(glyph) } {
        Ok(icon) => Ok(icon),
        Err(err) => {
            tracing::warn!("Segoe MDL2 icon 0x{glyph:04X} failed ({err}), using fallback");
            thumb_icon(draw)
        }
    }
}

/// Render a glyph from the system Segoe MDL2 Assets font into an HICON.
unsafe fn mdl2_glyph_icon(glyph: u16) -> Result<HICON, String> {
    let size = GetSystemMetrics(SM_CXICON);
    if size <= 0 {
        return Err("SM_CXICON unavailable".into());
    }
    let size_usize = size as usize;

    let hdc_screen = GetDC(None);
    if hdc_screen.0 == 0 {
        return Err("GetDC failed".into());
    }

    let hdc = CreateCompatibleDC(hdc_screen);
    if hdc.0 == 0 {
        let _ = ReleaseDC(None, hdc_screen);
        return Err("CreateCompatibleDC failed".into());
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bitmap = CreateDIBSection(
        hdc,
        &bmi as *const BITMAPINFO,
        DIB_RGB_COLORS,
        &mut bits,
        None,
        0,
    )
    .map_err(|e| {
        let _ = DeleteDC(hdc);
        let _ = ReleaseDC(None, hdc_screen);
        format!("CreateDIBSection failed: {e}")
    })?;
    if bits.is_null() {
        let _ = DeleteDC(hdc);
        let _ = ReleaseDC(None, hdc_screen);
        return Err("CreateDIBSection returned null".into());
    }

    let _old_bitmap = SelectObject(hdc, bitmap);
    let byte_len = icon_byte_len(size_usize)?;
    std::ptr::write_bytes(bits as *mut u8, 0, byte_len);

    let font = CreateFontW(
        -(size * 7 / 10),
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        windows::core::w!("Segoe MDL2 Assets"),
    );
    if font.0 == 0 {
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(hdc);
        let _ = ReleaseDC(None, hdc_screen);
        return Err("CreateFontW(Segoe MDL2 Assets) failed".into());
    }

    let _old_font = SelectObject(hdc, font);
    let _ = SetBkMode(hdc, TRANSPARENT);
    let _ = SetTextColor(hdc, COLORREF(0x00FFFFFF));

    let mut text = [glyph, 0u16];
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: size,
        bottom: size,
    };
    let _ = DrawTextW(
        hdc,
        &mut text,
        &mut rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );

    let pixel_len = icon_pixel_len(size_usize)?;
    let mut pixels = vec![0u32; pixel_len];
    let src = bits as *const u8;
    for y in 0..size_usize {
        for x in 0..size_usize {
            let i = (y * size_usize + x) * 4;
            let b = *src.add(i);
            let g = *src.add(i + 1);
            let r = *src.add(i + 2);
            if u32::from(r) + u32::from(g) + u32::from(b) > 32 {
                pixels[y * size_usize + x] = 0xFFFFFFFF;
            }
        }
    }

    let _ = SelectObject(hdc, _old_font);
    let _ = DeleteObject(font);
    let _ = SelectObject(hdc, _old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(hdc);
    let _ = ReleaseDC(None, hdc_screen);

    create_argb_icon(&pixels, size)
}

fn thumb_icon(draw: fn(&mut [u32; 1024])) -> Result<HICON, String> {
    let mut pixels = [0u32; 1024];
    draw(&mut pixels);
    unsafe { create_argb_icon(&pixels, FALLBACK_ICON_SIZE) }
}

fn px(buf: &mut [u32; 1024], x: i32, y: i32) {
    if (0..FALLBACK_ICON_SIZE).contains(&x) && (0..FALLBACK_ICON_SIZE).contains(&y) {
        buf[(y * FALLBACK_ICON_SIZE + x) as usize] = 0xFFFFFFFF;
    }
}

fn fill(buf: &mut [u32; 1024], x0: i32, y0: i32, x1: i32, y1: i32) {
    for y in y0..=y1 {
        for x in x0..=x1 {
            px(buf, x, y);
        }
    }
}

/// Right-pointing triangle (play).
fn draw_play(buf: &mut [u32; 1024]) {
    for y in 7..=24 {
        let row = y - 7;
        let half = if row <= 8 { row + 1 } else { 17 - row };
        for x in (16 - half + 3)..=(16 + half + 3) {
            px(buf, x, y);
        }
    }
}

/// Two vertical bars (pause).
fn draw_pause(buf: &mut [u32; 1024]) {
    fill(buf, 10, 7, 13, 24);
    fill(buf, 19, 7, 22, 24);
}

/// Vertical bar + left triangle (previous).
fn draw_previous(buf: &mut [u32; 1024]) {
    fill(buf, 8, 7, 10, 24);
    for y in 7..=24 {
        let row = y - 7;
        let half = if row <= 8 { row + 1 } else { 17 - row };
        for x in (16 - half - 1)..=(16 + half - 1) {
            if x <= 20 {
                px(buf, x, y);
            }
        }
    }
}

/// Right triangle + vertical bar (next).
fn draw_next(buf: &mut [u32; 1024]) {
    fill(buf, 22, 7, 24, 24);
    for y in 7..=24 {
        let row = y - 7;
        let half = if row <= 8 { row + 1 } else { 17 - row };
        for x in (16 - half + 3)..=(16 + half + 3) {
            if x >= 11 {
                px(buf, x, y);
            }
        }
    }
}

unsafe fn create_argb_icon(pixels: &[u32], size: i32) -> Result<HICON, String> {
    let size = size as usize;
    if pixels.len() != size * size {
        return Err("icon pixel buffer size mismatch".into());
    }

    let mask_stride = ((size + 31) / 32) * 4;
    let mut and_bits = vec![0xFFu8; mask_stride * size];
    let mut bgra = vec![0u8; size * size * 4];

    for y in 0..size {
        for x in 0..size {
            let alpha = (pixels[y * size + x] >> 24) & 0xFF;
            let dst = (y * size + x) * 4;
            if alpha > 64 {
                bgra[dst] = 0xFF;
                bgra[dst + 1] = 0xFF;
                bgra[dst + 2] = 0xFF;
                bgra[dst + 3] = 0xFF;
                let byte = y * mask_stride + x / 8;
                let bit = 7 - (x % 8);
                and_bits[byte] &= !(1 << bit);
            }
        }
    }

    let hdc = CreateCompatibleDC(None);
    if hdc.0 == 0 {
        return Err("CreateCompatibleDC failed".into());
    }

    let and_bitmap =
        CreateBitmap(size as i32, size as i32, 1, 1, Some(and_bits.as_ptr() as _));
    if and_bitmap.0 == 0 {
        let _ = DeleteDC(hdc);
        return Err("CreateBitmap (mask) failed".into());
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let color = CreateDIBSection(
        hdc,
        &bmi as *const BITMAPINFO,
        DIB_RGB_COLORS,
        &mut bits,
        None,
        0,
    )
    .map_err(|e| format!("CreateDIBSection failed: {e}"))?;
    if bits.is_null() {
        let _ = DeleteObject(and_bitmap);
        let _ = DeleteDC(hdc);
        return Err("CreateDIBSection returned null".into());
    }

    std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());

    let info = ICONINFO {
        fIcon: true.into(),
        hbmColor: color,
        hbmMask: and_bitmap,
        ..Default::default()
    };
    let icon = CreateIconIndirect(&info).map_err(|e| format!("CreateIconIndirect: {e}"))?;
    let _ = DeleteObject(color);
    let _ = DeleteObject(and_bitmap);
    let _ = DeleteDC(hdc);
    Ok(icon)
}

// ── Taskbar thumbnail toolbar ─────────────────────────────────────────────────

const TASKBAR_SUBCLASS_ID: usize = 0x5741_5645;
const BTN_PREV: u32 = 1001;
const BTN_PLAY_PAUSE: u32 = 1002;
const BTN_NEXT: u32 = 1003;
const WM_USER_UPDATE_TASKBAR: u32 = WM_USER + 1;
const WM_USER_UPDATE_NAV_BUTTONS: u32 = WM_USER + 2;

static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

struct TaskbarState {
    taskbar: Option<ITaskbarList3>,
    buttons_added: bool,
    app: AppHandle,
    is_playing: bool,
    prev_enabled: bool,
    next_enabled: bool,
    icons: ThumbIcons,
}

impl Drop for TaskbarState {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.icons.previous);
            let _ = DestroyIcon(self.icons.play);
            let _ = DestroyIcon(self.icons.pause);
            let _ = DestroyIcon(self.icons.next);
        }
    }
}

pub struct TaskbarControls {
    attached: bool,
}

impl TaskbarControls {
    fn new() -> Self {
        Self { attached: false }
    }

    fn attach(&mut self, app: &AppHandle) -> Result<(), String> {
        if self.attached {
            return Ok(());
        }

        let hwnd = HWND(window_hwnd(app)? as _);
        if TASKBAR_CREATED_MSG.load(Ordering::Relaxed) == 0 {
            let name = wide("TaskbarButtonCreated");
            let id = unsafe { RegisterWindowMessageW(PCWSTR(name.as_ptr())) };
            TASKBAR_CREATED_MSG.store(id, Ordering::Relaxed);
        }

        let context = Box::new(TaskbarState {
            taskbar: None,
            buttons_added: false,
            app: app.clone(),
            is_playing: false,
            prev_enabled: true,
            next_enabled: true,
            icons: ThumbIcons::new()?,
        });

        unsafe {
            SetWindowSubclass(
                hwnd,
                Some(taskbar_subclass),
                TASKBAR_SUBCLASS_ID,
                Box::into_raw(context) as usize,
            );
            let msg = TASKBAR_CREATED_MSG.load(Ordering::Relaxed);
            if msg != 0 {
                let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
            }
        }

        self.attached = true;
        tracing::info!("Taskbar thumbnail controls ready");
        Ok(())
    }

    fn set_playback_state(&self, app: &AppHandle, is_playing: bool) {
        post_taskbar_msg(app, WM_USER_UPDATE_TASKBAR, if is_playing { 1 } else { 0 });
    }

    fn set_navigation_enabled(&self, app: &AppHandle, prev: bool, next: bool) {
        let value = (if prev { 1 } else { 0 }) | ((if next { 1 } else { 0 }) << 8);
        post_taskbar_msg(app, WM_USER_UPDATE_NAV_BUTTONS, value);
    }
}

fn post_taskbar_msg(app: &AppHandle, msg: u32, value: usize) {
    if let Ok(hwnd) = window_hwnd(app) {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as _)), msg, WPARAM(value), LPARAM(0));
        }
    }
}

fn thumb_mask(parts: &[THUMBBUTTONMASK]) -> THUMBBUTTONMASK {
    THUMBBUTTONMASK(parts.iter().map(|m| m.0).sum())
}

unsafe extern "system" fn taskbar_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    data: usize,
) -> LRESULT {
    let ctx = &mut *(data as *mut TaskbarState);

    if msg == TASKBAR_CREATED_MSG.load(Ordering::Relaxed) {
        ctx.taskbar = None;
        ctx.buttons_added = false;
        if let Ok(tb) = init_taskbar_buttons(hwnd, ctx) {
            ctx.taskbar = Some(tb);
            ctx.buttons_added = true;
        }
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }

    if msg == WM_USER_UPDATE_TASKBAR {
        ctx.is_playing = wparam.0 != 0;
        if let Some(ref tb) = ctx.taskbar {
            if ctx.buttons_added {
                let icon = if ctx.is_playing {
                    ctx.icons.pause
                } else {
                    ctx.icons.play
                };
                let mut btn = THUMBBUTTON {
                    dwMask: thumb_mask(&[THB_ICON, THB_TOOLTIP]),
                    iId: BTN_PLAY_PAUSE,
                    hIcon: icon,
                    szTip: [0; 260],
                    ..Default::default()
                };
                set_tip(&mut btn.szTip, if ctx.is_playing { "Pause" } else { "Play" });
                let _ = tb.ThumbBarUpdateButtons(hwnd, &[btn]);
            }
        }
        return LRESULT(0);
    }

    if msg == WM_USER_UPDATE_NAV_BUTTONS {
        ctx.prev_enabled = (wparam.0 & 0xFF) != 0;
        ctx.next_enabled = ((wparam.0 >> 8) & 0xFF) != 0;
        if let Some(ref tb) = ctx.taskbar {
            if ctx.buttons_added {
                let buttons = [
                    THUMBBUTTON {
                        dwMask: thumb_mask(&[THB_FLAGS, THB_ICON]),
                        iId: BTN_PREV,
                        hIcon: ctx.icons.previous,
                        dwFlags: if ctx.prev_enabled {
                            THBF_ENABLED
                        } else {
                            THBF_DISABLED
                        },
                        ..Default::default()
                    },
                    THUMBBUTTON {
                        dwMask: thumb_mask(&[THB_FLAGS, THB_ICON]),
                        iId: BTN_NEXT,
                        hIcon: ctx.icons.next,
                        dwFlags: if ctx.next_enabled {
                            THBF_ENABLED
                        } else {
                            THBF_DISABLED
                        },
                        ..Default::default()
                    },
                ];
                let _ = tb.ThumbBarUpdateButtons(hwnd, &buttons);
            }
        }
        return LRESULT(0);
    }

    if msg == WM_COMMAND {
        let hi = (wparam.0 >> 16) & 0xFFFF;
        let lo = wparam.0 & 0xFFFF;
        if hi as u32 == THBN_CLICKED {
            let event = match lo as u32 {
                BTN_PREV => Some("media-control-previous"),
                BTN_PLAY_PAUSE => Some("media-control-toggle"),
                BTN_NEXT => Some("media-control-next"),
                _ => None,
            };
            if let Some(name) = event {
                let _ = ctx.app.emit(name, ());
            }
            return LRESULT(0);
        }
    }

    if msg == WM_NCDESTROY {
        let _ = RemoveWindowSubclass(hwnd, Some(taskbar_subclass), subclass_id);
        let _ = Box::from_raw(ctx);
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
}

unsafe fn init_taskbar_buttons(
    hwnd: HWND,
    ctx: &TaskbarState,
) -> windows::core::Result<ITaskbarList3> {
    let taskbar: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
    taskbar.HrInit()?;

    let mut buttons = [
        THUMBBUTTON {
            dwMask: thumb_mask(&[THB_ICON, THB_TOOLTIP, THB_FLAGS]),
            iId: BTN_PREV,
            hIcon: ctx.icons.previous,
            szTip: [0; 260],
            dwFlags: THBF_ENABLED,
            ..Default::default()
        },
        THUMBBUTTON {
            dwMask: thumb_mask(&[THB_ICON, THB_TOOLTIP, THB_FLAGS]),
            iId: BTN_PLAY_PAUSE,
            hIcon: ctx.icons.play,
            szTip: [0; 260],
            dwFlags: THBF_ENABLED,
            ..Default::default()
        },
        THUMBBUTTON {
            dwMask: thumb_mask(&[THB_ICON, THB_TOOLTIP, THB_FLAGS]),
            iId: BTN_NEXT,
            hIcon: ctx.icons.next,
            szTip: [0; 260],
            dwFlags: THBF_ENABLED,
            ..Default::default()
        },
    ];
    set_tip(&mut buttons[0].szTip, "Previous");
    set_tip(&mut buttons[1].szTip, "Play");
    set_tip(&mut buttons[2].szTip, "Next");
    taskbar.ThumbBarAddButtons(hwnd, &buttons)?;
    Ok(taskbar)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set_tip(buf: &mut [u16; 260], s: &str) {
    for (i, c) in s.encode_utf16().enumerate().take(259) {
        buf[i] = c;
    }
}

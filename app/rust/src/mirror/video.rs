//! Hardware decode and display on Windows, from `spikes/mirror-latency`
//! (measured there: decode + draw ~1 ms median).
//!
//! Media Foundation's H.264 / H.265 decoder runs on the GPU (DXVA) and hands
//! back GPU textures. The D3D11 video processor converts them to RGB (with the
//! right colour matrix and range) and scales them smoothly into the window's
//! swap chain. Nothing is copied back to the CPU.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::{ManuallyDrop, size_of};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::Graphics::Gdi::{
    BLACK_BRUSH, GetMonitorInfoW, GetStockObject, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::core::{Error, Interface, PCWSTR, Result, w};

/// A decoded picture, still on the GPU.
pub type Picture = IMFSample;

const WAITABLE: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// Frames go on screen the moment they're ready, mid-refresh if need be
/// (a tear line at worst), instead of waiting for the next refresh: up to a
/// refresh less delay. Where Windows supports it (`Video::tearing`).
static TEARING: AtomicBool = AtomicBool::new(false);

fn swap_flags() -> DXGI_SWAP_CHAIN_FLAG {
    if TEARING.load(Relaxed) { WAITABLE | DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING } else { WAITABLE }
}

// One mirror window at a time (see `super::show`).
static RESIZED: AtomicBool = AtomicBool::new(false);
static CLOSED: AtomicBool = AtomicBool::new(false);
/// Set when the window leaves full screen: re-fit it to the picture, which
/// may have turned meanwhile.
static REFIT: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Where the window was before going full screen; `Some` while it is.
    static WINDOWED: Cell<Option<WINDOWPLACEMENT>> = const { Cell::new(None) };
    /// The picture's shape, so resizing by hand keeps it (no black bars).
    static ASPECT: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
}

/// The "Full screen" item in the title bar's menu (below 0xF000, low 4 bits clear).
const MENU_FULLSCREEN: usize = 0x10;
/// "Game mode": keys work the phone game's joystick (`super::control`).
const MENU_GAME: usize = 0x20;
const VK_F2: usize = 0x71;
const VK_ESCAPE: usize = 0x1B;
const VK_RETURN: usize = 0x0D;
const VK_F11: usize = 0x7A;

/// Keeps the picture going while the window is moved or resized (Windows'
/// own loop runs meanwhile; see `super::step_now`). Windows fires timers at
/// most every ~10 ms.
const STEP_TIMER: usize = 1;

const STYLE: WINDOW_STYLE = WS_OVERLAPPEDWINDOW;

pub(super) fn is_fullscreen() -> bool {
    WINDOWED.get().is_some()
}

/// Switches between a normal window and borderless full screen on the
/// window's monitor. Borderless (not exclusive) keeps the flip swap chain and
/// lets Windows skip composition, so it costs no latency.
fn toggle_fullscreen(hwnd: HWND) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        match WINDOWED.take() {
            None => {
                let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                if GetWindowPlacement(hwnd, &mut placement).is_err()
                    || !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool()
                {
                    return;
                }
                WINDOWED.set(Some(placement));
                SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(STYLE.0 as isize));
                let m = mi.rcMonitor;
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    m.left,
                    m.top,
                    m.right - m.left,
                    m.bottom - m.top,
                    SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
            }
            Some(placement) => {
                SetWindowLongPtrW(hwnd, GWL_STYLE, style | STYLE.0 as isize);
                let _ = SetWindowPlacement(hwnd, &placement);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_FRAMECHANGED,
                );
                REFIT.store(true, Relaxed);
            }
        }
    }
    super::settings::change(|s| s.fullscreen = is_fullscreen());
}

/// What the window adds around the picture: its borders (sides and
/// bottom) and our bar on top (Windows' own title bar is gone, see
/// `WM_NCCALCSIZE`).
fn chrome_size(hwnd: Option<HWND>) -> (i32, i32) {
    use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
    let dpi = unsafe { hwnd.map_or_else(|| GetDpiForSystem(), |h| GetDpiForWindow(h)) };
    let mut r = RECT::default();
    let _ = unsafe { AdjustWindowRectExForDpi(&mut r, WS_THICKFRAME, false, WINDOW_EX_STYLE(0), dpi) };
    let bar = if hwnd.is_some() && is_fullscreen() { 0 } else { super::chrome::height_for_dpi(dpi) };
    (r.right - r.left, r.bottom + bar)
}

/// Keeps a window being resized by hand in the picture's shape.
fn keep_aspect(hwnd: HWND, edge: u32, r: &mut RECT) {
    let Some((pw, ph)) = ASPECT.get() else { return };
    let (fw, fh) = chrome_size(Some(hwnd));
    let (cw, ch) = (r.right - r.left - fw, r.bottom - r.top - fh);
    match edge {
        // Top or bottom edge: the height leads.
        WMSZ_TOP | WMSZ_BOTTOM => r.right = r.left + (ch as f64 * pw as f64 / ph as f64).round() as i32 + fw,
        // Sides and corners: the width leads.
        _ => {
            let h = (cw as f64 * ph as f64 / pw as f64).round() as i32 + fh;
            if matches!(edge, WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
                r.top = r.bottom - h;
            } else {
                r.bottom = r.top + h;
            }
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = frame(hwnd, msg, wp, lp) {
        return r;
    }
    if let Some(r) = super::control::handle(hwnd, msg, wp, lp) {
        return r;
    }
    match msg {
        WM_SIZE => {
            RESIZED.store(true, Relaxed);
            super::chrome::invalidate();
            LRESULT(0)
        }
        WM_ACTIVATE | WM_SETTEXT | WM_SETTINGCHANGE | WM_THEMECHANGED => {
            super::chrome::invalidate();
            if msg == WM_SETTINGCHANGE || msg == WM_THEMECHANGED {
                dark_frame(hwnd);
            }
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_SIZING => {
            keep_aspect(hwnd, wp.0 as u32, unsafe { &mut *(lp.0 as *mut RECT) });
            LRESULT(1)
        }
        // Moving or resizing (or the title bar's menu): Windows' loop runs
        // until it's over, so the picture is kept going from a timer.
        WM_ENTERSIZEMOVE | WM_ENTERMENULOOP => {
            unsafe { SetTimer(hwnd, STEP_TIMER, 5, None) };
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_EXITSIZEMOVE | WM_EXITMENULOOP => {
            let _ = unsafe { KillTimer(hwnd, STEP_TIMER) };
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_TIMER if wp.0 == STEP_TIMER => {
            super::step_now();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wp.0 {
                VK_ESCAPE if is_fullscreen() => toggle_fullscreen(hwnd),
                // Controlling the phone: Esc is its Back button, not "close".
                VK_ESCAPE if super::control::ready() => super::control::back(),
                VK_ESCAPE => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                VK_F11 => toggle_fullscreen(hwnd),
                VK_F2 => super::control::toggle_game(),
                _ => {}
            }
            LRESULT(0)
        }
        // Alt+Enter (bit 29: Alt held); everything else Alt+ goes on to Windows.
        WM_SYSKEYDOWN if wp.0 == VK_RETURN && lp.0 & (1 << 29) != 0 => {
            toggle_fullscreen(hwnd);
            LRESULT(0)
        }
        WM_SYSCHAR if wp.0 == VK_RETURN => LRESULT(0), // no beep after Alt+Enter
        WM_SYSCOMMAND if wp.0 & 0xFFF0 == MENU_FULLSCREEN => {
            toggle_fullscreen(hwnd);
            LRESULT(0)
        }
        WM_SYSCOMMAND if wp.0 & 0xFFF0 == MENU_GAME => {
            super::control::toggle_game();
            LRESULT(0)
        }
        WM_DESTROY => {
            CLOSED.store(true, Relaxed);
            WINDOWED.set(None);
            ASPECT.set(None);
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Windows' frame follows the app theme (its border and shadow).
fn dark_frame(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let dark = BOOL::from(!super::chrome::light_theme());
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const BOOL).cast(),
            size_of::<BOOL>() as u32,
        );
    }
}

fn client_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// The window without Windows' title bar: our bar (`super::chrome`) takes
/// its place. `None`: not about the frame.
fn frame(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    use super::chrome::{self, Part};
    if is_fullscreen() && msg != WM_NCCALCSIZE {
        return None;
    }
    match msg {
        // The client area takes the title bar's place; the sides and
        // bottom keep Windows' resize borders.
        WM_NCCALCSIZE if wp.0 != 0 && !is_fullscreen() => unsafe {
            let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let r = DefWindowProcW(hwnd, msg, wp, lp);
            params.rgrc[0].top = top;
            // Maximized, the window reaches past the screen by its border.
            if IsZoomed(hwnd).as_bool() {
                let (_, fh) = chrome_size(Some(hwnd));
                params.rgrc[0].top += fh - chrome::height(hwnd);
            }
            Some(r)
        },
        WM_NCHITTEST => unsafe {
            let hit = DefWindowProcW(hwnd, msg, wp, lp);
            if hit.0 != HTCLIENT as isize {
                return Some(hit); // the sides, bottom and corners
            }
            let (sx, sy) = client_xy(lp);
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            if pt.y >= chrome::height(hwnd) {
                return Some(hit);
            }
            // The top edge resizes, as Windows' own title bar does.
            let (_, fh) = chrome_size(Some(hwnd));
            let border = fh - chrome::height(hwnd);
            if !IsZoomed(hwnd).as_bool() && pt.y < border.min(6) {
                return Some(LRESULT(HTTOP as isize));
            }
            Some(LRESULT(match chrome::part_at(hwnd, pt.x, pt.y) {
                Some(Part::Min) => HTMINBUTTON,
                Some(Part::Max) => HTMAXBUTTON,
                Some(Part::Close) => HTCLOSE,
                Some(Part::Back | Part::More) => HTCLIENT,
                None => HTCAPTION,
            } as isize))
        },
        // The window buttons: drawn and pressed here (Windows' own handling
        // would paint its old buttons over ours).
        WM_NCMOUSEMOVE => {
            let part = match wp.0 as u32 {
                HTMINBUTTON => Some(Part::Min),
                HTMAXBUTTON => Some(Part::Max),
                HTCLOSE => Some(Part::Close),
                _ => None,
            };
            chrome::set_hover(part);
            track_leave(hwnd, true);
            None
        }
        WM_NCMOUSELEAVE => {
            chrome::set_hover(None);
            chrome::set_pressed(None);
            None
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if matches!(wp.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            chrome::set_pressed(match wp.0 as u32 {
                HTMINBUTTON => Some(Part::Min),
                HTMAXBUTTON => Some(Part::Max),
                _ => Some(Part::Close),
            });
            Some(LRESULT(0))
        }
        WM_NCLBUTTONUP if matches!(wp.0 as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            let pressed = chrome::pressed();
            chrome::set_pressed(None);
            let command = match (wp.0 as u32, pressed) {
                (HTMINBUTTON, Some(Part::Min)) => SC_MINIMIZE,
                (HTMAXBUTTON, Some(Part::Max)) if unsafe { IsZoomed(hwnd).as_bool() } => SC_RESTORE,
                (HTMAXBUTTON, Some(Part::Max)) => SC_MAXIMIZE,
                (HTCLOSE, Some(Part::Close)) => SC_CLOSE,
                _ => return Some(LRESULT(0)),
            };
            chrome::set_hover(None);
            unsafe {
                let _ = PostMessageW(hwnd, WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0));
            }
            Some(LRESULT(0))
        }
        // Back and "...": in the client area, above the picture.
        WM_MOUSEMOVE => {
            let (x, y) = client_xy(lp);
            match chrome::part_at(hwnd, x, y) {
                Some(p @ (Part::Back | Part::More)) => {
                    chrome::set_hover(Some(p));
                    track_leave(hwnd, false);
                    // Not a drag on the phone: unless one is under way.
                    None
                }
                _ => {
                    chrome::set_hover(None);
                    None
                }
            }
        }
        WM_MOUSELEAVE => {
            chrome::set_hover(None);
            chrome::set_pressed(None);
            None
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = client_xy(lp);
            match chrome::part_at(hwnd, x, y) {
                Some(p @ (Part::Back | Part::More)) => {
                    chrome::set_pressed(Some(p));
                    Some(LRESULT(0))
                }
                _ => None,
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = client_xy(lp);
            let Some(p @ (Part::Back | Part::More)) = chrome::pressed() else { return None };
            chrome::set_pressed(None);
            if chrome::part_at(hwnd, x, y) == Some(p) {
                match p {
                    Part::Back => super::control::back(),
                    _ => more_menu(hwnd),
                }
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Asks for `WM_(NC)MOUSELEAVE`, to clear the hover.
fn track_leave(hwnd: HWND, nonclient: bool) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: if nonclient { TME_LEAVE | TME_NONCLIENT } else { TME_LEAVE },
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    unsafe {
        let _ = TrackMouseEvent(&mut t);
    }
}

/// Not in the bindings next to the other mouse messages.
const WM_MOUSELEAVE: u32 = 0x02A3;

const MENU_ON_TOP: usize = 0x30;
const MENU_AUDIO: usize = 0x40;
const MENU_STOP: usize = 0x50;

/// "...": the window's settings.
fn more_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
        let on_top = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as isize != 0;
        let audio = super::audio::wanted();
        let _ = AppendMenuW(menu, MF_STRING | check(super::control::game()), MENU_GAME, w!("Game mode\tF2"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_FULLSCREEN, w!("Full screen\tF11"));
        let _ = AppendMenuW(menu, MF_STRING | check(on_top), MENU_ON_TOP, w!("Keep on top"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING | check(audio), MENU_AUDIO, w!("Play phone audio on this PC"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_STOP, w!("Stop sharing"));
        // Under "...", left-aligned with it.
        let (from, _) = super::chrome::more_at();
        let mut pt = POINT { x: from as i32, y: super::chrome::height(hwnd) };
        let _ = ClientToScreen(hwnd, &mut pt);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, 0, hwnd, None);
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            MENU_GAME => super::control::toggle_game(),
            MENU_FULLSCREEN => toggle_fullscreen(hwnd),
            MENU_ON_TOP => {
                let z = if on_top { HWND_NOTOPMOST } else { HWND_TOPMOST };
                let _ = SetWindowPos(hwnd, z, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                super::settings::change(|s| s.on_top = !on_top);
            }
            MENU_AUDIO => super::audio::set_wanted(!audio),
            MENU_STOP => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
        super::chrome::invalidate();
    }
}

/// Handles window messages. Returns false once the window is closed.
pub fn pump() -> bool {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    !CLOSED.load(Relaxed)
}

/// Closes the window (from its thread).
pub fn close(video: &Video) {
    if let Some((hwnd, _, _)) = &video.window {
        unsafe {
            let _ = DestroyWindow(*hwnd);
        }
    }
    CLOSED.store(true, Relaxed);
}

/// A window `width` x `height` (client area, physical pixels), black until
/// the first frame.
fn create_window(title: &str, width: i32, height: i32) -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("NudgeMirror");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            // The app's own icon (resource 101 in the Flutter runner).
            hIcon: LoadIconW(instance, PCWSTR(101 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let (fw, fh) = chrome_size(None);
        let r = RECT { left: 0, top: 0, right: width + fw, bottom: height + fh };
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(title.as_ptr()),
            STYLE | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            instance,
            None,
        )?;
        // Right-click on the title bar (or Alt+Space): "Full screen".
        let menu = GetSystemMenu(hwnd, false);
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_FULLSCREEN, w!("Full screen\tF11"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_GAME, w!("Game mode (keys move)\tF2"));
        // Windows' frame redone without its title bar (`WM_NCCALCSIZE`).
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED);
        dark_frame(hwnd);
        let _ = SetForegroundWindow(hwnd);
        super::control::set_window(hwnd, &String::from_utf16_lossy(&title[..title.len() - 1]));
        super::control::listen_to_touchpad(hwnd);
        // The settings it had last time (game mode and sound: `super::show`).
        let saved = super::settings::get();
        if saved.on_top {
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        if saved.fullscreen {
            toggle_fullscreen(hwnd);
        }
        Ok(hwnd)
    }
}

/// The usable part (no taskbar) of the monitor `hwnd` is on, or the main
/// monitor's before there is a window.
fn work_area(hwnd: Option<HWND>) -> RECT {
    unsafe {
        let monitor = match hwnd {
            Some(h) => MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST),
            None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
        };
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT { left: 0, top: 0, right: 1920, bottom: 1040 }
        }
    }
}

/// The largest client area in a `w` x `h` picture's shape whose window
/// (title bar and borders included) fits in `share` of the work area, and
/// no bigger than the picture itself.
fn fitted(hwnd: Option<HWND>, w: i32, h: i32, share: f64) -> (i32, i32) {
    let area = work_area(hwnd);
    let (fw, fh) = chrome_size(hwnd);
    let room_w = (area.right - area.left) as f64 * share - fw as f64;
    let room_h = (area.bottom - area.top) as f64 * share - fh as f64;
    let scale = (room_h / h as f64).min(room_w / w as f64).min(1.0);
    ((w as f64 * scale) as i32, (h as f64 * scale) as i32)
}

/// What the decoder says about the current stream.
#[derive(Clone, Copy, Debug)]
struct StreamInfo {
    /// Decoded texture size (may include padding rows/columns).
    coded: (u32, u32),
    /// The visible part of it.
    visible: RECT,
    bt709: bool,
    full_range: bool,
}

struct Decoder {
    mft: IMFTransform,
    hevc: bool,
    info: Option<StreamInfo>,
}

impl Decoder {
    fn new(hevc: bool, manager: &IMFDXGIDeviceManager) -> Result<Self> {
        unsafe {
            let subtype = if hevc { MFVideoFormat_HEVC } else { MFVideoFormat_H264 };
            let input = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
            let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
            let mut count = 0u32;
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_DECODER,
                MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_LOCALMFT | MFT_ENUM_FLAG_SORTANDFILTER,
                Some(&input),
                None,
                &mut list,
                &mut count,
            )?;
            let activates = std::slice::from_raw_parts_mut(list, count as usize);
            let mft = activates.first().and_then(|a| a.as_ref()).map(|a| a.ActivateObject::<IMFTransform>());
            for a in activates.iter_mut() {
                drop(a.take());
            }
            CoTaskMemFree(Some(list as *const c_void));
            let mft = match mft {
                Some(m) => m?,
                None => {
                    let what = if hevc {
                        "No H.265 decoder: install \"HEVC Video Extensions\" from the Microsoft Store, or use H.264"
                    } else {
                        "No H.264 decoder found"
                    };
                    return Err(Error::new(MF_E_TOPO_CODEC_NOT_FOUND, what));
                }
            };

            let attrs = mft.GetAttributes()?;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) == 0 {
                return Err(Error::new(E_FAIL, "the decoder can't use the GPU"));
            }
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)?;
            // Without these the decoder holds several frames back before output.
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            let _ = attrs.SetUINT32(&CODECAPI_AVLowLatencyMode, 1);

            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &t, 0)?;

            let mut d = Decoder { mft, hevc, info: None };
            let _ = d.set_output_type(); // may only be possible after the first frame
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            if d.mft.GetOutputStreamInfo(0)?.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
                return Err(Error::new(E_FAIL, "the decoder isn't handing out GPU frames"));
            }
            Ok(d)
        }
    }

    /// Picks NV12 output and reads the frame size, visible area and colour format.
    fn set_output_type(&mut self) -> Result<()> {
        unsafe {
            let mut i = 0;
            let t = loop {
                let t = self.mft.GetOutputAvailableType(0, i)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    break t;
                }
                i += 1;
            };
            self.mft.SetOutputType(0, &t, 0)?;

            let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let coded = ((size >> 32) as u32, size as u32);
            let mut visible = RECT { left: 0, top: 0, right: coded.0 as i32, bottom: coded.1 as i32 };
            let mut area = MFVideoArea::default();
            let blob = std::slice::from_raw_parts_mut(
                &mut area as *mut MFVideoArea as *mut u8,
                std::mem::size_of::<MFVideoArea>(),
            );
            if t.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None).is_ok() && area.Area.cx > 0 {
                visible = RECT {
                    left: area.OffsetX.value as i32,
                    top: area.OffsetY.value as i32,
                    right: area.OffsetX.value as i32 + area.Area.cx,
                    bottom: area.OffsetY.value as i32 + area.Area.cy,
                };
            }
            // The phone asks for BT.709, limited range; trust what the stream says.
            let bt709 = t.GetUINT32(&MF_MT_YUV_MATRIX).map_or(true, |m| m != MFVideoTransferMatrix_BT601.0 as u32);
            let full_range =
                t.GetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE).is_ok_and(|r| r == MFNominalRange_0_255.0 as u32);
            let info = StreamInfo { coded, visible, bt709, full_range };
            if self.info.is_none_or(|old| format!("{old:?}") != format!("{info:?}")) {
                log::info!(
                    "mirror stream: {} {}x{} ({}, {} range)",
                    if self.hevc { "H.265" } else { "H.264" },
                    visible.right - visible.left,
                    visible.bottom - visible.top,
                    if bt709 { "BT.709" } else { "BT.601" },
                    if full_range { "full" } else { "limited" },
                );
            }
            self.info = Some(info);
            Ok(())
        }
    }

    /// Decodes one access unit; returns the newest frame it produced, if any.
    fn decode(&mut self, data: &[u8], time_us: u64) -> Result<Option<IMFSample>> {
        unsafe {
            let buffer = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time_us as i64 * 10)?;

            let mut newest = self.drain()?;
            match self.mft.ProcessInput(0, &sample, 0) {
                Err(e) if e.code() == MF_E_NOTACCEPTING => {
                    newest = self.drain()?.or(newest);
                    self.mft.ProcessInput(0, &sample, 0)?;
                }
                r => r?,
            }
            Ok(self.drain()?.or(newest))
        }
    }

    fn drain(&mut self) -> Result<Option<IMFSample>> {
        let mut newest = None;
        loop {
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = unsafe { self.mft.ProcessOutput(0, &mut out, &mut status) };
            let sample = unsafe { ManuallyDrop::take(&mut out[0].pSample) };
            drop(unsafe { ManuallyDrop::take(&mut out[0].pEvents) });
            match r {
                Ok(()) => newest = sample.or(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.set_output_type()?,
                Err(e) => return Err(e),
            }
        }
    }
}

pub struct Video {
    device: ID3D11Device,
    /// Kept with the device it belongs to.
    _context: ID3D11DeviceContext,
    vdevice: ID3D11VideoDevice,
    vcontext: ID3D11VideoContext1,
    manager: IMFDXGIDeviceManager,
    /// The window, its swap chain, and the swap chain's "ready for a frame" signal.
    window: Option<(HWND, IDXGISwapChain1, HANDLE)>,
    /// The picture size the window was last fitted to.
    fitted_to: Option<(i32, i32)>,
    processor: Option<((u32, u32), ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)>,
    decoder: Option<Decoder>,
    /// The picture last shown, to draw again when only the bar changed.
    last: Option<IMFSample>,
}

impl Video {
    /// Opens the window (phone-shaped until the first frame says otherwise)
    /// and gets the GPU and decoder ready, so the first frame shows at once.
    pub fn new(title: &str) -> Result<Self> {
        unsafe {
            // Without this, Windows renders the window at 100% and stretches it
            // to the display's scale (e.g. 200%): a blurry picture. This
            // thread only: the app's own window is left as it is.
            let _ = windows::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
            CLOSED.store(false, Relaxed);
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device: ID3D11Device = device.unwrap();
            let context = context.unwrap();
            // Media Foundation uses the device from its own threads.
            let _ = device.cast::<ID3D11Multithread>()?.SetMultithreadProtected(true);
            // Don't let frames queue up behind the display.
            device.cast::<IDXGIDevice1>()?.SetMaximumFrameLatency(1)?;

            let mut token = 0u32;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(&device, token)?;

            let mut v = Video {
                vdevice: device.cast()?,
                vcontext: context.cast()?,
                device,
                _context: context,
                manager,
                window: None,
                fitted_to: None,
                processor: None,
                decoder: None,
                last: None,
            };
            v.decoder = Some(Decoder::new(false, &v.manager)?);
            let (w, h) = fitted(None, 1056, 2304, 0.96);
            v.open_window(title, w, h)?;
            Ok(v)
        }
    }

    /// Decodes one frame; returns the newest decoded picture, if any.
    pub fn decode(&mut self, data: &[u8], hevc: bool, time_us: u64) -> Result<Option<IMFSample>> {
        if self.decoder.as_ref().is_none_or(|d| d.hevc != hevc) {
            self.decoder = None;
            self.decoder = Some(Decoder::new(hevc, &self.manager)?);
        }
        let d = self.decoder.as_mut().unwrap();
        match d.decode(data, time_us) {
            Ok(s) => Ok(s),
            Err(e) => {
                self.decoder = None; // start fresh at the next keyframe
                Err(e)
            }
        }
    }

    fn stream_info(&self) -> Option<StreamInfo> {
        self.decoder.as_ref().and_then(|d| d.info)
    }

    fn processor(&mut self, coded: (u32, u32)) -> Result<(ID3D11VideoProcessorEnumerator, ID3D11VideoProcessor)> {
        if let Some((size, e, p)) = &self.processor {
            if *size == coded {
                return Ok((e.clone(), p.clone()));
            }
        }
        unsafe {
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
                InputWidth: coded.0,
                InputHeight: coded.1,
                OutputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
                OutputWidth: coded.0,
                OutputHeight: coded.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
            };
            let e = self.vdevice.CreateVideoProcessorEnumerator(&desc)?;
            let p = self.vdevice.CreateVideoProcessor(&e, 0)?;
            self.processor = Some((coded, e.clone(), p.clone()));
            Ok((e, p))
        }
    }

    /// Converts and scales a decoded frame into `target`, fitted inside `area`.
    /// `top`: the picture goes below this many rows (the bar). Returns
    /// where it went.
    fn blit(&mut self, sample: &IMFSample, target: &ID3D11Texture2D, target_size: (u32, u32), top: i32) -> Result<RECT> {
        let info = self.stream_info().ok_or_else(|| Error::new(E_FAIL, "no stream info"))?;
        let (enumerator, vp) = self.processor(info.coded)?;
        unsafe {
            let dxgi: IMFDXGIBuffer = sample.GetBufferByIndex(0)?.cast()?;
            let mut texture: Option<ID3D11Texture2D> = None;
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut texture as *mut _ as *mut *mut c_void)?;
            let texture = texture.unwrap();
            let index = dxgi.GetSubresourceIndex()?;

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: index },
                },
            };
            let mut input = None;
            self.vdevice.CreateVideoProcessorInputView(&texture, &enumerator, &in_desc, Some(&mut input))?;
            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut output = None;
            self.vdevice.CreateVideoProcessorOutputView(target, &enumerator, &out_desc, Some(&mut output))?;
            let output = output.unwrap();

            let src = info.visible;
            let (tw, th) = (target_size.0 as i32, target_size.1 as i32);
            let full = RECT { left: 0, top: 0, right: tw, bottom: th };
            // Keep the phone's shape; centre it with black bars.
            let ah = (th - top).max(1);
            let (sw, sh) = ((src.right - src.left) as f64, (src.bottom - src.top) as f64);
            let scale = (tw as f64 / sw).min(ah as f64 / sh);
            let (dw, dh) = ((sw * scale).round() as i32, (sh * scale).round() as i32);
            let (x, y) = ((tw - dw) / 2, top + (ah - dh) / 2);
            let dest = RECT { left: x, top: y, right: x + dw, bottom: y + dh };
            super::control::set_picture(dest); // the back buffer is the window's client area

            let vc = &self.vcontext;
            vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false); // no driver "enhancements"
            vc.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src));
            vc.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dest));
            vc.VideoProcessorSetOutputTargetRect(&vp, true, Some(&full));
            let black = D3D11_VIDEO_COLOR {
                Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } },
            };
            vc.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
            let in_space = match (info.bt709, info.full_range) {
                (true, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                (true, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P709,
                (false, false) => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
                (false, true) => DXGI_COLOR_SPACE_YCBCR_FULL_G22_LEFT_P601,
            };
            vc.VideoProcessorSetStreamColorSpace1(&vp, 0, in_space);
            vc.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                pInputSurface: ManuallyDrop::new(input),
                ..Default::default()
            };
            let mut streams = [stream];
            let r = vc.VideoProcessorBlt(&vp, &output, 0, &streams);
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            r.map(|()| dest)
        }
    }

    /// True if the display can take a frame now, without waiting. When it returns
    /// true, call `present` next: the check uses up the slot.
    pub fn ready_to_present(&self) -> bool {
        match &self.window {
            None => true,
            Some((_, _, ready)) => unsafe { WaitForSingleObject(*ready, 0) == WAIT_OBJECT_0 },
        }
    }

    fn open_window(&mut self, title: &str, width: i32, height: i32) -> Result<()> {
        {
            let hwnd = create_window(title, width, height)?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_NONE,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Flags: 0,
                ..Default::default()
            };
            let (swapchain, ready) = unsafe {
                let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
                let tearing = factory.cast::<IDXGIFactory5>().is_ok_and(|f| {
                    let mut allow = BOOL(0);
                    f.CheckFeatureSupport(
                        DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                        (&mut allow as *mut BOOL).cast(),
                        size_of::<BOOL>() as u32,
                    )
                    .is_ok()
                        && allow.as_bool()
                });
                TEARING.store(tearing, Relaxed);
                log::info!("mirror: frames shown {}", if tearing { "at once (tearing allowed)" } else { "at the next refresh" });
                let desc = DXGI_SWAP_CHAIN_DESC1 { Flags: swap_flags().0 as u32, ..desc };
                let sc = factory.CreateSwapChainForHwnd(&self.device, hwnd, &desc, None, None)?;
                // At most one frame queued; the handle is signalled when it's taken.
                let sc2: IDXGISwapChain2 = sc.cast()?;
                sc2.SetMaximumFrameLatency(1)?;
                (sc, sc2.GetFrameLatencyWaitableObject())
            };
            self.window = Some((hwnd, swapchain, ready));
            RESIZED.store(false, Relaxed);
        }
        Ok(())
    }

    /// Shows a decoded frame in the window, without waiting for the display:
    /// false if it wasn't shown (minimized, or the display still busy).
    pub fn present(&mut self, sample: &IMFSample) -> Result<bool> {
        let Some(info) = self.stream_info() else { return Ok(true) };
        let Some((hwnd, _, _)) = self.window.clone() else { return Ok(true) };
        if unsafe { IsIconic(hwnd).as_bool() } {
            return Ok(false); // nothing to draw into; resized when it comes back
        }
        let picture = (info.visible.right - info.visible.left, info.visible.bottom - info.visible.top);
        if self.fitted_to != Some(picture) || REFIT.swap(false, Relaxed) {
            // First frame, the phone turned, or back from full screen: give
            // the window the picture's shape.
            self.fit_window(hwnd, picture);
        }
        self.last = Some(sample.clone());
        self.draw(Some(sample))
    }

    /// Draws the last picture again with the bar as it is now (hover,
    /// the phone locked), or just the bar before there's a picture.
    pub fn redraw(&mut self) -> Result<bool> {
        let Some((hwnd, _, _)) = self.window.clone() else { return Ok(true) };
        if unsafe { IsIconic(hwnd).as_bool() } {
            return Ok(false);
        }
        let last = self.last.clone();
        self.draw(last.as_ref())
    }

    fn draw(&mut self, sample: Option<&IMFSample>) -> Result<bool> {
        let Some((hwnd, swapchain, _)) = self.window.clone() else { return Ok(true) };
        unsafe {
            let mut client = RECT::default();
            GetClientRect(hwnd, &mut client)?;
            let size = ((client.right - client.left).max(1) as u32, (client.bottom - client.top).max(1) as u32);
            if RESIZED.swap(false, Relaxed) {
                swapchain.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, swap_flags())?;
            }
            let back: ID3D11Texture2D = swapchain.GetBuffer(0)?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut desc);
            let top = super::chrome::height(hwnd);
            let have_picture = sample.is_some() && self.stream_info().is_some();
            if let Some(s) = sample.filter(|_| have_picture) {
                self.blit(s, &back, (desc.Width, desc.Height), top)?;
            }
            let mut title = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut title).max(0) as usize;
            let title = String::from_utf16_lossy(&title[..n]);
            let bar = super::chrome::Bar {
                title: &title,
                active: GetForegroundWindow() == hwnd,
                maximized: IsZoomed(hwnd).as_bool(),
                clear: !have_picture,
            };
            let surface: IDXGISurface = back.cast()?;
            if let Err(e) = super::chrome::draw(&self.device.cast()?, &surface, hwnd, &bar) {
                log::warn!("mirror bar: {e}");
            }
            drop(surface);
            drop(back);
            // Interval 0: hand the frame to the compositor now, don't wait for
            // vsync; and never block when it can't take one (minimizing,
            // maximizing): the window's messages would wait too.
            let tear = if TEARING.load(Relaxed) { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
            match swapchain.Present(0, DXGI_PRESENT_DO_NOT_WAIT | tear) {
                r if r == DXGI_ERROR_WAS_STILL_DRAWING => Ok(false),
                r => r.ok().map(|()| true),
            }
        }
    }
}

impl Video {
    /// Resizes the window to `picture`'s shape. The first time, as big as
    /// fits the screen. After that, a phone turning sideways keeps the
    /// window's longer side, shrunk only if that no longer fits the screen,
    /// and the window stays centred where it was and on screen. Full screen
    /// and maximized windows keep their size: the picture is centred in them.
    fn fit_window(&mut self, hwnd: HWND, picture: (i32, i32)) {
        let first = self.fitted_to.is_none();
        self.fitted_to = Some(picture);
        ASPECT.set(Some(picture));
        unsafe {
            if is_fullscreen() || IsZoomed(hwnd).as_bool() {
                return;
            }
            let mut client = RECT::default();
            let mut outer = RECT::default();
            if GetClientRect(hwnd, &mut client).is_err() || GetWindowRect(hwnd, &mut outer).is_err() {
                return;
            }
            // The picture's part of the window: below the bar.
            let (cw, ch) = (client.right - client.left, client.bottom - client.top - super::chrome::height(hwnd));
            let (pw, ph) = (picture.0 as f64, picture.1 as f64);
            let most = fitted(Some(hwnd), picture.0, picture.1, if first { 0.96 } else { 1.0 });
            let (w, h) = if first {
                most
            } else {
                let scale = cw.max(ch) as f64 / pw.max(ph);
                let (w, h) = ((pw * scale) as i32, (ph * scale) as i32);
                if w > most.0 || h > most.1 { most } else { (w, h) }
            };
            if (w, h) == (cw, ch) {
                return;
            }
            let (fw, fh) = chrome_size(Some(hwnd));
            let (ow, oh) = (w + fw, h + fh);
            // Same centre, then pushed back inside the work area.
            let area = work_area(Some(hwnd));
            let mut x = (outer.left + outer.right) / 2 - ow / 2;
            let mut y = (outer.top + outer.bottom) / 2 - oh / 2;
            x = x.min(area.right - ow).max(area.left);
            y = y.min(area.bottom - oh).max(area.top);
            let _ = SetWindowPos(hwnd, None, x, y, ow, oh, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

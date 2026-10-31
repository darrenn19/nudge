//! The mirror window's top bar, drawn by us in the style of Windows 11 apps
//! (like Phone Link): Back (the phone's Back), the phone's name, a "..."
//! menu with the window's settings, then minimize, maximize and close.
//!
//! The window has no Windows title bar: `WM_NCCALCSIZE` gives its space to
//! the client area (`super::video`), and the bar is drawn with Direct2D onto
//! the same swap chain as the picture, so the picture keeps its own fast
//! path. `WM_NCHITTEST` makes the empty part of the bar drag the window and
//! the three window buttons Windows' own (snap layouts on maximize); Back
//! and "..." are handled here.
//!
//! Also drawn here: the notice over the picture while the phone is locked.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::core::{Result, w};

/// The bar needs drawing again; set from any thread.
static REDRAW: AtomicBool = AtomicBool::new(true);
/// What to say over the picture (`Notice`).
static NOTICE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// A notice over the picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Notice {
    None = 0,
    /// The phone is locked: nothing is shared until it's unlocked.
    Locked = 1,
    /// Unlocked, but Android stopped the capture at the lock: its user has
    /// to allow sharing again.
    AllowAgain = 2,
}

/// The bar's height, and the widths of its buttons, in DIPs (96 per inch).
const BAR_DIP: f32 = 40.0;
const BACK_DIP: f32 = 44.0;
const MORE_DIP: f32 = 46.0;
const CAPTION_DIP: f32 = 46.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Part {
    Back,
    More,
    Min,
    Max,
    Close,
}

thread_local! {
    /// The part under the mouse, and the one pressed.
    static HOVER: Cell<Option<Part>> = const { Cell::new(None) };
    static PRESSED: Cell<Option<Part>> = const { Cell::new(None) };
    /// Where "..." was drawn last (px from the left): it follows the name.
    static MORE_AT: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
    static DRAWER: RefCell<Option<Drawer>> = const { RefCell::new(None) };
}

/// The bar needs drawing again (hover, a resize, the title changed).
pub(super) fn invalidate() {
    REDRAW.store(true, Relaxed);
}

/// True once after `invalidate`.
pub(super) fn take_redraw() -> bool {
    REDRAW.swap(false, Relaxed)
}

/// From the session (any thread).
pub(super) fn set_notice(n: Notice) {
    if NOTICE.swap(n as u8, Relaxed) != n as u8 {
        invalidate();
    }
}

pub(super) fn notice() -> Notice {
    match NOTICE.load(Relaxed) {
        1 => Notice::Locked,
        2 => Notice::AllowAgain,
        _ => Notice::None,
    }
}

pub(super) fn scale(hwnd: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// The bar's height in pixels (none in full screen).
pub(super) fn height(hwnd: HWND) -> i32 {
    if super::video::is_fullscreen() { 0 } else { (BAR_DIP * scale(hwnd)).round() as i32 }
}

/// The bar's height for a DPI, before there's a window.
pub(super) fn height_for_dpi(dpi: u32) -> i32 {
    (BAR_DIP * dpi as f32 / 96.0).round() as i32
}

/// The bar's part at a client point, if any (`None` also for the empty
/// part, which drags the window).
pub(super) fn part_at(hwnd: HWND, x: i32, y: i32) -> Option<Part> {
    let h = height(hwnd);
    if y < 0 || y >= h {
        return None;
    }
    let s = scale(hwnd);
    let mut client = RECT::default();
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client) };
    let (x, w) = (x as f32, client.right as f32);
    let caption = CAPTION_DIP * s;
    let (more_from, more_to) = MORE_AT.get();
    Some(match x {
        x if x >= w - caption => Part::Close,
        x if x >= w - 2.0 * caption => Part::Max,
        x if x >= w - 3.0 * caption => Part::Min,
        x if x < BACK_DIP * s => Part::Back,
        x if x >= more_from && x < more_to => Part::More,
        _ => return None,
    })
}

pub(super) fn set_hover(part: Option<Part>) {
    if HOVER.replace(part) != part {
        invalidate();
    }
}

pub(super) fn set_pressed(part: Option<Part>) {
    if PRESSED.replace(part) != part {
        invalidate();
    }
}

pub(super) fn pressed() -> Option<Part> {
    PRESSED.get()
}

/// Windows' app theme: light (true) or dark.
pub(super) fn light_theme() -> bool {
    use windows::Win32::System::Registry::*;
    let mut value = 1u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    r.is_err() || value != 0
}

struct Theme {
    bar: D2D1_COLOR_F,
    text: D2D1_COLOR_F,
    dim: D2D1_COLOR_F,
    hover: D2D1_COLOR_F,
    press: D2D1_COLOR_F,
}

const fn rgb(r: u8, g: u8, b: u8) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: 1.0 }
}

const LIGHT: Theme = Theme {
    bar: rgb(0xF3, 0xF3, 0xF3),
    text: rgb(0x1A, 0x1A, 0x1A),
    dim: rgb(0x8A, 0x8A, 0x8A),
    hover: rgb(0xE3, 0xE3, 0xE3),
    press: rgb(0xD6, 0xD6, 0xD6),
};
const DARK: Theme = Theme {
    bar: rgb(0x20, 0x20, 0x20),
    text: rgb(0xFF, 0xFF, 0xFF),
    dim: rgb(0x9A, 0x9A, 0x9A),
    hover: rgb(0x2D, 0x2D, 0x2D),
    press: rgb(0x29, 0x29, 0x29),
};
const CLOSE_HOVER: D2D1_COLOR_F = rgb(0xC4, 0x2B, 0x1C);
const CLOSE_PRESS: D2D1_COLOR_F = rgb(0xB2, 0x2A, 0x1B);
const WHITE: D2D1_COLOR_F = rgb(0xFF, 0xFF, 0xFF);

/// Direct2D and DirectWrite, made once per window on the swap chain's device.
struct Drawer {
    dc: ID2D1DeviceContext,
    write: IDWriteFactory,
    /// The DPI scale its text formats were made for.
    scale: f32,
    title: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    caption_icons: IDWriteTextFormat,
    notice: IDWriteTextFormat,
    notice_small: IDWriteTextFormat,
}

impl Drawer {
    fn new(device: &windows::Win32::Graphics::Dxgi::IDXGIDevice, scale: f32) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dc = factory.CreateDevice(device)?.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let write: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let format = |family: windows::core::PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                let f = write.CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * scale,
                    w!("en-us"),
                )?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Ok(f)
            };
            // Windows 11's icon font; Windows 10's has the same glyphs.
            let icon_font = if font_exists(&write, w!("Segoe Fluent Icons")) {
                w!("Segoe Fluent Icons")
            } else {
                w!("Segoe MDL2 Assets")
            };
            let icons = format(icon_font, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            icons.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let caption_icons = format(icon_font, 10.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            caption_icons.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            let title = format(w!("Segoe UI Variable Text"), 12.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let notice = format(w!("Segoe UI Variable Display"), 24.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            notice.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            notice.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            let notice_small = format(w!("Segoe UI Variable Text"), 15.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            notice_small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            notice_small.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            Ok(Self { dc, write, scale, title, icons, caption_icons, notice, notice_small })
        }
    }
}

fn font_exists(write: &IDWriteFactory, name: windows::core::PCWSTR) -> bool {
    unsafe {
        let mut fonts = None;
        if write.GetSystemFontCollection(&mut fonts, false).is_err() {
            return false;
        }
        let Some(fonts) = fonts else { return false };
        let mut index = 0;
        let mut exists = BOOL(0);
        fonts.FindFamilyName(name, &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

/// What the bar shows.
pub(super) struct Bar<'a> {
    pub title: &'a str,
    pub active: bool,
    pub maximized: bool,
    /// No picture was drawn: start from black.
    pub clear: bool,
}

/// Where "..." is (px from the left), for its menu.
pub(super) fn more_at() -> (f32, f32) {
    MORE_AT.get()
}

/// Draws the bar (and any notice) over `surface`, the back buffer the
/// picture was just drawn into.
pub(super) fn draw(
    device: &windows::Win32::Graphics::Dxgi::IDXGIDevice,
    surface: &IDXGISurface,
    hwnd: HWND,
    bar: &Bar<'_>,
) -> Result<()> {
    let s = scale(hwnd);
    DRAWER.with_borrow_mut(|d| {
        if d.as_ref().is_none_or(|d| d.scale != s) {
            *d = Some(Drawer::new(device, s)?);
        }
        let d = d.as_ref().unwrap();
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let target = d.dc.CreateBitmapFromDxgiSurface(surface, Some(&props))?;
            d.dc.SetTarget(&target);
            d.dc.BeginDraw();
            paint(d, hwnd, bar, s);
            let r = d.dc.EndDraw(None, None);
            d.dc.SetTarget(None);
            r
        }
    })
}

unsafe fn paint(d: &Drawer, hwnd: HWND, bar: &Bar<'_>, s: f32) {
    let dc = &d.dc;
    if bar.clear {
        unsafe { dc.Clear(Some(&rgb(0, 0, 0))) };
    }
    let theme = if light_theme() { &LIGHT } else { &DARK };
    let brush = |c: &D2D1_COLOR_F| unsafe { dc.CreateSolidColorBrush(c, None) };
    let mut client = RECT::default();
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client) };
    let w = client.right as f32;
    let h = height(hwnd) as f32;
    let fill = |r: D2D_RECT_F, c: &D2D1_COLOR_F| {
        if let Ok(b) = brush(c) {
            unsafe { dc.FillRectangle(&r, &b) };
        }
    };
    let text = |t: &str, format: &IDWriteTextFormat, r: D2D_RECT_F, c: &D2D1_COLOR_F| {
        let t: Vec<u16> = t.encode_utf16().collect();
        if let Ok(b) = brush(c) {
            unsafe {
                dc.DrawText(&t, format, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    };

    if h > 0.0 {
        let fg = if bar.active { &theme.text } else { &theme.dim };
        fill(D2D_RECT_F { left: 0.0, top: 0.0, right: w, bottom: h }, &theme.bar);
        let hover = HOVER.get();
        let pressed = PRESSED.get();
        let button = |part: Part, r: D2D_RECT_F, glyph: &str, format: &IDWriteTextFormat| {
            let close = part == Part::Close;
            let mut color = fg;
            if pressed == Some(part) {
                fill(r, if close { &CLOSE_PRESS } else { &theme.press });
                if close {
                    color = &WHITE;
                }
            } else if hover == Some(part) && pressed.is_none() {
                fill(r, if close { &CLOSE_HOVER } else { &theme.hover });
                if close {
                    color = &WHITE;
                }
            }
            text(glyph, format, r, color);
        };
        // Back, the name, "...".
        let back = BACK_DIP * s;
        button(Part::Back, D2D_RECT_F { left: 0.0, top: 0.0, right: back, bottom: h }, "\u{E72B}", &d.icons);
        let caption = CAPTION_DIP * s;
        let title_x = back + 4.0 * s;
        let room = (w - 3.0 * caption - MORE_DIP * s - title_x - 8.0 * s).max(0.0);
        let title: Vec<u16> = bar.title.encode_utf16().collect();
        let mut title_w = room;
        if let Ok(layout) = unsafe { d.write.CreateTextLayout(&title, &d.title, room, h) } {
            let ellipsis = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            let sign = unsafe { d.write.CreateEllipsisTrimmingSign(&d.title) }.ok();
            let _ = unsafe { layout.SetTrimming(&ellipsis, sign.as_ref()) };
            let mut m = DWRITE_TEXT_METRICS::default();
            if unsafe { layout.GetMetrics(&mut m) }.is_ok() {
                title_w = m.width.min(room);
            }
            if let Ok(b) = brush(fg) {
                unsafe { dc.DrawTextLayout(D2D_POINT_2F { x: title_x, y: 0.0 }, &layout, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP) };
            }
        }
        let more_from = title_x + title_w + 8.0 * s;
        let more_to = more_from + MORE_DIP * s;
        MORE_AT.set((more_from, more_to));
        let more = D2D_RECT_F { left: more_from, top: 0.0, right: more_to, bottom: h };
        button(Part::More, more, "\u{E712}", &d.icons);
        // Minimize, maximize (restore), close.
        let at = |i: f32| D2D_RECT_F { left: w - (3.0 - i) * caption, top: 0.0, right: w - (2.0 - i) * caption, bottom: h };
        button(Part::Min, at(0.0), "\u{E921}", &d.caption_icons);
        button(Part::Max, at(1.0), if bar.maximized { "\u{E923}" } else { "\u{E922}" }, &d.caption_icons);
        button(Part::Close, at(2.0), "\u{E8BB}", &d.caption_icons);
    }

    let notice = notice();
    if notice != Notice::None {
        // The whole window below the bar, like Phone Link's "Unlock your
        // mobile device": the app's background, a phone, a title, a line.
        let area = D2D_RECT_F { left: 0.0, top: h, right: w, bottom: client.bottom as f32 };
        fill(area, &theme.bar);
        let cx = (area.left + area.right) / 2.0;
        let mid = (area.top + area.bottom) / 2.0;
        // The phone: rounded, a soft blue-grey gradient, a dark outline.
        let (pw, ph) = (92.0 * s, 190.0 * s);
        let phone = D2D_RECT_F { left: cx - pw / 2.0, top: mid - 40.0 * s - ph, right: cx + pw / 2.0, bottom: mid - 40.0 * s };
        let rounded = D2D1_ROUNDED_RECT { rect: phone, radiusX: 10.0 * s, radiusY: 10.0 * s };
        let gradient = |top: D2D1_COLOR_F, bottom: D2D1_COLOR_F, from: f32, to: f32| unsafe {
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: top },
                D2D1_GRADIENT_STOP { position: 1.0, color: bottom },
            ];
            let stops = ID2D1RenderTarget::CreateGradientStopCollection(dc, &stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: D2D_POINT_2F { x: cx, y: from },
                endPoint: D2D_POINT_2F { x: cx, y: to },
            };
            dc.CreateLinearGradientBrush(&props, None, &stops).ok()
        };
        unsafe {
            if let Some(b) = gradient(rgb(0xD9, 0xE0, 0xEC), rgb(0xA9, 0xB8, 0xD4), phone.top, phone.bottom) {
                dc.FillRoundedRectangle(&rounded, &b);
            }
            if let Ok(b) = brush(&theme.text) {
                dc.DrawRoundedRectangle(&rounded, &b, 2.5 * s, None);
            }
            // The badge near its top: a blue circle with the symbol.
            let badge = D2D1_ELLIPSE {
                point: D2D_POINT_2F { x: cx, y: phone.top + 44.0 * s },
                radiusX: 20.0 * s,
                radiusY: 20.0 * s,
            };
            let (b_top, b_bottom) = (badge.point.y - 20.0 * s, badge.point.y + 20.0 * s);
            if let Some(b) = gradient(rgb(0x2F, 0x6F, 0xEB), rgb(0x5B, 0x3F, 0xD9), b_top, b_bottom) {
                dc.FillEllipse(&badge, &b);
            }
        }
        let (glyph, title, detail) = match notice {
            Notice::AllowAgain => (
                "\u{E768}",
                "Allow sharing on your phone",
                "Android asks again after the phone locks. Tap Start on your phone to carry on.",
            ),
            _ => (
                "\u{E785}",
                "Unlock your phone",
                "Nothing is shared while it's locked. Unlock it to carry on; sharing stops after 3 minutes locked.",
            ),
        };
        let badge_box = D2D_RECT_F { left: cx - 20.0 * s, top: phone.top + 24.0 * s, right: cx + 20.0 * s, bottom: phone.top + 64.0 * s };
        text(glyph, &d.icons, badge_box, &WHITE);
        let line = |top: f32, bottom: f32| D2D_RECT_F { left: area.left + 24.0 * s, top, right: area.right - 24.0 * s, bottom };
        text(title, &d.notice, line(mid + 4.0 * s, mid + 64.0 * s), &theme.text);
        text(detail, &d.notice_small, line(mid + 64.0 * s, mid + 130.0 * s), &theme.dim);
    }
}

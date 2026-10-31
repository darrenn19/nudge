//! Controlling the phone from its mirror window: mouse and keyboard become
//! `nudge_core::input` messages, which the session task sends (`super::show`)
//! and the phone carries out (`ControlService.kt`).
//!
//! - Left button: a finger (click = tap, hold = long press, drag = swipe).
//! - Right button or mouse "back" button: Back. Middle button: Home. Mouse
//!   "forward" button: Recents.
//! - Mouse wheel: scrolling where the pointer is, a fixed step per notch.
//!   Touchpad (two-finger) scrolling: like a finger on the phone. The
//!   content follows the fingers as it would on this PC, measured on the
//!   picture (`TOUCHPAD_PX_PER_NOTCH`); when they leave the pad the phone's
//!   finger lets go at their speed (`PanEnd`), so quick swipes fling and
//!   half swipes snap back. The touchpad's raw reports tell when fingers are
//!   on it (they flow only then); Windows' momentum scrolling after a lift
//!   is dropped, the phone flings by itself.
//! - Typing goes into the phone's focused text field, with the usual keys:
//!   Backspace / Delete (Ctrl: a word), arrows (Ctrl: by word; Shift:
//!   select), Home / End (Ctrl: whole text), Ctrl+A/C/X/V. Enter sends (in
//!   a chat, its Send button); Shift+Enter is a new line. Esc is Back once
//!   control works.
//!
//! Positions are fractions of the picture (not the window), so the black
//! bars around it and the window's size don't matter.
//!
//! **Game mode** (F2, or the title bar menu): WASD and the arrow keys work a
//! game's on-screen joystick, as a second finger, while the mouse taps the
//! buttons. The keys push a finger from the joystick's centre toward the
//! direction held (diagonals too) and lift it when let go. Right-drag on the
//! picture places the joystick: press on its centre, release at its rim (a
//! plain right-click moves only the centre). Placed once per phone and
//! orientation, remembered in `%APPDATA%\Nudge\game-controls.txt`. Typing
//! is off in game mode; Esc is still Back. The phone is told (`Input::Mode`)
//! and sends a smaller picture meanwhile, for less latency.
//!
//! Everything here runs on the window's thread, except `set_status`.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::time::{Duration, Instant};

use nudge_core::input::{self, ControlStatus, Input, Key, TouchAction};
use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

/// What the phone last said about control: 255 = nothing yet.
static STATUS: AtomicU8 = AtomicU8::new(255);

/// While the button is held still, its position is sent again this often
/// (the phone lifts a finger it hears nothing about for a second).
const HOLD_REPEAT: Duration = Duration::from_millis(200);

/// The mouse's finger, and the game-mode joystick's.
const MOUSE: u8 = 0;
const JOYSTICK: u8 = 1;

/// Movement keys held, as bits.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Touchpad scrolling: how far the picture's content moves per 120 units of
/// scroll (one wheel notch's worth), in this window's pixels. About what
/// Windows apps scroll for the same finger movement.
const TOUCHPAD_PX_PER_NOTCH: f64 = 50.0;

/// After a scroll amount that isn't whole notches, treat scrolling as from
/// a touchpad for this long (touchpads sometimes send whole notches too).
const TOUCHPAD_MEMORY: Duration = Duration::from_millis(400);

/// The fingers have left the touchpad once its reports stop for this long
/// (a precision touchpad reports ~125 times a second while touched). Under
/// Android's 40 ms "finger stopped" limit, so a flick keeps its speed.
const TOUCHPAD_LIFT: Duration = Duration::from_millis(20);

/// Drags and touchpad scrolling go out at most this often; what comes in
/// between is merged (the newest position, the summed scrolling). A fast
/// mouse reports up to 1000 times a second: every one sent made a stream of
/// tiny packets competing with the video for Wi-Fi airtime, and gave the
/// phone more pieces of gesture than it can play smoothly.
const MOVE_EVERY: Duration = Duration::from_millis(5);

/// While two fingers rest on the touchpad mid-swipe, the phone hears this
/// often that they're still there (it lets go after 400 ms of nothing).
const SWIPE_KEEPALIVE: Duration = Duration::from_millis(200);

/// A touchpad reports ~100+ times a second while touched, resting fingers
/// included: this long without a report, the fingers have left it.
const PAD_SILENT: Duration = Duration::from_millis(60);

/// Windows' own scrolling from the touchpad is ignored this long after
/// two fingers were on it: the measured swipe has it covered.
const PAD_WHEEL_QUIET: Duration = Duration::from_millis(500);

/// A right-drag shorter than this (client pixels) only moves the centre.
const MIN_RADIUS_DRAG: i32 = 8;

/// A game's joystick: its centre as fractions of the picture, its radius as
/// a fraction of the picture's shorter side.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Joystick {
    x: f64,
    y: f64,
    r: f64,
}

impl Joystick {
    /// Where most games put it: low on the left.
    fn default_for(landscape: bool) -> Self {
        if landscape {
            Self { x: 0.16, y: 0.70, r: 0.14 }
        } else {
            Self { x: 0.25, y: 0.80, r: 0.14 }
        }
    }
}

#[derive(Default)]
struct State {
    to: Option<UnboundedSender<Input>>,
    hwnd: Option<HWND>,
    title: String,
    /// What the title shows now (control status, game mode), so it changes only with them.
    shown_status: Option<(u8, bool)>,
    /// Where the picture is drawn in the window (client pixels).
    picture: Option<RECT>,
    /// The finger, while the left button is down: its last position.
    down: Option<(u16, u16)>,
    last_sent: Option<Instant>,
    /// The first half of a character outside the BMP (emoji), from WM_CHAR.
    high_surrogate: Option<u16>,
    /// Game mode is on.
    game: bool,
    /// Movement keys held (`UP` | `DOWN` | ...).
    keys: u8,
    /// The joystick finger, while down: its position, and when it was last sent.
    joy_at: Option<(u16, u16)>,
    joy_sent: Option<Instant>,
    /// Where a right-drag placing the joystick started (client pixels).
    placing: Option<(i32, i32)>,
    /// Placed joysticks, by "<phone>\t<landscape|portrait>".
    joysticks: std::collections::HashMap<String, Joystick>,
    /// The "turn on control on the phone" message was shown.
    explained: bool,
    /// When scrolling last came from a touchpad.
    touchpad_at: Option<Instant>,
    /// The touchpad's last raw report: fingers are on it while they flow.
    /// `None` until one comes (no precision touchpad, or not supported).
    touchpad_report: Option<Instant>,
    /// Fingers are on the touchpad now.
    touchpad_touched: bool,
    /// A touchpad swipe went to the phone during this touch: it gets `PanEnd`.
    panned: bool,
    /// A drag position not sent yet (`MOVE_EVERY`).
    move_pending: Option<(u16, u16)>,
    /// Touchpad scrolling not sent yet: where it started, and the content's
    /// movement summed (share of the picture, 32767 = all of it).
    pan_pending: Option<(u16, u16, f64, f64)>,
    /// When a drag or touchpad scroll last went out.
    moved_at: Option<Instant>,
    /// The touchpad's fingers, measured (`touchpad`).
    pad: super::touchpad::Touchpad,
    /// A two-finger swipe on the touchpad, while it lasts.
    swipe: Option<PadSwipe>,
    /// Swipe movement not sent yet: where, and mm moved (`MOVE_EVERY`).
    swipe_pending: Option<(u16, u16, f64, f64)>,
    /// When two or more fingers were last on the touchpad.
    two_fingers_at: Option<Instant>,
}

/// A two-finger swipe on the touchpad, sent to the phone 1:1 in mm.
struct PadSwipe {
    /// The fingers making it: another finger joining or one leaving starts afresh.
    ids: Vec<u32>,
    /// Their centre (mm on the pad) as last seen.
    at: (f64, f64),
    /// Where on the picture the pointer was when it started.
    from: (u16, u16),
    /// Movement has gone to the phone (it gets `PanEnd` at the end).
    moved: bool,
    /// When something last went to the phone about it.
    sent: Instant,
}

/// Asks Windows for the precision touchpad's raw reports while this window
/// is in front: they say when fingers are on the pad.
pub(super) fn listen_to_touchpad(hwnd: HWND) {
    use windows::Win32::UI::Input::{RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS, RegisterRawInputDevices};
    let device = RAWINPUTDEVICE {
        usUsagePage: 0x0D, // digitizer
        usUsage: 0x05,     // touch pad
        dwFlags: RAWINPUTDEVICE_FLAGS(0), // while this window is in front
        hwndTarget: hwnd,
    };
    let ok = unsafe { RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if let Err(e) = ok {
        log::info!("mirror: no touchpad reports ({e}); touchpad swipes end after a pause");
    }
}

/// Once per session: the phone ignores clicks until its user turns control
/// on. A message box of its own (another thread): the window keeps playing.
fn explain_setup(phone: &str) {
    let text = format!(
        "{phone} needs one setting before it can be controlled from here.\n\n\
         On the phone: tap \"Allow control\" on the \"Your screen is being shared\" \
         notification (or Settings > Accessibility > Installed apps > Nudge remote control), \
         and turn it on.\n\nThen click here again. It's asked only once."
    );
    std::thread::spawn(move || {
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                windows::core::w!("Control the phone"),
                MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
            );
        }
    });
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Where input goes (the session task).
pub(super) fn set_sender(to: UnboundedSender<Input>) {
    STATUS.store(255, Relaxed);
    STATE.with_borrow_mut(|s| *s = State { to: Some(to), ..State::default() });
}

pub(super) fn set_window(hwnd: HWND, title: &str) {
    STATE.with_borrow_mut(|s| {
        s.hwnd = Some(hwnd);
        s.title = title.to_owned();
        s.joysticks = load_joysticks();
    });
}

/// Turns game mode on or off (F2, or the title bar menu).
pub(super) fn toggle_game() {
    STATE.with_borrow_mut(|s| {
        s.game = !s.game;
        s.keys = 0;
        s.high_surrogate = None;
        update_joystick(s);
        // The phone sends a smaller picture in game mode: less time to encode
        // and to send each frame.
        let mode = if s.game { input::GAME } else { 0 };
        send(s, Input::Mode(mode));
        log::info!("mirror: game mode {}", if s.game { "on" } else { "off" });
    });
}

fn joysticks_file() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("Nudge").join("game-controls.txt"))
}

/// Lines of "<phone>\t<orientation>\t<x>\t<y>\t<r>".
fn load_joysticks() -> std::collections::HashMap<String, Joystick> {
    let text = joysticks_file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            let [name, orientation, x, y, r] = f[..] else { return None };
            let j = Joystick { x: x.parse().ok()?, y: y.parse().ok()?, r: r.parse().ok()? };
            Some((format!("{name}\t{orientation}"), j))
        })
        .collect()
}

fn save_joysticks(all: &std::collections::HashMap<String, Joystick>) {
    let Some(path) = joysticks_file() else { return };
    let mut text = String::new();
    for (k, j) in all {
        text += &format!("{k}\t{}\t{}\t{}\n", j.x, j.y, j.r);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, text) {
        log::warn!("mirror: couldn't save the joystick: {e}");
    }
}

fn landscape(s: &State) -> bool {
    s.picture.is_some_and(|r| r.right - r.left > r.bottom - r.top)
}

fn joystick_key(s: &State) -> String {
    // The title is the phone's name; tabs would break the file.
    let name = s.title.replace('\t', " ");
    format!("{name}\t{}", if landscape(s) { "landscape" } else { "portrait" })
}

fn joystick(s: &State) -> Joystick {
    s.joysticks.get(&joystick_key(s)).copied().unwrap_or_else(|| Joystick::default_for(landscape(s)))
}

/// Moves the joystick finger to match the keys held: down at the centre
/// when the first key goes down, pushed toward the direction, lifted when
/// none is held.
fn update_joystick(s: &mut State) {
    let keys = if s.game { s.keys } else { 0 };
    let dir_x = f64::from(keys & RIGHT != 0) - f64::from(keys & LEFT != 0);
    let dir_y = f64::from(keys & DOWN != 0) - f64::from(keys & UP != 0);
    let Some(pic) = s.picture else { return };
    if dir_x == 0.0 && dir_y == 0.0 {
        // No keys, or opposite ones cancelling out: let go.
        if let Some((x, y)) = s.joy_at.take() {
            send_joy(s, Input::Touch { action: TouchAction::Up, pointer: JOYSTICK, x, y });
        }
        return;
    }
    let j = joystick(s);
    let (w, h) = ((pic.right - pic.left).max(1) as f64, (pic.bottom - pic.top).max(1) as f64);
    let len = (dir_x * dir_x + dir_y * dir_y).sqrt();
    let reach = j.r * w.min(h);
    let to = |f: f64| (f.clamp(0.0, 1.0) * 65535.0).round() as u16;
    let target = (to(j.x + dir_x / len * reach / w), to(j.y + dir_y / len * reach / h));
    match s.joy_at {
        None => {
            let (cx, cy) = (to(j.x), to(j.y));
            send_joy(s, Input::Touch { action: TouchAction::Down, pointer: JOYSTICK, x: cx, y: cy });
            send_joy(s, Input::Touch { action: TouchAction::Move, pointer: JOYSTICK, x: target.0, y: target.1 });
        }
        Some(at) if at == target => return,
        Some(_) => send_joy(s, Input::Touch { action: TouchAction::Move, pointer: JOYSTICK, x: target.0, y: target.1 }),
    }
    s.joy_at = Some(target);
}

fn send_joy(s: &mut State, input: Input) {
    if let Some(to) = &s.to {
        let _ = to.send(input);
    }
    s.joy_sent = Some(Instant::now());
}

fn movement(vk: VIRTUAL_KEY) -> Option<u8> {
    Some(match vk {
        VK_W | VK_UP => UP,
        VK_S | VK_DOWN => DOWN,
        VK_A | VK_LEFT => LEFT,
        VK_D | VK_RIGHT => RIGHT,
        _ => return None,
    })
}

/// Game mode's keys and right button. `None`: not for game mode.
fn handle_game(s: &mut State, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_KEYDOWN | WM_KEYUP => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            let Some(bit) = movement(vk) else {
                // Esc (Back), F2, F11 go to the window; the rest is ignored.
                return if matches!(vk, VK_ESCAPE | VK_F2 | VK_F11) { None } else { Some(LRESULT(0)) };
            };
            if msg == WM_KEYDOWN {
                s.keys |= bit;
            } else {
                s.keys &= !bit;
            }
            update_joystick(s);
        }
        WM_CHAR => {} // no typing in game mode
        // Another window has the keyboard: its key-ups won't come here.
        WM_KILLFOCUS => {
            s.keys = 0;
            update_joystick(s);
            return None;
        }
        WM_RBUTTONDOWN => {
            let (x, y) = client_point(lp);
            place(s, x, y, false)?;
            s.placing = Some((x, y));
        }
        WM_RBUTTONUP => {
            let (from_x, from_y) = s.placing.take()?;
            let pic = s.picture?;
            let (w, h) = ((pic.right - pic.left).max(1) as f64, (pic.bottom - pic.top).max(1) as f64);
            let (x, y) = client_point(lp);
            let mut j = joystick(s);
            j.x = ((from_x - pic.left) as f64 / w).clamp(0.0, 1.0);
            j.y = ((from_y - pic.top) as f64 / h).clamp(0.0, 1.0);
            let drag = f64::from((x - from_x).pow(2) + (y - from_y).pow(2)).sqrt();
            if drag >= f64::from(MIN_RADIUS_DRAG) {
                j.r = (drag / w.min(h)).clamp(0.02, 0.5);
            }
            let key = joystick_key(s);
            log::info!("mirror: joystick for {key:?} at {j:?}");
            s.joysticks.insert(key, j);
            save_joysticks(&s.joysticks);
            // A finger already down moves to the new place.
            if let Some((x, y)) = s.joy_at.take() {
                send_joy(s, Input::Touch { action: TouchAction::Up, pointer: JOYSTICK, x, y });
            }
            update_joystick(s);
        }
        _ => return None,
    }
    Some(LRESULT(0))
}

/// Where the latest picture was drawn, in client pixels.
pub(super) fn set_picture(r: RECT) {
    STATE.with_borrow_mut(|s| s.picture = Some(r));
}

/// From the session task: the phone's `Status`.
pub(super) fn set_status(status: ControlStatus) {
    STATUS.store(status as u8, Relaxed);
}

/// The phone said control works.
pub(super) fn ready() -> bool {
    STATUS.load(Relaxed) == ControlStatus::Ready as u8
}

/// Sends `input`, after any merged drag or scrolling still waiting (so
/// everything stays in order).
fn send(s: &mut State, input: Input) {
    flush_moves(s);
    emit(s, input);
}

fn emit(s: &mut State, input: Input) {
    if let Some(to) = &s.to {
        let _ = to.send(input);
    }
    s.last_sent = Some(Instant::now());
}

/// Sends the merged drag position and touchpad scrolling, if any.
fn flush_moves(s: &mut State) {
    if let Some((x, y)) = s.move_pending.take() {
        emit(s, Input::Touch { action: TouchAction::Move, pointer: MOUSE, x, y });
        s.moved_at = Some(Instant::now());
    }
    if let Some((x, y, dx, dy)) = s.pan_pending.take() {
        let share = |v: f64| v.round().clamp(-32767.0, 32767.0) as i16;
        emit(s, Input::Pan { x, y, dx: share(dx), dy: share(dy) });
        s.moved_at = Some(Instant::now());
    }
    if let Some((x, y, dx, dy)) = s.swipe_pending.take() {
        let hundredths = |mm: f64| (mm * 100.0).round().clamp(-32767.0, 32767.0) as i16;
        emit(s, Input::Swipe { x, y, dx: hundredths(dx), dy: hundredths(dy) });
        s.moved_at = Some(Instant::now());
        if let Some(w) = &mut s.swipe {
            w.sent = Instant::now();
        }
    }
}

/// A frame of touchpad fingers: two or more moving together are a swipe,
/// sent as the distance they moved; fewer end it.
fn on_pad(s: &mut State, hwnd: HWND, fingers: Vec<super::touchpad::Contact>) {
    if fingers.len() < 2 {
        if let Some(w) = s.swipe.take()
            && w.moved
        {
            send(s, Input::PanEnd); // lifted: the phone's finger lets go at the speed it had
        }
        return;
    }
    s.two_fingers_at = Some(Instant::now());
    let mut ids: Vec<u32> = fingers.iter().map(|f| f.id).collect();
    ids.sort_unstable();
    let n = fingers.len() as f64;
    let at = (fingers.iter().map(|f| f.x).sum::<f64>() / n, fingers.iter().map(|f| f.y).sum::<f64>() / n);
    match &mut s.swipe {
        Some(w) if w.ids == ids => {
            let (dx, dy) = (at.0 - w.at.0, at.1 - w.at.1);
            w.at = at;
            if dx == 0.0 && dy == 0.0 {
                return;
            }
            w.moved = true;
            let from = w.from;
            let p = s.swipe_pending.get_or_insert((from.0, from.1, 0.0, 0.0));
            p.2 += dx;
            p.3 += dy;
            if move_due(s) {
                flush_moves(s);
            }
        }
        _ => {
            // A new swipe, or fingers joined or left: carry on from here (no jump).
            let moved = s.swipe.as_ref().is_some_and(|w| w.moved);
            let from = s.swipe.as_ref().map(|w| w.from).or_else(|| {
                let mut pt = POINT::default();
                unsafe {
                    GetCursorPos(&mut pt).ok()?;
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                place(s, pt.x, pt.y, true)
            });
            let Some(from) = from else { return };
            s.swipe = Some(PadSwipe { ids, at, from, moved, sent: Instant::now() });
        }
    }
}

/// A drag or scroll may go out now (`MOVE_EVERY`).
fn move_due(s: &State) -> bool {
    s.moved_at.is_none_or(|t| t.elapsed() >= MOVE_EVERY)
}

/// A client position as a fraction of the picture; `None` outside it,
/// unless `clamp`.
fn place(s: &State, x: i32, y: i32, clamp: bool) -> Option<(u16, u16)> {
    let r = s.picture?;
    let (w, h) = ((r.right - r.left).max(1) as f64, (r.bottom - r.top).max(1) as f64);
    let fx = (x - r.left) as f64 / (w - 1.0).max(1.0);
    let fy = (y - r.top) as f64 / (h - 1.0).max(1.0);
    if !clamp && !((0.0..=1.0).contains(&fx) && (0.0..=1.0).contains(&fy)) {
        return None;
    }
    let to = |f: f64| (f.clamp(0.0, 1.0) * 65535.0).round() as u16;
    Some((to(fx), to(fy)))
}

fn client_point(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

fn held(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

/// Handles a window message that's input for the phone. `None`: not ours,
/// the window handles it as usual.
pub(super) fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    // Capture calls send messages to this window straight away, which come
    // back here: make them once the state is let go of.
    let mut capture: Option<bool> = None;
    let r = STATE.with_borrow_mut(|s| {
        s.to.as_ref()?;
        if s.game
            && let Some(r) = handle_game(s, msg, wp, lp)
        {
            return Some(r);
        }
        match msg {
            WM_LBUTTONDOWN => {
                let (x, y) = client_point(lp);
                let (px, py) = place(s, x, y, false)?;
                capture = Some(true);
                s.down = Some((px, py));
                send(s, Input::Touch { action: TouchAction::Down, pointer: MOUSE, x: px, y: py });
            }
            WM_MOUSEMOVE => {
                s.down?;
                let (x, y) = client_point(lp);
                let p = place(s, x, y, true)?;
                if s.down == Some(p) {
                    return Some(LRESULT(0));
                }
                s.down = Some(p);
                // Merged with the moves of the next few ms (`MOVE_EVERY`).
                s.move_pending = Some(p);
                if move_due(s) {
                    flush_moves(s);
                }
            }
            WM_LBUTTONUP => {
                let last = s.down.take()?;
                let (x, y) = client_point(lp);
                let (px, py) = place(s, x, y, true).unwrap_or(last);
                send(s, Input::Touch { action: TouchAction::Up, pointer: MOUSE, x: px, y: py });
                // `down` is cleared first: releasing sends WM_CAPTURECHANGED.
                capture = Some(false);
            }
            // Another window took the mouse mid-drag (Alt+Tab, a popup).
            WM_CAPTURECHANGED => {
                let (x, y) = s.down.take()?;
                send(s, Input::Touch { action: TouchAction::Cancel, pointer: MOUSE, x, y });
            }
            WM_RBUTTONUP => send(s, Input::Key(Key::Back, 0)),
            WM_MBUTTONUP => send(s, Input::Key(Key::Home, 0)),
            WM_XBUTTONUP => {
                let key = if (wp.0 >> 16) & 0xFFFF == XBUTTON1 as usize { Key::Back } else { Key::Recents };
                send(s, Input::Key(key, 0));
                return Some(LRESULT(1)); // XBUTTON messages want TRUE
            }
            WM_RBUTTONDOWN | WM_MBUTTONDOWN => {}
            WM_XBUTTONDOWN => return Some(LRESULT(1)),
            // A touchpad report: fingers are on it. Windows still needs it.
            WM_INPUT => {
                if s.touchpad_report.is_none() {
                    log::info!("mirror: touchpad reports coming (swipes end when the fingers lift)");
                }
                s.touchpad_report = Some(Instant::now());
                s.touchpad_touched = true;
                if let Some(fingers) = s.pad.on_input(lp) {
                    on_pad(s, hwnd, fingers);
                }
                return None;
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                // Wheel messages carry screen coordinates.
                let (sx, sy) = client_point(lp);
                let mut pt = POINT { x: sx, y: sy };
                unsafe {
                    let _ = ScreenToClient(hwnd, &mut pt);
                }
                let (x, y) = place(s, pt.x, pt.y, true)?;
                // Positive: away from the user (vertical), right (horizontal).
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                if delta % 120 != 0 {
                    s.touchpad_at = Some(Instant::now());
                }
                let touchpad = s.touchpad_at.is_some_and(|t| t.elapsed() < TOUCHPAD_MEMORY);
                // A touchpad measured from its own reports: swipes go 1:1 from
                // there (`on_pad`), Windows' version of them is left out.
                if s.pad.works()
                    && (touchpad || s.two_fingers_at.is_some_and(|t| t.elapsed() < PAD_WHEEL_QUIET))
                {
                    return Some(LRESULT(0));
                }
                // Windows' momentum after the fingers left: the phone's own
                // fling takes over instead.
                if touchpad && s.touchpad_report.is_some() && !s.touchpad_touched {
                    return Some(LRESULT(0));
                }
                if touchpad {
                    s.panned = true;
                    // The content moves as far as it would here, as a share
                    // of the picture: right scrolling moves content left.
                    let pic = s.picture?;
                    let px = f64::from(delta) / 120.0 * TOUCHPAD_PX_PER_NOTCH;
                    let share = |len: i32| px / f64::from(len.max(1)) * 32767.0;
                    let (dx, dy) = if msg == WM_MOUSEWHEEL {
                        (0.0, share(pic.bottom - pic.top))
                    } else {
                        (-share(pic.right - pic.left), 0.0)
                    };
                    // Summed with the next few ms of scrolling (`MOVE_EVERY`).
                    let p = s.pan_pending.get_or_insert((x, y, 0.0, 0.0));
                    p.2 += dx;
                    p.3 += dy;
                    if move_due(s) {
                        flush_moves(s);
                    }
                } else {
                    let (dx, dy) = if msg == WM_MOUSEWHEEL { (0, delta) } else { (delta, 0) };
                    send(s, Input::Scroll { x, y, dx, dy });
                }
            }
            WM_KEYDOWN => {
                let ctrl = held(VK_CONTROL) && !held(VK_MENU); // Ctrl+Alt is AltGr: a character
                let mods = if held(VK_SHIFT) { input::SHIFT } else { 0 } | if ctrl { input::CTRL } else { 0 };
                let (key, mods) = match VIRTUAL_KEY(wp.0 as u16) {
                    VK_BACK => (Key::Backspace, mods),
                    VK_DELETE => (Key::Delete, mods),
                    VK_RETURN => (Key::Enter, mods),
                    VK_LEFT => (Key::Left, mods),
                    VK_RIGHT => (Key::Right, mods),
                    VK_UP => (Key::Up, mods),
                    VK_DOWN => (Key::Down, mods),
                    VK_HOME => (Key::LineStart, mods),
                    VK_END => (Key::LineEnd, mods),
                    VK_BROWSER_BACK => (Key::Back, 0),
                    VK_BROWSER_HOME => (Key::Home, 0),
                    VK_A if ctrl => (Key::SelectAll, 0),
                    VK_C if ctrl => (Key::Copy, 0),
                    VK_X if ctrl => (Key::Cut, 0),
                    VK_V if ctrl => (Key::Paste, 0),
                    _ => return None, // Esc, F11 and the rest: the window's
                };
                send(s, Input::Key(key, mods));
            }
            WM_CHAR => {
                let unit = wp.0 as u16;
                let text = match unit {
                    0xD800..=0xDBFF => {
                        s.high_surrogate = Some(unit);
                        return Some(LRESULT(0));
                    }
                    0xDC00..=0xDFFF => {
                        let high = s.high_surrogate.take()?;
                        String::from_utf16(&[high, unit]).ok()?
                    }
                    // Control characters (Enter, Backspace, Ctrl+letter) went as keys.
                    u if u < 0x20 || u == 0x7F => return Some(LRESULT(0)),
                    u => String::from_utf16(&[u]).ok()?,
                };
                s.high_surrogate = None;
                send(s, Input::Text(text));
            }
            _ => return None,
        }
        Some(LRESULT(0))
    });
    unsafe {
        match capture {
            Some(true) => {
                SetCapture(hwnd);
            }
            Some(false) => {
                let _ = ReleaseCapture();
            }
            None => {}
        }
    }
    r
}

/// Esc: Back on the phone (once control works).
pub(super) fn back() {
    STATE.with_borrow_mut(|s| send(s, Input::Key(Key::Back, 0)));
}

/// Called often from the window loop: keeps a held button alive on the
/// phone, and shows in the title whether control needs turning on.
pub(super) fn tick() {
    // Setting the title sends the window a message: done after the state is let go of.
    let retitle = STATE.with_borrow_mut(|s| {
        if (s.move_pending.is_some() || s.pan_pending.is_some() || s.swipe_pending.is_some()) && move_due(s) {
            flush_moves(s);
        }
        // Reports stopped without saying the fingers lifted: they did.
        if s.swipe.is_some() && s.touchpad_report.is_none_or(|t| t.elapsed() >= PAD_SILENT) {
            on_pad(s, s.hwnd.unwrap_or_default(), Vec::new());
        }
        // Two fingers resting mid-swipe: still there (no movement).
        if let Some(w) = &s.swipe
            && w.moved
            && s.swipe_pending.is_none()
            && w.sent.elapsed() >= SWIPE_KEEPALIVE
        {
            let (x, y) = w.from;
            s.swipe_pending = Some((x, y, 0.0, 0.0));
            flush_moves(s);
        }
        if let Some((x, y)) = s.down
            && s.last_sent.is_none_or(|t| t.elapsed() >= HOLD_REPEAT)
        {
            send(s, Input::Touch { action: TouchAction::Move, pointer: MOUSE, x, y });
        }
        // The fingers left the touchpad: the phone lets go.
        if s.touchpad_touched && s.touchpad_report.is_some_and(|t| t.elapsed() >= TOUCHPAD_LIFT) {
            s.touchpad_touched = false;
            if std::mem::take(&mut s.panned) {
                log::info!("mirror: touchpad fingers lifted: swipe ends");
                send(s, Input::PanEnd);
            }
        }
        if let Some((x, y)) = s.joy_at
            && s.joy_sent.is_none_or(|t| t.elapsed() >= HOLD_REPEAT)
        {
            send_joy(s, Input::Touch { action: TouchAction::Move, pointer: JOYSTICK, x, y });
        }
        let status = STATUS.load(Relaxed);
        if s.shown_status == Some((status, s.game)) {
            return None;
        }
        if status == ControlStatus::NeedsSetup as u8 && !s.explained {
            s.explained = true;
            explain_setup(&s.title);
        }
        s.shown_status = Some((status, s.game));
        let hint = match status {
            x if x == ControlStatus::NeedsSetup as u8 => " (to control it, turn on Nudge on the phone)",
            x if x == ControlStatus::Unavailable as u8 => " (can't be controlled)",
            _ => "",
        };
        let game = if s.game { " - Game mode: WASD/arrows move, right-drag places the joystick, F2 to leave" } else { "" };
        Some((s.hwnd?, format!("{}{hint}{game}", s.title)))
    });
    if let Some((hwnd, title)) = retitle {
        let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
        unsafe {
            let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
        }
    }
}

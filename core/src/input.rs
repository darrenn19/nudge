//! Remote control during screen mirroring: the messages the viewing device
//! sends to control the shared one (taps, drags, scrolling, keys, text), and
//! the shared device's reply saying whether control works.
//!
//! They travel on the mirroring session as messages on [`STREAM`]
//! (`Session::send_message`): encrypted with the session, in order,
//! recovered when lost. Only the shared device's user can turn control on
//! (on Android, Nudge's Accessibility switch), and only while it's sharing:
//! a session exists only after they agreed, and ends when it locks.
//!
//! Positions are fractions of the shared picture as it's shown now (0 = left
//! or top edge, 65535 = right or bottom), so the viewer needs neither the
//! shared screen's size nor its rotation.
//!
//! Wire format (integers big-endian), one per message:
//!
//! | kind | body |
//! |---|---|
//! | 1 Touch | action u8 (0 down, 1 move, 2 up, 3 cancel), pointer u8, x u16, y u16 |
//! | 2 Scroll | x u16, y u16, dx i16, dy i16 (1/120 of a wheel notch; positive dy = wheel away from the user) |
//! | 3 Key | key u8 (see [`Key`]), then optionally mods u8 (1 Shift, 2 Ctrl) |
//! | 4 Text | UTF-8, at most [`MAX_TEXT`] bytes |
//! | 5 Status | u8 (see [`ControlStatus`]): from the shared device |
//! | 6 Pan | x u16, y u16, dx i16, dy i16: content dragged by this share of the picture (32767 = all of it); touchpad scrolling |
//! | 7 PanEnd | (none): the fingers left the touchpad: let go now, at the speed they were moving |
//! | 9 Swipe | x u16, y u16, dx i16, dy i16: the touchpad fingers moved this far, in 1/100 mm; the shared device's finger moves the same distance on its screen (1:1, as if on the phone) |
//! | 8 Mode | flags u8 (1 = game mode): the viewer's mode; the shared device trades picture size for latency in game mode |
//!
//! Keys with mods: Ctrl+Backspace / Ctrl+Delete delete a word, Ctrl+Left /
//! Right move by a word, Shift with a move selects, Home / End go to the
//! line's start or end (with Ctrl, the text's), Enter sends (in a chat: its
//! Send button) and Shift+Enter starts a new line.
//!
//! A held touch: the viewer repeats the last `Move` at least every
//! [`HOLD_REPEAT_MS`] while a pointer stays down without moving, and the
//! shared device lifts a pointer that hears nothing for [`HOLD_TIMEOUT_MS`]
//! (its `Up` was lost, or the viewer went away). The shared device sends
//! `Status` once it has heard from the viewer, and again when it changes.

/// The session stream input travels on (video is 0).
pub const STREAM: u8 = 1;

/// Longest text in one message, bytes.
pub const MAX_TEXT: usize = 1000;

/// While a pointer is held still, the viewer repeats its position this often.
pub const HOLD_REPEAT_MS: u64 = 250;

/// A pointer down with no message for this long is lifted.
pub const HOLD_TIMEOUT_MS: u64 = 1000;

const TOUCH: u8 = 1;
const SCROLL: u8 = 2;
const KEY: u8 = 3;
const TEXT: u8 = 4;
const STATUS: u8 = 5;
const PAN: u8 = 6;
const PAN_END: u8 = 7;
const MODE: u8 = 8;
const SWIPE: u8 = 9;

/// `Mode` flag: game mode is on.
pub const GAME: u8 = 1;

/// Modifier bits sent with a key.
pub const SHIFT: u8 = 1;
pub const CTRL: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAction {
    Down = 0,
    Move = 1,
    Up = 2,
    /// Lift without acting (the pointer left the window mid-drag).
    Cancel = 3,
}

/// Keys and system buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Back = 1,
    Home = 2,
    Recents = 3,
    Notifications = 4,
    QuickSettings = 5,
    Enter = 10,
    Backspace = 11,
    Delete = 12,
    Left = 13,
    Right = 14,
    Up = 15,
    Down = 16,
    /// Start of the line (with Ctrl: of the text).
    LineStart = 17,
    LineEnd = 18,
    SelectAll = 20,
    Copy = 21,
    Cut = 22,
    Paste = 23,
}

impl Key {
    fn from_u8(b: u8) -> Option<Self> {
        use Key::*;
        Some(match b {
            1 => Back,
            2 => Home,
            3 => Recents,
            4 => Notifications,
            5 => QuickSettings,
            10 => Enter,
            11 => Backspace,
            12 => Delete,
            13 => Left,
            14 => Right,
            15 => Up,
            16 => Down,
            17 => LineStart,
            18 => LineEnd,
            20 => SelectAll,
            21 => Copy,
            22 => Cut,
            23 => Paste,
            _ => return None,
        })
    }
}

/// Whether the shared device can be controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatus {
    /// It can't be (no way to inject input there).
    Unavailable = 0,
    Ready = 1,
    /// Its user has to turn it on first (Android: the Accessibility switch);
    /// they've been shown how.
    NeedsSetup = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Touch { action: TouchAction, pointer: u8, x: u16, y: u16 },
    /// Mouse wheel or touchpad scrolling at a point.
    Scroll { x: u16, y: u16, dx: i16, dy: i16 },
    /// A key, with modifiers (`SHIFT` | `CTRL`).
    Key(Key, u8),
    /// Typed text, inserted at the cursor (replacing a selection).
    Text(String),
    Status(ControlStatus),
    /// Touchpad scrolling at a point: drag the content by `dx`, `dy` as a
    /// share of the picture (32767 = its whole width or height). Moves the
    /// phone's content as far as the PC's would move, whatever the sizes.
    Pan { x: u16, y: u16, dx: i16, dy: i16 },
    /// The fingers left the touchpad: the phone's finger lets go at once,
    /// so a quick swipe flings and a slow one stops where it is.
    PanEnd,
    /// Touchpad scrolling measured on the touchpad itself: its fingers moved
    /// `dx`, `dy` (1/100 mm; down and right positive). The shared device's
    /// finger moves as far on its own screen, so a swipe feels as on the
    /// phone. Ends with `PanEnd`.
    Swipe { x: u16, y: u16, dx: i16, dy: i16 },
    /// The viewer's mode (`GAME`): sent when it changes. Older shared
    /// devices ignore it.
    Mode(u8),
}

impl Input {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Input::Touch { action, pointer, x, y } => {
                let mut m = vec![TOUCH, *action as u8, *pointer];
                m.extend_from_slice(&x.to_be_bytes());
                m.extend_from_slice(&y.to_be_bytes());
                m
            }
            Input::Scroll { x, y, dx, dy } => {
                let mut m = vec![SCROLL];
                for v in [*x, *y, *dx as u16, *dy as u16] {
                    m.extend_from_slice(&v.to_be_bytes());
                }
                m
            }
            Input::Key(k, 0) => vec![KEY, *k as u8],
            Input::Key(k, mods) => vec![KEY, *k as u8, *mods],
            Input::Text(t) => {
                let mut end = t.len().min(MAX_TEXT);
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                let mut m = vec![TEXT];
                m.extend_from_slice(&t.as_bytes()[..end]);
                m
            }
            Input::Status(s) => vec![STATUS, *s as u8],
            Input::Pan { x, y, dx, dy } => {
                let mut m = vec![PAN];
                for v in [*x, *y, *dx as u16, *dy as u16] {
                    m.extend_from_slice(&v.to_be_bytes());
                }
                m
            }
            Input::PanEnd => vec![PAN_END],
            Input::Mode(flags) => vec![MODE, *flags],
            Input::Swipe { x, y, dx, dy } => {
                let mut m = vec![SWIPE];
                for v in [*x, *y, *dx as u16, *dy as u16] {
                    m.extend_from_slice(&v.to_be_bytes());
                }
                m
            }
        }
    }

    /// `None` for anything malformed or unknown (a newer peer): ignore it.
    pub fn parse(m: &[u8]) -> Option<Self> {
        let (&kind, b) = m.split_first()?;
        let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
        Some(match kind {
            TOUCH if b.len() >= 6 => Input::Touch {
                action: match b[0] {
                    0 => TouchAction::Down,
                    1 => TouchAction::Move,
                    2 => TouchAction::Up,
                    3 => TouchAction::Cancel,
                    _ => return None,
                },
                pointer: b[1],
                x: u16_at(2),
                y: u16_at(4),
            },
            SCROLL if b.len() >= 8 => Input::Scroll {
                x: u16_at(0),
                y: u16_at(2),
                dx: u16_at(4) as i16,
                dy: u16_at(6) as i16,
            },
            KEY if !b.is_empty() => Input::Key(Key::from_u8(b[0])?, b.get(1).copied().unwrap_or(0) & (SHIFT | CTRL)),
            TEXT if b.len() <= MAX_TEXT => Input::Text(String::from_utf8(b.to_vec()).ok()?),
            STATUS if !b.is_empty() => Input::Status(match b[0] {
                0 => ControlStatus::Unavailable,
                1 => ControlStatus::Ready,
                2 => ControlStatus::NeedsSetup,
                _ => return None,
            }),
            PAN if b.len() >= 8 => Input::Pan {
                x: u16_at(0),
                y: u16_at(2),
                dx: u16_at(4) as i16,
                dy: u16_at(6) as i16,
            },
            PAN_END => Input::PanEnd,
            MODE if !b.is_empty() => Input::Mode(b[0]),
            SWIPE if b.len() >= 8 => Input::Swipe {
                x: u16_at(0),
                y: u16_at(2),
                dx: u16_at(4) as i16,
                dy: u16_at(6) as i16,
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let all = [
            Input::Touch { action: TouchAction::Down, pointer: 0, x: 0, y: 65535 },
            Input::Touch { action: TouchAction::Cancel, pointer: 3, x: 1234, y: 4321 },
            Input::Scroll { x: 10, y: 20, dx: -120, dy: 360 },
            Input::Key(Key::Back, 0),
            Input::Key(Key::Paste, 0),
            Input::Key(Key::Backspace, CTRL),
            Input::Key(Key::LineEnd, SHIFT | CTRL),
            Input::Pan { x: 1, y: 2, dx: -300, dy: 4000 },
            Input::PanEnd,
            Input::Mode(GAME),
            Input::Mode(0),
            Input::Swipe { x: 3, y: 4, dx: -250, dy: 1234 },
            Input::Text("héllo 👋".into()),
            Input::Text(String::new()),
            Input::Status(ControlStatus::NeedsSetup),
        ];
        for i in all {
            assert_eq!(Input::parse(&i.encode()), Some(i));
        }
    }

    #[test]
    fn long_text_is_cut_on_a_character() {
        let t = "é".repeat(MAX_TEXT); // 2 bytes each
        let Some(Input::Text(got)) = Input::parse(&Input::Text(t).encode()) else { panic!() };
        assert_eq!(got.len(), MAX_TEXT);
        assert!(got.chars().all(|c| c == 'é'));
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(Input::parse(&[]), None);
        assert_eq!(Input::parse(&[TOUCH, 0, 0, 1]), None);
        assert_eq!(Input::parse(&[TOUCH, 9, 0, 0, 0, 0, 0]), None);
        assert_eq!(Input::parse(&[KEY, 99]), None);
        assert_eq!(Input::parse(&[TEXT, 0xFF]), None);
        assert_eq!(Input::parse(&[77]), None);
    }
}

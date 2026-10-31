//! The mirror window's settings (its "..." menu), kept between sessions in
//! `%APPDATA%\Nudge\mirror-window.txt`, one `name=on|off` per line: game
//! mode, full screen, keep on top, and where the phone's sound plays. Saved
//! as soon as one changes; applied when a mirror window opens.

use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Settings {
    pub game: bool,
    pub fullscreen: bool,
    pub on_top: bool,
    /// The phone's sound plays on this PC.
    pub audio: bool,
}

static CURRENT: Mutex<Option<Settings>> = Mutex::new(None);

fn dir() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("APPDATA")?).join("Nudge"))
}

fn load() -> Settings {
    let mut s = Settings::default();
    let Some(dir) = dir() else { return s };
    // Before this file, the sound's place had a file of its own.
    if let Ok(t) = std::fs::read_to_string(dir.join("mirror-audio.txt")) {
        s.audio = t.trim() == "pc";
    }
    let text = std::fs::read_to_string(dir.join("mirror-window.txt")).unwrap_or_default();
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else { continue };
        let on = value.trim() == "on";
        match name.trim() {
            "game" => s.game = on,
            "fullscreen" => s.fullscreen = on,
            "on_top" => s.on_top = on,
            "audio" => s.audio = on,
            _ => {}
        }
    }
    s
}

/// The settings as saved.
pub(super) fn get() -> Settings {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(load)
}

/// Changes one and saves them all.
pub(super) fn change(f: impl FnOnce(&mut Settings)) {
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    let s = current.get_or_insert_with(load);
    f(s);
    let on = |b: bool| if b { "on" } else { "off" };
    let text = format!(
        "game={}\nfullscreen={}\non_top={}\naudio={}\n",
        on(s.game),
        on(s.fullscreen),
        on(s.on_top),
        on(s.audio)
    );
    let Some(dir) = dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(dir.join("mirror-window.txt"), text) {
        log::warn!("mirror: couldn't save the window's settings: {e}");
    }
}

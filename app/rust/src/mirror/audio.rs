//! The phone's sound, played on this PC ("Play phone audio on this PC" in
//! the mirror window's "..." menu).
//!
//! The phone captures what its apps play (Android's playback capture, part
//! of the same screen-sharing consent) and sends it as raw PCM on
//! `input::AUDIO_STREAM`: 48 kHz stereo, 10 ms a frame, about 1.5 Mbit/s,
//! nothing to decode. Here it goes through a short buffer that absorbs
//! network jitter into the default output device (WASAPI, shared mode;
//! Windows converts the rate if the device runs at another).
//!
//! The choice is remembered with the window's other settings (`super::settings`).

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use nudge_core::input::{AUDIO_CHANNELS, AUDIO_RATE, Input};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const BYTES_PER_FRAME: usize = AUDIO_CHANNELS as usize * 2;
const BYTES_PER_MS: usize = AUDIO_RATE as usize / 1000 * BYTES_PER_FRAME;

/// Playback starts (and restarts after running dry) once this much is
/// buffered: room for network jitter without a gap.
const START_MS: usize = 30;
/// More than this buffered (the network caught up after a pause): the
/// oldest is dropped down to `START_MS`, so the sound stays with the picture.
const MOST_MS: usize = 120;

/// The user wants the phone's sound here (saved, `super::settings`).
pub(super) fn wanted() -> bool {
    super::settings::get().audio
}

/// From the menu: tells the phone, and remembers it.
pub(super) fn set_wanted(on: bool) {
    super::settings::change(|s| s.audio = on);
    log::info!("mirror: phone audio {}", if on { "on this PC" } else { "on the phone" });
    super::control::send_input(Input::Audio(on));
}

/// Plays PCM handed to it, on a thread of its own; stops when dropped.
pub(super) struct Speaker {
    to: Sender<Vec<u8>>,
}

impl Speaker {
    pub fn new() -> Self {
        let (to, from) = mpsc::channel();
        std::thread::Builder::new()
            .name("nudge-mirror-audio".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
                let mut buffered = VecDeque::new();
                // The device can go away (headphones unplugged): start again on the new default.
                loop {
                    match play(&from, &mut buffered) {
                        Ok(()) => break, // the session ended
                        Err(e) => {
                            log::warn!("mirror audio: {e}; trying again");
                            std::thread::sleep(Duration::from_millis(500));
                            if matches!(from.try_recv(), Err(TryRecvError::Disconnected)) {
                                break;
                            }
                        }
                    }
                }
            })
            .expect("couldn't start the audio thread");
        Self { to }
    }

    pub fn push(&self, pcm: Vec<u8>) {
        let _ = self.to.send(pcm);
    }
}

fn play(from: &Receiver<Vec<u8>>, buffered: &mut VecDeque<u8>) -> windows::core::Result<()> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: AUDIO_CHANNELS,
            nSamplesPerSec: AUDIO_RATE,
            nAvgBytesPerSec: AUDIO_RATE * BYTES_PER_FRAME as u32,
            nBlockAlign: BYTES_PER_FRAME as u16,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            200_000, // 20 ms, in 100 ns units
            0,
            &format,
            None,
        )?;
        let ready = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(ready)?;
        let render: IAudioRenderClient = client.GetService()?;
        let size = client.GetBufferSize()?;
        client.Start()?;
        log::info!("mirror audio: playing ({} ms device buffer)", size as usize * 1000 / AUDIO_RATE as usize);

        let mut playing = false;
        let mut stats = (Instant::now(), 0u32, 0u32); // (since, underruns, trims)
        let result = loop {
            WaitForSingleObject(ready, 50);
            loop {
                match from.try_recv() {
                    Ok(pcm) => buffered.extend(pcm),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = client.Stop();
                        let _ = CloseHandle(ready);
                        return Ok(());
                    }
                }
            }
            // Too far behind: drop the oldest.
            if buffered.len() > MOST_MS * BYTES_PER_MS {
                let excess = buffered.len() - START_MS * BYTES_PER_MS;
                buffered.drain(..excess - excess % BYTES_PER_FRAME);
                stats.2 += 1;
            }
            if !playing {
                if buffered.len() < START_MS * BYTES_PER_MS {
                    continue;
                }
                playing = true;
            }
            let padding = match client.GetCurrentPadding() {
                Ok(p) => p,
                Err(e) => break Err(e),
            };
            let room = (size - padding) as usize;
            let have = buffered.len() / BYTES_PER_FRAME;
            let n = room.min(have);
            if have == 0 {
                playing = false; // ran dry: buffer up again before carrying on
                stats.1 += 1;
            }
            if n > 0 {
                let out = match render.GetBuffer(n as u32) {
                    Ok(p) => p,
                    Err(e) => break Err(e),
                };
                let out = std::slice::from_raw_parts_mut(out, n * BYTES_PER_FRAME);
                for (o, b) in out.iter_mut().zip(buffered.drain(..n * BYTES_PER_FRAME)) {
                    *o = b;
                }
                if let Err(e) = render.ReleaseBuffer(n as u32, 0) {
                    break Err(e);
                }
            }
            if stats.0.elapsed() >= Duration::from_secs(10) {
                if stats.1 > 0 || stats.2 > 0 {
                    log::info!("mirror audio: ran dry {} times, trimmed {} times in 10 s", stats.1, stats.2);
                }
                stats = (Instant::now(), 0, 0);
            }
        };
        let _ = client.Stop();
        let _ = CloseHandle(ready);
        result
    }
}

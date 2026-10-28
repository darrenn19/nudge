//! `--selftest`: a fake phone on this PC, so the receive/decode/display path can
//! be checked without a phone. Software H.264 (openh264, Baseline) at 720x1280.

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::time::Duration;

use openh264::OpenH264API;
use openh264::encoder::{Encoder, EncoderConfig, FrameType};
use openh264::formats::YUVBuffer;

use crate::now_us;

pub fn start(port: u16) {
    std::thread::spawn(move || {
        if let Err(e) = fake_phone(port) {
            eprintln!("selftest: {e}");
        }
    });
}

/// A moving bar, a gradient and a coloured square: easy to judge scaling and colour.
fn draw(w: usize, h: usize, n: usize) -> YUVBuffer {
    let mut yuv = vec![0u8; w * h * 3 / 2];
    let (luma, chroma) = yuv.split_at_mut(w * h);
    let bar = (n * 8) % w;
    for y in 0..h {
        for x in 0..w {
            let stripe = if (y / 40) % 2 == 0 && (x / 2) % 2 == 0 { 40 } else { 0 };
            let mut v = 16 + (x * 200 / w) as u8 + stripe;
            if x >= bar && x < bar + 24 {
                v = 235;
            }
            luma[y * w + x] = v;
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let (u, v) = chroma.split_at_mut(cw * ch);
    for y in 0..ch {
        for x in 0..cw {
            let red = (100..200).contains(&x) && (100..200).contains(&y);
            u[y * cw + x] = if red { 90 } else { 128 };
            v[y * cw + x] = if red { 240 } else { 128 };
        }
    }
    YUVBuffer::from_vec(yuv, w, h)
}

fn fake_phone(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let sock = UdpSocket::bind("127.0.0.1:0")?;
    sock.connect(("127.0.0.1", port))?;
    sock.set_nonblocking(true)?;
    sock.send(&[3])?; // hello

    let (w, h) = (720, 1280);
    let config = EncoderConfig::new().set_bitrate_bps(8_000_000).max_frame_rate(60.0);
    let mut enc = Encoder::with_api_config(OpenH264API::from_source(), config)?;
    let mut id = 0u32;
    let mut buf = [0u8; 2048];
    // The packets of the last 16 frames, for resends.
    let mut history: VecDeque<(u32, Vec<Vec<u8>>)> = VecDeque::new();
    loop {
        let start = now_us();
        while let Ok(n) = sock.recv(&mut buf) {
            match buf[0] {
                10 if n >= 9 => {
                    let mut p = vec![2u8];
                    p.extend_from_slice(&buf[1..9]);
                    p.extend_from_slice(&now_us().to_le_bytes());
                    let _ = sock.send(&p);
                }
                11 => enc.force_intra_frame(),
                12 if n >= 7 => {
                    let frame = u32::from_le_bytes(buf[1..5].try_into().unwrap());
                    let count = u16::from_le_bytes([buf[5], buf[6]]) as usize;
                    let Some((_, packets)) = history.iter().find(|(f, _)| *f == frame) else { continue };
                    let wanted: Vec<usize> = if count == 0xFFFF {
                        (0..packets.len()).collect()
                    } else {
                        (0..count.min((n - 7) / 2)).map(|i| u16::from_le_bytes([buf[7 + i * 2], buf[8 + i * 2]]) as usize).collect()
                    };
                    for i in wanted {
                        if let Some(p) = packets.get(i) {
                            let mut p = p.clone();
                            p[9] |= 4; // resent
                            let _ = sock.send(&p);
                        }
                    }
                }
                _ => {}
            }
        }

        let capture = now_us();
        let frame = draw(w, h, id as usize);
        let bits = enc.encode(&frame)?;
        let key = matches!(bits.frame_type(), FrameType::IDR | FrameType::I);
        let data = bits.to_vec();
        let encoded = now_us();
        if !data.is_empty() {
            let count = data.len().div_ceil(1200);
            let mut packets = Vec::with_capacity(count);
            for (i, chunk) in data.chunks(1200).enumerate() {
                let mut p = Vec::with_capacity(26 + chunk.len());
                p.push(1);
                p.extend_from_slice(&id.to_le_bytes());
                p.extend_from_slice(&(i as u16).to_le_bytes());
                p.extend_from_slice(&(count as u16).to_le_bytes());
                p.push(key as u8);
                p.extend_from_slice(&capture.to_le_bytes());
                p.extend_from_slice(&encoded.to_le_bytes());
                p.extend_from_slice(chunk);
                let _ = sock.send(&p);
                packets.push(p);
            }
            history.push_back((id, packets));
            if history.len() > 16 {
                history.pop_front();
            }
            id += 1;
        }
        let spent = now_us() - start;
        std::thread::sleep(Duration::from_micros(16_667u64.saturating_sub(spent)));
    }
}

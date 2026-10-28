# Mirror latency spike (throwaway)

Step 1 of screen mirroring (see CLAUDE.md): measure how long it takes for something on the
phone's screen to reach the laptop's screen, before building anything for real.

- **Phone:** the "Mirror spike" launcher icon (debug builds only,
  `app/android/app/src/debug/.../spike/`). MediaProjection → hardware H.264 (High profile) or
  H.265 encoder, constant bitrate, low-latency mode → plain UDP, paced (packets spread out
  instead of one burst per frame). Keeps the last 10 frames' packets to resend on request.
- **PC:** this tool. UDP → hardware decode (Media Foundation, on the GPU) → the D3D11 video
  processor converts colours and scales into the window. Prints a latency breakdown per
  stage and writes a CSV per run. Press **S** in the window to save a full-size PNG. Draws
  only when the display can take a frame without waiting, always the newest one.
- **Loss recovery:** when packets go missing the PC asks for just those (NACK), up to 3
  times, about 1.5 round trips apart. It asks for a keyframe only if the frame still isn't
  complete after that, or after 50 ms. With intra-refresh on, it doesn't even do that: the
  picture heals by itself within 30 frames.
- `--selftest` runs a fake phone on the PC (no phone needed); `--seconds=N` stops by itself;
  `--drop=P` throws away P% of video packets on arrival, to test loss recovery.

There's no encryption. The phone and PC clocks are lined up with
pings, so the total is measured per frame, not estimated. Not included: about one frame for
the phone's own display pipeline and one for the PC monitor (roughly +10–25 ms).

## One-time setup

1. Allow the port through the firewall (in an admin PowerShell):
   `netsh advfirewall firewall add rule name="Nudge mirror spike" dir=in action=allow protocol=UDP localport=47900`
2. Install the debug app: `cd app; flutter run` (or install `app/build/app/outputs/flutter-apk/app-debug.apk`).

## Running a test

1. On the PC: `cargo run --release -- wifi` in this folder. The label (`wifi`, `cable`,
   `hotspot`) names the CSV. It lists the PC's addresses.
2. On the phone, open **Mirror spike**, type the PC address on the network you're testing, and tap
   **Start** → choose **Entire screen**.
3. Let it run for about 30 s on the spike screen (the clock changes every frame). Then
   scroll a busy web page or play a video for 30 s.
4. Close the PC window (or press Esc). Copy the summary table.

Paths to test:

| Path | How | PC address to type |
|---|---|---|
| Wi-Fi (router) | both on the same Wi-Fi | the PC's Wi-Fi address |
| Cable | USB tethering on | the address on the phone's USB network adapter |
| Hotspot | turn on the phone's Mobile Hotspot, join it from the laptop | the address on that network |

Settings on the phone: bitrate (default 30 Mbit/s), longest side (default 2304, the most
Windows' H.264 decoder accepts), pacing cap (default 200 Mbit/s; 0 = off), and H.265 (needs
"HEVC Video Extensions" from the Microsoft Store on the PC). Two switches to compare:
- **Intra-refresh:** no big keyframes; each frame refreshes a slice of the picture.
- **Encoder speed-ups:** Qualcomm's low-latency key (if the encoder lists it), highest clock,
  one frame in flight.

The PC console prints what the phone actually used (lines starting `Phone:`), including
whether the encoder accepted each setting and which vendor keys it offers.

The summary adds a loss-recovery line (NACKs, frames recovered, how long recovery took,
frames given up) and the keyframe count.

## Go / no-go

Targets from CLAUDE.md: about 40–80 ms over Wi-Fi and 30–50 ms over cable, at 30–60 fps.
If p50 totals land near those, the plan stands. If not, the stage breakdown shows where
the time goes (phone encode, network, PC decode, drawing).

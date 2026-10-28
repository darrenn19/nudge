//! Frames over lossy datagrams: splitting, resending what's lost, and
//! putting frames back together in order.
//!
//! No sockets or clocks here: callers pass the time in, so tests can play
//! out loss and delay exactly.
//!
//! - The sender splits each frame into fragments, keeps recent frames for
//!   resends, and drops frames that waited too long to go out (the decoder
//!   then needs a keyframe).
//! - The receiver asks for missing fragments (NACK) as soon as it sees a gap,
//!   up to `NACK_TRIES` times about 1.5 round trips apart; resends skip the
//!   pacing queue. If a frame still isn't complete `GIVE_UP_US` after the gap
//!   was noticed, it skips it and asks for a keyframe, unless the stream
//!   uses intra-refresh, which heals by itself. A complete keyframe makes
//!   every earlier frame unnecessary.

use std::collections::{BTreeMap, VecDeque};

use super::wire::{self, F_INTRA_REFRESH, F_KEY, F_RESENT, Fragment};

/// Data bytes per fragment: a packet stays under ~1200 bytes, which fits
/// every network's MTU, tunnels included.
pub const MAX_FRAGMENT: usize = 1150;

/// Frames kept for resends, and for how long: Wi-Fi can hold everything for
/// a few hundred ms, and requests arriving after that must still find them.
const HISTORY_FRAMES: usize = 64;
const HISTORY_US: u64 = 1_000_000;
/// A frame not sent within this long is dropped instead (pacing or the
/// network can't keep up): sending it late only adds delay.
pub(super) const MAX_QUEUE_US: u64 = 100_000;

/// How many times to ask for the same missing fragments.
const NACK_TRIES: u32 = 5;
/// Shortest wait before asking again.
const MIN_RETRY_US: u64 = 3_000;
/// Give up on a frame this long after noticing it's incomplete.
const GIVE_UP_US: u64 = 50_000;
/// Recovery deadlines (asking again, giving up) run on link time: wall time
/// while packets keep arriving, with each silence counting as at most this
/// much (above the ~17 ms between frames at 60 fps, so a normal stream runs
/// at full speed). Wi-Fi sometimes holds everything for 50-300 ms and then delivers it
/// at once; that isn't loss, waiting it out costs nothing (later frames are
/// held up too), and giving up meanwhile would turn late frames into
/// skipped ones and waste the NACK tries.
const LINK_GAP_US: u64 = 20_000;
/// Give up this long after noticing a gap, in wall time, whatever link time
/// says: the sender may simply have gone quiet (a still screen), leaving a
/// frame that was really lost waiting.
const GIVE_UP_WALL_US: u64 = 300_000;
/// A frame with no new fragment for this long has a gap at the end (its
/// last fragments were lost). Adapts to the longest pause seen between one
/// frame's fragments (the sender's pacing: ~1 ms with fine timers, ~16 ms
/// where they're coarse), within these bounds. A gap before a later frame
/// is noticed sooner, when that frame arrives.
const STALL_MIN_US: u64 = 4_000;
const STALL_MAX_US: u64 = 30_000;
/// Between keyframe requests while waiting for one.
const KEY_REQUEST_EVERY_US: u64 = 150_000;
/// A frame this far ahead of the one expected means a restart: start over.
const RESYNC_FRAMES: u32 = 600;

/// A frame to send.
#[derive(Debug, Clone)]
pub struct OutFrame {
    /// Which stream (e.g. video 0, audio 1); each is ordered on its own.
    pub stream: u8,
    pub data: Vec<u8>,
    /// Decodable on its own.
    pub key: bool,
    /// The encoder heals losses by itself (intra-refresh): the receiver
    /// shouldn't ask for keyframes after a loss.
    pub intra_refresh: bool,
    /// When it was captured, in the sending session's clock (`Session::now_us`).
    pub capture_us: u64,
}

/// A frame received complete, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub stream: u8,
    pub id: u32,
    pub key: bool,
    pub data: Vec<u8>,
    /// Capture time in the sender's clock.
    pub capture_us: u64,
    /// When its last fragment arrived, in this session's clock.
    pub received_us: u64,
    /// Some of it arrived only after being asked for again.
    pub recovered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendStats {
    pub frames: u64,
    pub packets: u64,
    pub bytes: u64,
    pub nacks_received: u64,
    pub resent_packets: u64,
    /// Dropped because they waited longer than `MAX_QUEUE_US` to go out.
    pub dropped_frames: u64,
    pub key_requests_received: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReceiveStats {
    pub frames: u64,
    pub keyframes: u64,
    pub nacks_sent: u64,
    pub fragments_asked: u64,
    pub fragments_recovered: u64,
    pub frames_recovered: u64,
    /// Resent fragments that arrived after they were no longer needed.
    pub resends_unused: u64,
    /// Skipped after recovery failed.
    pub given_up: u64,
    /// Skipped because a later keyframe made them unnecessary.
    pub superseded: u64,
    /// Complete, but unusable while waiting for a keyframe.
    pub discarded: u64,
    pub key_requests: u64,
}

struct Queued {
    msg: Vec<u8>,
    frame: u32,
    at: u64,
}

struct Sent {
    frame: u32,
    at: u64,
    intra_refresh: bool,
    msgs: Vec<Vec<u8>>,
}

/// Byte offset of the flags in a fragment message.
const FLAGS_AT: usize = 10;

pub(super) struct FrameSender {
    stream: u8,
    next_id: u32,
    history: VecDeque<Sent>,
    queue: VecDeque<Queued>,
    last_dropped: Option<u32>,
    keyframe_needed: bool,
    pub(super) stats: SendStats,
}

impl FrameSender {
    pub(super) fn new(stream: u8) -> Self {
        Self {
            stream,
            next_id: 0,
            history: VecDeque::new(),
            queue: VecDeque::new(),
            last_dropped: None,
            keyframe_needed: false,
            stats: SendStats::default(),
        }
    }

    /// Queues a frame; returns its id.
    pub(super) fn push(&mut self, f: &OutFrame, now: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let flags = if f.key { F_KEY } else { 0 } | if f.intra_refresh { F_INTRA_REFRESH } else { 0 };
        let chunks: Vec<&[u8]> = if f.data.is_empty() {
            vec![&[]]
        } else {
            f.data.chunks(MAX_FRAGMENT).collect()
        };
        let count = chunks.len().min(u16::MAX as usize - 1) as u16;
        let msgs: Vec<Vec<u8>> = chunks
            .iter()
            .take(count as usize)
            .enumerate()
            .map(|(i, data)| {
                wire::fragment(&Fragment {
                    stream: self.stream,
                    frame: id,
                    index: i as u16,
                    count,
                    flags,
                    capture_us: f.capture_us,
                    data,
                })
            })
            .collect();
        for m in &msgs {
            self.queue.push_back(Queued {
                msg: m.clone(),
                frame: id,
                at: now,
            });
        }
        self.history.push_back(Sent {
            frame: id,
            at: now,
            intra_refresh: f.intra_refresh,
            msgs,
        });
        while self.history.len() > HISTORY_FRAMES
            || self.history.front().is_some_and(|s| now.saturating_sub(s.at) > HISTORY_US)
        {
            self.history.pop_front();
        }
        self.stats.frames += 1;
        id
    }

    /// The fragments the receiver asked for, to send right away (not paced:
    /// they're few, and late resends are useless).
    pub(super) fn on_nack(&mut self, frame: u32, missing: Option<&[u16]>) -> Vec<Vec<u8>> {
        self.stats.nacks_received += 1;
        let Some(sent) = self.history.iter().find(|s| s.frame == frame) else { return Vec::new() };
        let wanted: Vec<usize> = match missing {
            None => (0..sent.msgs.len()).collect(),
            Some(list) => list.iter().map(|&i| i as usize).filter(|&i| i < sent.msgs.len()).collect(),
        };
        // Ones still waiting for their first send are on their way already.
        let queued: std::collections::HashSet<usize> = self
            .queue
            .iter()
            .filter(|q| q.frame == frame)
            .map(|q| u16::from_be_bytes([q.msg[6], q.msg[7]]) as usize)
            .collect();
        let mut out = Vec::new();
        for i in wanted {
            if queued.contains(&i) {
                continue;
            }
            let mut msg = sent.msgs[i].clone();
            msg[FLAGS_AT] |= F_RESENT;
            out.push(msg);
        }
        self.stats.resent_packets += out.len() as u64;
        self.stats.packets += out.len() as u64;
        self.stats.bytes += out.iter().map(|m| m.len() as u64).sum::<u64>();
        out
    }

    /// The next message to send. Frames that waited too long are dropped.
    pub(super) fn pop(&mut self, now: u64) -> Option<Vec<u8>> {
        loop {
            let q = self.queue.pop_front()?;
            if now.saturating_sub(q.at) > MAX_QUEUE_US {
                if self.last_dropped != Some(q.frame) {
                    self.last_dropped = Some(q.frame);
                    self.stats.dropped_frames += 1;
                    let healing = self.history.iter().find(|s| s.frame == q.frame).is_some_and(|s| s.intra_refresh);
                    self.history.retain(|s| s.frame != q.frame);
                    if !healing {
                        self.keyframe_needed = true;
                    }
                }
                continue;
            }
            self.stats.packets += 1;
            self.stats.bytes += q.msg.len() as u64;
            return Some(q.msg);
        }
    }

    pub(super) fn has_queued(&self) -> bool {
        !self.queue.is_empty()
    }

    /// True once after frames were dropped: the encoder should make a keyframe.
    pub(super) fn take_keyframe_needed(&mut self) -> bool {
        std::mem::take(&mut self.keyframe_needed)
    }
}

struct Partial {
    /// Empty until the first fragment arrives (the count isn't known before).
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
    /// Highest fragment index seen, plus one.
    top: usize,
    flags: u8,
    capture_us: u64,
    /// When its latest fragment arrived (or when it was found missing).
    last_seen: u64,
    /// When something was first noticed missing (link time).
    gap_since: Option<u64>,
    /// The same in wall time.
    gap_wall: Option<u64>,
    /// Per fragment: times asked for, and when last (link time).
    asks: Vec<(u32, u64)>,
    /// The same, for the whole frame while none of it has arrived.
    whole_asks: (u32, u64),
    resent: bool,
}

impl Partial {
    fn new(now: u64) -> Self {
        Self {
            parts: Vec::new(),
            have: 0,
            top: 0,
            flags: 0,
            capture_us: 0,
            last_seen: now,
            gap_since: None,
            gap_wall: None,
            asks: Vec::new(),
            whole_asks: (0, 0),
            resent: false,
        }
    }
}

/// What the receiver wants done after `poll`.
#[derive(Default)]
pub(super) struct Polled {
    pub nacks: Vec<Vec<u8>>,
    pub frames: Vec<Frame>,
    pub key_request: bool,
}

pub(super) struct FrameReceiver {
    stream: u8,
    partials: BTreeMap<u32, Partial>,
    /// Complete frames waiting for an earlier one.
    ready: BTreeMap<u32, Frame>,
    /// The next frame to hand out.
    next: Option<u32>,
    highest: Option<u32>,
    /// The decoder can't continue without a keyframe.
    need_key: bool,
    intra_refresh: bool,
    last_key_request: Option<u64>,
    /// Smoothed time from a NACK to its resend arriving: the round trip
    /// plus however long the sender takes to answer.
    resend_rtt: Option<u64>,
    /// Longest recent pause between two fragments of one frame (decays).
    /// Until one is seen, the cautious maximum applies.
    pause_peak: Option<u64>,
    /// Link time (see `LINK_GAP_US`), and when the last packet arrived.
    link: u64,
    last_arrival: Option<u64>,
    pub(super) stats: ReceiveStats,
}

impl FrameReceiver {
    pub(super) fn new(stream: u8) -> Self {
        Self {
            stream,
            partials: BTreeMap::new(),
            ready: BTreeMap::new(),
            next: None,
            highest: None,
            need_key: true,
            intra_refresh: false,
            last_key_request: None,
            resend_rtt: None,
            pause_peak: None,
            link: 0,
            last_arrival: None,
            stats: ReceiveStats::default(),
        }
    }

    pub(super) fn stream(&self) -> u8 {
        self.stream
    }

    /// The decoder failed: hand out nothing more until a keyframe.
    pub(super) fn need_keyframe(&mut self) {
        self.start_needing_key();
    }

    /// From now on only a keyframe will do: ask for one right away.
    fn start_needing_key(&mut self) {
        if !self.need_key {
            self.need_key = true;
            self.last_key_request = None;
        }
    }

    /// Something is in flight or awaited: `poll` should run again soon.
    pub(super) fn busy(&self) -> bool {
        !self.partials.is_empty() || !self.ready.is_empty() || self.need_key && self.next.is_some()
    }

    /// Link time at `now` (see `LINK_GAP_US`).
    fn link_at(&self, now: u64) -> u64 {
        self.link + self.last_arrival.map_or(0, |t| now.saturating_sub(t).min(LINK_GAP_US))
    }

    /// Any packet from the other side arrived (fragments, pings): the link
    /// is delivering, so link time moves on.
    pub(super) fn on_packet(&mut self, now: u64) {
        self.link = self.link_at(now);
        self.last_arrival = Some(now);
    }

    pub(super) fn on_fragment(&mut self, f: &Fragment<'_>, now: u64) {
        self.on_packet(now);
        let (id, index, count) = (f.frame, f.index as usize, f.count as usize);
        let resent = f.flags & F_RESENT != 0;
        if self.next.is_some_and(|n| id < n) {
            self.stats.resends_unused += resent as u64;
            return; // already handed out or skipped
        }
        if self.next.is_some_and(|n| id - n > RESYNC_FRAMES) {
            self.partials.clear();
            self.ready.clear();
            self.next = None;
            self.highest = None;
            self.need_key = true;
        }
        if self.next.is_none() {
            // The first frame is usually a keyframe: give it time to arrive
            // before asking for one.
            self.last_key_request = Some(now);
        }
        let next = *self.next.get_or_insert(id);
        // Frames not heard from at all, between the newest one and this one.
        for gap in self.highest.map_or(next, |h| h + 1)..id {
            self.partials.entry(gap).or_insert_with(|| Partial::new(now));
        }
        if self.highest.is_none_or(|h| id > h) {
            self.highest = Some(id);
        }
        if self.ready.contains_key(&id) {
            self.stats.resends_unused += resent as u64;
            return;
        }

        let p = self.partials.entry(id).or_insert_with(|| Partial::new(now));
        if p.parts.is_empty() {
            p.parts = vec![None; count];
            p.asks = vec![(0, 0); count];
            p.flags = f.flags & !F_RESENT;
            p.capture_us = f.capture_us;
        }
        if p.parts.len() != count || p.parts[index].is_some() {
            self.stats.resends_unused += resent as u64;
            return;
        }
        if p.have > 0 && !resent {
            let pause = now.saturating_sub(p.last_seen);
            self.pause_peak = Some(self.pause_peak.map_or(pause, |peak| pause.max(peak)));
        }
        p.parts[index] = Some(f.data.to_vec());
        p.have += 1;
        p.last_seen = now;
        p.top = p.top.max(index + 1);
        if resent {
            // Only after a single NACK is it clear which one this answers.
            let (asked, at) = p.asks[index];
            if asked == 1 || asked == 0 && p.whole_asks.0 == 1 {
                let at = if asked == 1 { at } else { p.whole_asks.1 };
                let sample = self.link.saturating_sub(at);
                self.resend_rtt = Some(self.resend_rtt.map_or(sample, |r| (r * 7 + sample) / 8));
            }
            p.resent = true;
            self.stats.fragments_recovered += 1;
        }
        if p.have < count {
            return;
        }

        let p = self.partials.remove(&id).unwrap();
        if p.resent {
            self.stats.frames_recovered += 1;
        }
        // Forget old pauses slowly (about a second at 60 fps), so a sender
        // that once paused long doesn't slow detection for good.
        self.pause_peak = self.pause_peak.map(|peak| peak * 63 / 64);
        self.intra_refresh = p.flags & F_INTRA_REFRESH != 0;
        let frame = Frame {
            stream: self.stream,
            id,
            key: p.flags & F_KEY != 0,
            data: p.parts.into_iter().flatten().flatten().collect(),
            capture_us: p.capture_us,
            received_us: now,
            recovered: p.resent,
        };
        self.ready.insert(id, frame);
    }

    /// How long a frame may go without a new fragment before its end counts
    /// as lost: twice the longest recent pause between fragments, plus room
    /// for jitter.
    fn stall_us(&self) -> u64 {
        self.pause_peak
            .map_or(STALL_MAX_US, |peak| (peak * 2 + 2_000).clamp(STALL_MIN_US, STALL_MAX_US))
    }

    /// Asks for what's missing, skips what can't be recovered, and hands out
    /// frames in order. `rtt_us`: the current round-trip estimate.
    pub(super) fn poll(&mut self, now: u64, rtt_us: u64) -> Polled {
        let retry = (rtt_us.max(self.resend_rtt.unwrap_or(0)) * 3 / 2).max(MIN_RETRY_US);
        let stall = self.stall_us();
        // Wall time spots a lost frame end; link time paces recovery.
        let wall = now;
        let now = self.link_at(wall);
        let mut out = Polled::default();

        for (&id, p) in self.partials.iter_mut() {
            // Missing: a later frame has started, a later fragment of this
            // one has arrived, or nothing more has come for a while.
            let behind = self.highest.is_some_and(|h| id < h) || wall.saturating_sub(p.last_seen) > stall;
            // Ask for a fragment again only after `retry`, at most NACK_TRIES times.
            let due = |(tries, last): (u32, u64)| tries < NACK_TRIES && (tries == 0 || now.saturating_sub(last) >= retry);
            if p.parts.is_empty() {
                // None of it yet: ask for the whole frame.
                p.gap_since.get_or_insert(now);
                p.gap_wall.get_or_insert(wall);
                if due(p.whole_asks) {
                    p.whole_asks = (p.whole_asks.0 + 1, now);
                    self.stats.nacks_sent += 1;
                    self.stats.fragments_asked += 1;
                    out.nacks.push(wire::nack(self.stream, id, None));
                }
                continue;
            }
            let upto = if behind { p.parts.len() } else { p.top };
            let missing: Vec<usize> = (0..upto).filter(|&i| p.parts[i].is_none()).collect();
            if missing.is_empty() {
                continue;
            }
            p.gap_since.get_or_insert(now);
            p.gap_wall.get_or_insert(wall);
            let ask: Vec<u16> = missing.into_iter().filter(|&i| due(p.asks[i])).map(|i| i as u16).collect();
            if !ask.is_empty() {
                for &i in &ask {
                    let a = &mut p.asks[i as usize];
                    *a = (a.0 + 1, now);
                }
                self.stats.nacks_sent += 1;
                self.stats.fragments_asked += ask.len() as u64;
                out.nacks.push(wire::nack(self.stream, id, Some(&ask)));
            }
        }

        while let Some(n) = self.next {
            if let Some(f) = self.ready.remove(&n) {
                self.next = Some(n.wrapping_add(1));
                if self.need_key {
                    if !f.key {
                        self.stats.discarded += 1;
                        continue;
                    }
                    self.need_key = false;
                }
                self.stats.frames += 1;
                self.stats.keyframes += f.key as u64;
                out.frames.push(f);
                continue;
            }
            // A complete keyframe further on: nothing before it is needed.
            if let Some(k) = self.ready.iter().find(|(_, f)| f.key).map(|(&id, _)| id) {
                self.stats.superseded += u64::from(k - n) - self.ready.range(n..k).count() as u64;
                self.stats.discarded += self.ready.range(n..k).count() as u64;
                self.partials.retain(|&id, _| id > k);
                self.ready.retain(|&id, _| id >= k);
                self.next = Some(k);
                continue;
            }
            let Some(p) = self.partials.get(&n) else { break };
            // By link time only: resends may be slow to come (a busy sender,
            // coarse timers, a Wi-Fi pause), and a keyframe costs more than
            // waiting.
            let hopeless = p.gap_since.is_some_and(|g| now.saturating_sub(g) >= GIVE_UP_US)
                || p.gap_wall.is_some_and(|g| wall.saturating_sub(g) >= GIVE_UP_WALL_US);
            if !hopeless {
                break;
            }
            self.partials.remove(&n);
            self.next = Some(n.wrapping_add(1));
            self.stats.given_up += 1;
            // Later frames reference the lost one, unless intra-refresh heals it.
            if !self.intra_refresh {
                self.start_needing_key();
            }
        }

        // A keyframe already on its way will do.
        let key_coming = self.partials.values().any(|p| p.flags & F_KEY != 0);
        if self.need_key
            && !key_coming
            && self.next.is_some()
            && self.last_key_request.is_none_or(|t| wall.saturating_sub(t) >= KEY_REQUEST_EVERY_US)
        {
            self.last_key_request = Some(wall);
            self.stats.key_requests += 1;
            out.key_request = true;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime::wire::{Message, parse};

    fn frame(len: usize, key: bool, seed: u8) -> OutFrame {
        OutFrame {
            stream: 0,
            data: (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect(),
            key,
            intra_refresh: false,
            capture_us: seed as u64,
        }
    }

    /// One direction of a simulated link with a fixed one-way delay.
    struct Sim {
        tx: FrameSender,
        rx: FrameReceiver,
        now: u64,
        delay: u64,
        /// (arrival time, message) toward the receiver / the sender.
        forward: Vec<(u64, Vec<u8>)>,
        back: Vec<(u64, Vec<u8>)>,
        got: Vec<Frame>,
        key_requests: u32,
        sent: u64,
    }

    impl Sim {
        fn new(delay: u64) -> Self {
            Self {
                tx: FrameSender::new(0),
                rx: FrameReceiver::new(0),
                now: 0,
                delay,
                forward: Vec::new(),
                back: Vec::new(),
                got: Vec::new(),
                key_requests: 0,
                sent: 0,
            }
        }

        /// Advance 1 ms. `lose(n, frame)` decides whether the n-th packet
        /// sent, part of `frame`, is lost.
        fn step(&mut self, lose: &mut impl FnMut(u64, u32) -> bool) {
            self.now += 1_000;
            while let Some(m) = self.tx.pop(self.now) {
                self.sent += 1;
                let frame = match parse(&m) {
                    Some(Message::Fragment(f)) => f.frame,
                    _ => u32::MAX,
                };
                if !lose(self.sent, frame) {
                    self.forward.push((self.now + self.delay, m));
                }
            }
            let now = self.now;
            let (due, later): (Vec<_>, Vec<_>) = self.forward.drain(..).partition(|(t, _)| *t <= now);
            self.forward = later;
            for (_, m) in due {
                if let Some(Message::Fragment(f)) = parse(&m) {
                    self.rx.on_fragment(&f, now);
                }
            }
            let polled = self.rx.poll(now, 2 * self.delay);
            self.got.extend(polled.frames);
            for n in polled.nacks {
                self.back.push((now + self.delay, n));
            }
            if polled.key_request {
                self.key_requests += 1;
            }
            let (due, later): (Vec<_>, Vec<_>) = self.back.drain(..).partition(|(t, _)| *t <= now);
            self.back = later;
            for (_, m) in due {
                if let Some(Message::Nack { frame, missing, .. }) = parse(&m) {
                    for r in self.tx.on_nack(frame, missing.as_deref()) {
                        self.sent += 1;
                        if !lose(self.sent, frame) {
                            self.forward.push((now + self.delay, r));
                        }
                    }
                }
            }
        }

        fn run(&mut self, ms: u32, lose: &mut impl FnMut(u64, u32) -> bool) {
            for _ in 0..ms {
                self.step(lose);
            }
        }
    }

    #[test]
    fn delivers_in_order_without_loss() {
        let mut sim = Sim::new(2_000);
        let frames: Vec<OutFrame> = (0..20).map(|i| frame(5_000 + i * 100, i == 0, i as u8)).collect();
        for f in &frames {
            sim.tx.push(f, sim.now);
            sim.run(16, &mut |_, _| false);
        }
        sim.run(20, &mut |_, _| false);
        assert_eq!(sim.got.len(), 20);
        for (got, want) in sim.got.iter().zip(&frames) {
            assert_eq!(got.data, want.data);
            assert!(!got.recovered);
        }
        assert_eq!(sim.rx.stats.nacks_sent, 0);
        assert_eq!(sim.key_requests, 0, "the first frame is a keyframe");
    }

    #[test]
    fn recovers_lost_fragments_without_keyframes() {
        let mut sim = Sim::new(3_000);
        // Every 7th packet lost, the first time it's sent (resends get through).
        let mut lost = std::collections::HashSet::new();
        let mut lose = |n: u64, _| n % 7 == 0 && lost.insert(n);
        let frames: Vec<OutFrame> = (0..60).map(|i| frame(12_000, i == 0, i as u8)).collect();
        for f in &frames {
            sim.tx.push(f, sim.now);
            sim.run(16, &mut lose);
        }
        sim.run(60, &mut lose);
        assert_eq!(sim.got.len(), 60, "every frame arrives");
        for (got, want) in sim.got.iter().zip(&frames) {
            assert_eq!(got.data, want.data);
        }
        assert!(sim.rx.stats.frames_recovered > 30);
        assert_eq!(sim.rx.stats.given_up, 0);
        assert!(sim.key_requests <= 1, "at most one, before the first keyframe arrived");
    }

    #[test]
    fn recovers_a_frame_lost_entirely() {
        let mut sim = Sim::new(2_000);
        sim.tx.push(&frame(3_000, true, 0), sim.now);
        sim.run(16, &mut |_, _| false);
        // The whole second frame (3 packets: numbers 4-6) is lost.
        sim.tx.push(&frame(3_000, false, 1), sim.now);
        sim.run(16, &mut |n, _| (4..=6).contains(&n));
        sim.tx.push(&frame(3_000, false, 2), sim.now);
        sim.run(30, &mut |_, _| false);
        assert_eq!(sim.got.iter().map(|f| f.id).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(sim.got[1].recovered);
    }

    /// The last fragment of a frame is lost and no frame follows (a still
    /// screen): noticed after a few ms, not only when the next frame comes.
    #[test]
    fn recovers_a_lost_last_fragment_quickly() {
        let mut sim = Sim::new(1_000);
        sim.tx.push(&frame(3_000, true, 0), sim.now);
        sim.run(16, &mut |_, _| false);
        // Frame 1 is packets 4-6; the last one is lost once.
        sim.tx.push(&frame(3_000, false, 1), sim.now);
        let start = sim.now;
        let mut lost = false;
        let mut lose = |n: u64, _| n == 6 && !std::mem::replace(&mut lost, true);
        while sim.got.len() < 2 && sim.now - start < 40_000 {
            sim.step(&mut lose);
        }
        assert_eq!(sim.got.len(), 2);
        assert!(sim.got[1].recovered);
        let took = sim.got[1].received_us - start;
        assert!(took <= 10_000, "took {took} µs");
    }

    /// Wi-Fi holds everything for 200 ms in the middle of a frame, then
    /// delivers it with one fragment lost: the frame is recovered, not given
    /// up, and the pause doesn't use up the NACK tries.
    #[test]
    fn a_link_pause_doesnt_cost_frames() {
        let fragment = |id: u32, index: u16, key: bool| {
            let data = [index as u8; 100];
            wire::fragment(&Fragment {
                stream: 0,
                frame: id,
                index,
                count: 3,
                flags: if key { F_KEY } else { 0 },
                capture_us: 0,
                data: &data,
            })
        };
        let mut rx = FrameReceiver::new(0);
        let mut got = Vec::new();
        let mut nacks = 0;
        let feed = |rx: &mut FrameReceiver, m: Vec<u8>, now: u64| {
            if let Some(Message::Fragment(f)) = parse(&m) {
                rx.on_fragment(&f, now);
            }
        };
        for i in 0..3 {
            feed(&mut rx, fragment(0, i, true), 1_000);
        }
        feed(&mut rx, fragment(1, 0, false), 17_000);
        // Nothing for 200 ms.
        for t in (18_000..218_000).step_by(1_000) {
            let p = rx.poll(t, 5_000);
            nacks += p.nacks.len();
            got.extend(p.frames);
        }
        // The pause counts as at most LINK_GAP_US: a few asks, never all of them.
        assert!(nacks < NACK_TRIES as usize, "{nacks} asks during the pause");
        // The rest arrives, minus fragment 1, which then comes as a resend.
        feed(&mut rx, fragment(1, 2, false), 218_000);
        for t in (218_000..230_000).step_by(1_000) {
            got.extend(rx.poll(t, 5_000).frames);
        }
        let mut resend = fragment(1, 1, false);
        resend[FLAGS_AT] |= F_RESENT;
        feed(&mut rx, resend, 230_000);
        got.extend(rx.poll(231_000, 5_000).frames);
        assert_eq!(got.iter().map(|f| f.id).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(rx.stats.given_up, 0);
    }

    #[test]
    fn gives_up_and_asks_for_a_keyframe() {
        let mut sim = Sim::new(2_000);
        sim.tx.push(&frame(3_000, true, 0), sim.now);
        sim.run(16, &mut |_, _| false);
        let requests = sim.key_requests;
        // Frame 1 never gets through, however often it's resent.
        let mut lose_1 = |_, frame| frame == 1;
        sim.tx.push(&frame(3_000, false, 1), sim.now);
        sim.run(16, &mut lose_1);
        // The phone carries on at 60 fps meanwhile.
        for i in 2..9 {
            sim.tx.push(&frame(3_000, false, i), sim.now);
            sim.run(16, &mut lose_1);
        }
        assert_eq!(sim.rx.stats.given_up, 1);
        assert_eq!(sim.got.len(), 1, "later frames reference the lost one");
        assert!(sim.key_requests > requests);
        // A keyframe gets things going again.
        sim.tx.push(&frame(3_000, true, 9), sim.now);
        sim.run(20, &mut |_, _| false);
        assert_eq!(sim.got.last().unwrap().id, 9);
    }

    #[test]
    fn intra_refresh_carries_on_after_a_loss() {
        let mut sim = Sim::new(2_000);
        let mut f = frame(3_000, true, 0);
        f.intra_refresh = true;
        sim.tx.push(&f, sim.now);
        sim.run(16, &mut |_, _| false);
        let requests = sim.key_requests;
        f = frame(3_000, false, 1);
        f.intra_refresh = true;
        let mut lose_1 = |_, frame| frame == 1;
        sim.tx.push(&f, sim.now);
        sim.run(16, &mut lose_1);
        for _ in 2..9 {
            f.data[0] ^= 1;
            sim.tx.push(&f, sim.now);
            sim.run(16, &mut lose_1);
        }
        sim.run(20, &mut lose_1);
        assert_eq!(sim.got.iter().map(|f| f.id).collect::<Vec<_>>(), [0, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(sim.key_requests, requests, "no keyframe asked for");
    }

    #[test]
    fn a_keyframe_supersedes_earlier_gaps() {
        let mut rx = FrameReceiver::new(0);
        let tx_frames: Vec<Vec<u8>> = {
            let mut tx = FrameSender::new(0);
            tx.push(&frame(100, true, 0), 0);
            tx.push(&frame(3_000, false, 1), 0);
            tx.push(&frame(100, true, 2), 0);
            std::iter::from_fn(|| tx.pop(0)).collect()
        };
        // Frame 0, half of frame 1, all of frame 2 (a keyframe).
        for (i, m) in tx_frames.iter().enumerate() {
            if i == 2 {
                continue;
            }
            if let Some(Message::Fragment(f)) = parse(m) {
                rx.on_fragment(&f, 1_000);
            }
        }
        let polled = rx.poll(1_000, 2_000);
        assert_eq!(polled.frames.iter().map(|f| f.id).collect::<Vec<_>>(), [0, 2]);
        assert_eq!(rx.stats.superseded, 1);
    }

    #[test]
    fn sender_drops_frames_that_waited_too_long() {
        let mut tx = FrameSender::new(0);
        tx.push(&frame(100, true, 0), 0);
        tx.push(&frame(100, false, 1), 0);
        assert!(tx.pop(MAX_QUEUE_US + 1).is_none());
        assert_eq!(tx.stats.dropped_frames, 2);
        assert!(tx.take_keyframe_needed());
        assert!(!tx.take_keyframe_needed());
        // NACKs for dropped frames get nothing.
        assert!(tx.on_nack(0, None).is_empty());
    }

    #[test]
    fn doesnt_resend_what_hasnt_been_sent_yet() {
        let mut tx = FrameSender::new(0);
        tx.push(&frame(5_000, true, 0), 0); // 5 fragments
        tx.pop(0);
        tx.pop(0);
        assert_eq!(tx.on_nack(0, None).len(), 2, "only the two already sent");
        assert_eq!(std::iter::from_fn(|| tx.pop(0)).count(), 3);
        assert_eq!(tx.stats.resent_packets, 2);
    }

    #[test]
    fn a_slowly_arriving_frame_isnt_nacked_early() {
        let mut tx = FrameSender::new(0);
        let mut rx = FrameReceiver::new(0);
        tx.push(&frame(100 * MAX_FRAGMENT, true, 0), 0);
        // One fragment every millisecond: 100 ms for the frame, no gaps.
        let (mut now, mut got) = (0, 0);
        while let Some(m) = tx.pop(now) {
            now += 1_000;
            if let Some(Message::Fragment(f)) = parse(&m) {
                rx.on_fragment(&f, now);
            }
            let polled = rx.poll(now, 2_000);
            assert!(polled.nacks.is_empty(), "NACK at {now} µs");
            got += polled.frames.len();
        }
        assert_eq!(got, 1);
    }

    #[test]
    fn resends_are_marked() {
        let mut tx = FrameSender::new(0);
        tx.push(&frame(2_000, true, 0), 0);
        while tx.pop(0).is_some() {}
        let resent = tx.on_nack(0, Some(&[1]));
        let Some(Message::Fragment(f)) = parse(&resent[0]) else { panic!() };
        assert_eq!((f.frame, f.index), (0, 1));
        assert_ne!(f.flags & F_RESENT, 0);
        assert_eq!(tx.stats.resent_packets, 1);
    }
}

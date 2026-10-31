//! Video bitrate that follows the network, for the side that encodes.
//!
//! Wi-Fi often can't carry the full rate (30 Mbit/s), above all with busy
//! pictures (games) that use all of it. Then packets queue up in the
//! network: delay climbs to hundreds of ms, frames are lost and given up
//! on, and the keyframes that follow make it worse. So the encoder's rate
//! follows the network, the way game streaming does:
//!
//! - **Down fast** when the round trip rises above the session's best
//!   (packets are queuing) or the receiver keeps asking for lost fragments:
//!   to 70%, at most every 300 ms.
//! - **Up slowly** after a second of a clear network: +8% of the top rate
//!   per second, back to the top.
//!
//! Over a cable nothing queues, so it stays at the top.

/// Queuing (round trip above the best) that counts as congestion.
const CONGESTED_US: u64 = 25_000;
/// Queuing low enough to try a higher rate.
const CLEAR_US: u64 = 8_000;
/// Lost-fragment requests per second that count as congestion.
const NACKS_PER_SEC: f64 = 15.0;
const DOWN: f64 = 0.7;
const DOWN_EVERY_US: u64 = 300_000;
/// A clear network for this long before going up, and between steps up.
const UP_AFTER_US: u64 = 1_000_000;
/// Each step up, as a share of the top rate.
const UP_STEP: f64 = 0.08;

/// What the controller decided, for logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateChange {
    pub kbps: u32,
    /// Round trip above the best seen, µs, when it decided.
    pub queuing_us: u64,
}

#[derive(Debug, Clone)]
pub struct RateControl {
    max_kbps: u32,
    min_kbps: u32,
    kbps: u32,
    last_down: Option<u64>,
    last_up: Option<u64>,
    clear_since: Option<u64>,
    /// Lost-fragment requests counted so far, and when.
    nacks: Option<(u64, u64)>,
}

impl RateControl {
    /// Starts at the top rate.
    pub fn new(max_kbps: u32, min_kbps: u32) -> Self {
        Self {
            max_kbps,
            min_kbps: min_kbps.min(max_kbps),
            kbps: max_kbps,
            last_down: None,
            last_up: None,
            clear_since: None,
            nacks: None,
        }
    }

    pub fn kbps(&self) -> u32 {
        self.kbps
    }

    /// Call every ~100-250 ms with the session's numbers: the latest round
    /// trip and the best of the last seconds (`Stats::rtt_latest_us`,
    /// `Stats::clock_rtt_us`), and lost-fragment requests received so far
    /// (`Stats::sent.nacks_received`). Returns the new rate when it changes.
    pub fn update(
        &mut self,
        now_us: u64,
        rtt_us: Option<u64>,
        best_rtt_us: Option<u64>,
        nacks_total: u64,
    ) -> Option<RateChange> {
        let queuing = match (rtt_us, best_rtt_us) {
            (Some(r), Some(b)) => r.saturating_sub(b),
            _ => 0,
        };
        let nack_rate = match self.nacks.replace((now_us, nacks_total)) {
            Some((at, n)) if now_us > at => {
                nacks_total.saturating_sub(n) as f64 / ((now_us - at) as f64 / 1e6)
            }
            _ => 0.0,
        };
        let congested = queuing > CONGESTED_US || nack_rate > NACKS_PER_SEC;
        if congested {
            self.clear_since = None;
            if self.last_down.is_some_and(|t| now_us.saturating_sub(t) < DOWN_EVERY_US) {
                return None;
            }
            let to = ((self.kbps as f64 * DOWN) as u32).max(self.min_kbps);
            self.last_down = Some(now_us);
            return self.set(to, queuing);
        }
        if queuing > CLEAR_US || nack_rate > 0.0 {
            self.clear_since = None;
            return None;
        }
        let since = *self.clear_since.get_or_insert(now_us);
        let waited = now_us.saturating_sub(since) >= UP_AFTER_US
            && self.last_up.is_none_or(|t| now_us.saturating_sub(t) >= UP_AFTER_US)
            && self.last_down.is_none_or(|t| now_us.saturating_sub(t) >= UP_AFTER_US);
        if !waited || self.kbps >= self.max_kbps {
            return None;
        }
        self.last_up = Some(now_us);
        let step = (self.max_kbps as f64 * UP_STEP) as u32;
        self.set((self.kbps + step).min(self.max_kbps), queuing)
    }

    fn set(&mut self, kbps: u32, queuing_us: u64) -> Option<RateChange> {
        if kbps == self.kbps {
            return None;
        }
        self.kbps = kbps;
        Some(RateChange { kbps, queuing_us })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000;

    #[test]
    fn stays_at_the_top_on_a_clear_network() {
        let mut r = RateControl::new(30_000, 4_000);
        for i in 0..100 {
            assert_eq!(r.update(i * 200 * MS, Some(3 * MS), Some(2 * MS), 0), None);
        }
        assert_eq!(r.kbps(), 30_000);
    }

    #[test]
    fn drops_fast_when_packets_queue_and_never_below_the_floor() {
        let mut r = RateControl::new(30_000, 4_000);
        let mut t = 0;
        let mut changes = 0;
        for _ in 0..40 {
            t += 100 * MS;
            if r.update(t, Some(120 * MS), Some(5 * MS), 0).is_some() {
                changes += 1;
            }
        }
        // Every 300 ms, not every call.
        assert!(changes <= 14, "{changes}");
        assert_eq!(r.kbps(), 4_000);
        // Down to 70% within the first call.
        let mut r = RateControl::new(30_000, 4_000);
        assert_eq!(r.update(MS, Some(60 * MS), Some(5 * MS), 0).map(|c| c.kbps), Some(21_000));
    }

    #[test]
    fn lost_fragments_count_as_congestion() {
        let mut r = RateControl::new(30_000, 4_000);
        r.update(0, Some(5 * MS), Some(5 * MS), 0);
        // 10 requests in 200 ms: 50 a second.
        assert!(r.update(200 * MS, Some(5 * MS), Some(5 * MS), 10).is_some());
        assert!(r.kbps() < 30_000);
    }

    #[test]
    fn climbs_back_slowly_once_clear() {
        let mut r = RateControl::new(30_000, 4_000);
        r.update(0, Some(100 * MS), Some(5 * MS), 0);
        assert_eq!(r.kbps(), 21_000);
        let mut t = 0;
        // Clear from now on: no step up within the first second.
        for _ in 0..4 {
            t += 200 * MS;
            assert_eq!(r.update(t, Some(6 * MS), Some(5 * MS), 0), None);
        }
        let mut seen = vec![];
        for _ in 0..100 {
            t += 200 * MS;
            if let Some(c) = r.update(t, Some(6 * MS), Some(5 * MS), 0) {
                seen.push((t, c.kbps));
            }
        }
        assert_eq!(r.kbps(), 30_000);
        // At most one step a second.
        assert!(seen.windows(2).all(|w| w[1].0 - w[0].0 >= UP_AFTER_US));
        assert_eq!(seen[0].1, 23_400);
    }
}

//! The playback buffer between the network and the sound card.
//!
//! Packets go in at the sender's pace, the sound card takes audio out at its
//! own pace. The buffer keeps a small cushion (the target delay) so network
//! jitter does not cause gaps, converts the sample rate when the two sides
//! differ, and nudges the conversion ratio by a fraction of a percent so the
//! cushion stays the same size even though the two clocks drift apart.

use std::collections::VecDeque;

const MAX_CORRECTION: f64 = 0.003;
const CORRECTION_PER_MS: f64 = 0.000_15;
const FILL_TIME_CONSTANT: f64 = 1.0;
const MAX_GAP_PACKETS: u32 = 64;
const AUTO_STEP_MS: f64 = 10.0;
const AUTO_MAX_MS: f64 = 250.0;
const AUTO_WINDOW_S: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Buffering,
    Playing,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub lost_packets: u64,
    pub late_packets: u64,
    pub underruns: u64,
    pub skips: u64,
    pub recent_underruns: usize,
}

pub struct Playout {
    queue: VecDeque<[f32; 2]>,
    pos: f64,
    in_rate: u32,
    out_rate: u32,
    target_ms: f64,
    auto: bool,
    auto_floor_ms: f64,
    phase: Phase,
    fill_ema: f64,
    expected: Option<u32>,
    last_nbs: usize,
    clock: u64,
    underrun_clock: Vec<u64>,
    pub stats: Stats,
    pub gain: f32,
    pub muted: bool,
    peak: f32,
}

fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

impl Playout {
    pub fn new(target_ms: f64, auto: bool) -> Self {
        Self {
            queue: VecDeque::with_capacity(48000),
            pos: 1.0,
            in_rate: 48000,
            out_rate: 48000,
            target_ms,
            auto,
            auto_floor_ms: target_ms,
            phase: Phase::Idle,
            fill_ema: 0.0,
            expected: None,
            last_nbs: 0,
            clock: 0,
            underrun_clock: Vec::new(),
            stats: Stats::default(),
            gain: 1.0,
            muted: false,
            peak: 0.0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn target_ms(&self) -> f64 {
        self.target_ms
    }

    /// Choose the delay. With `auto`, the buffer may grow above it when the
    /// network is unsteady.
    pub fn set_target(&mut self, target_ms: f64, auto: bool) {
        if !target_ms.is_finite() {
            return;
        }
        let target_ms = target_ms.clamp(5.0, AUTO_MAX_MS);
        if (target_ms - self.auto_floor_ms).abs() > f64::EPSILON || auto != self.auto {
            self.target_ms = target_ms;
            self.auto_floor_ms = target_ms;
            self.auto = auto;
        }
    }

    pub fn set_out_rate(&mut self, rate: u32) {
        if rate != self.out_rate && rate > 0 {
            self.out_rate = rate;
            self.restart();
        }
    }

    fn target_frames(&self) -> f64 {
        self.target_ms * self.in_rate as f64 / 1000.0
    }

    /// Frames ready to play after the read position.
    fn available(&self) -> f64 {
        (self.queue.len() as f64 - self.pos - 2.0).max(0.0)
    }

    pub fn buffered_ms(&self) -> f64 {
        self.available() * 1000.0 / self.in_rate as f64
    }

    /// Forget everything and wait for new audio.
    pub fn restart(&mut self) {
        self.queue.clear();
        self.pos = 1.0;
        self.phase = Phase::Idle;
        self.expected = None;
        self.fill_ema = 0.0;
    }

    /// Start of a new stream: also forget the delay the auto mode added.
    pub fn new_stream(&mut self) {
        self.restart();
        self.target_ms = self.auto_floor_ms;
        self.underrun_clock.clear();
    }

    /// Add one packet of audio. `seq` is the VBAN frame counter.
    pub fn push(&mut self, frames: &[[f32; 2]], seq: u32, rate: u32) {
        if frames.is_empty() {
            return;
        }
        if rate != self.in_rate {
            self.in_rate = rate;
            self.restart();
        }
        if let Some(expected) = self.expected {
            let ahead = seq.wrapping_sub(expected);
            if ahead == 0 {
                // in order
            } else if ahead < MAX_GAP_PACKETS {
                self.stats.lost_packets += ahead as u64;
                if self.phase == Phase::Playing {
                    self.conceal(ahead as usize * self.last_nbs.max(frames.len()));
                }
            } else if ahead > u32::MAX - 1024 {
                self.stats.late_packets += 1;
                return;
            } else {
                // A big jump: the sender restarted. Start over cleanly.
                self.restart();
            }
        }
        self.expected = Some(seq.wrapping_add(1));
        self.last_nbs = frames.len();
        if self.queue.is_empty() {
            // One frame of history in front of the read position.
            self.queue.push_back(frames[0]);
            self.pos = 1.0;
        }
        self.queue.extend(frames.iter().copied());
        if self.phase == Phase::Idle {
            self.phase = Phase::Buffering;
        }
        // Trim in every phase: if the sound card stops pulling (device lost,
        // output error) or a sender floods us, memory must stay bounded.
        let limit = self.target_frames() + self.target_frames().max(0.06 * self.in_rate as f64);
        if self.available() > limit {
            let excess = (self.available() - self.target_frames()) as usize;
            self.drop_front(excess);
            self.stats.skips += 1;
            self.fill_ema = self.available();
        }
    }

    fn drop_front(&mut self, frames: usize) {
        let n = frames.min(self.queue.len().saturating_sub(4));
        self.queue.drain(..n);
    }

    /// Fill a hole left by lost packets with the last sound fading out, so
    /// the timing stays right without a click.
    fn conceal(&mut self, frames: usize) {
        let last = self.queue.back().copied().unwrap_or([0.0, 0.0]);
        let frames = frames.min(self.in_rate as usize / 10);
        for k in 0..frames {
            let fade = (-(k as f32) / 96.0).exp();
            self.queue.push_back([last[0] * fade, last[1] * fade]);
        }
    }

    /// Mark the stream as gone quiet (sender stopped).
    pub fn sender_gone(&mut self) {
        self.restart();
    }

    /// Take the peak level since the last call, before volume.
    pub fn take_peak(&mut self) -> f32 {
        std::mem::take(&mut self.peak)
    }

    /// Fill an interleaved output buffer with `channels` channels.
    pub fn pull(&mut self, out: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        let frames = out.len() / channels;
        if frames == 0 {
            return;
        }
        self.clock += frames as u64;
        let target = self.target_frames();
        if self.phase == Phase::Buffering && self.available() >= target {
            self.phase = Phase::Playing;
            self.fill_ema = self.available();
        }
        if self.phase != Phase::Playing {
            out.iter_mut().for_each(|s| *s = 0.0);
            return;
        }

        let block_s = frames as f64 / self.out_rate as f64;
        let alpha = (block_s / FILL_TIME_CONSTANT).min(1.0);
        self.fill_ema += (self.available() - self.fill_ema) * alpha;
        let error_ms = (self.fill_ema - target) * 1000.0 / self.in_rate as f64;
        let correction = (error_ms * CORRECTION_PER_MS).clamp(-MAX_CORRECTION, MAX_CORRECTION);
        let step = self.in_rate as f64 / self.out_rate as f64 * (1.0 + correction);

        let gain = if self.muted { 0.0 } else { self.gain };
        let mut peak = self.peak;
        for i in 0..frames {
            let base = self.pos.floor();
            let idx = base as usize;
            if idx + 2 >= self.queue.len() {
                // Ran dry: play silence and build the cushion up again.
                self.phase = Phase::Buffering;
                self.stats.underruns += 1;
                self.note_underrun();
                out[i * channels..].iter_mut().for_each(|s| *s = 0.0);
                break;
            }
            let t = (self.pos - base) as f32;
            let (a, b, c, d) = (
                self.queue[idx - 1],
                self.queue[idx],
                self.queue[idx + 1],
                self.queue[idx + 2],
            );
            let left = catmull(a[0], b[0], c[0], d[0], t);
            let right = catmull(a[1], b[1], c[1], d[1], t);
            peak = peak.max(left.abs()).max(right.abs());
            let frame = &mut out[i * channels..(i + 1) * channels];
            if channels == 1 {
                frame[0] = (left + right) * 0.5 * gain;
            } else {
                frame[0] = left * gain;
                frame[1] = right * gain;
                frame[2..].iter_mut().for_each(|s| *s = 0.0);
            }
            self.pos += step;
            while self.pos >= 2.0 {
                self.queue.pop_front();
                self.pos -= 1.0;
            }
        }
        self.peak = peak;
    }

    fn note_underrun(&mut self) {
        let window = (AUTO_WINDOW_S * self.out_rate as f64) as u64;
        let now = self.clock;
        self.underrun_clock
            .retain(|&t| now.saturating_sub(t) < window);
        self.underrun_clock.push(now);
        self.stats.recent_underruns = self.underrun_clock.len();
        if self.auto && self.underrun_clock.len() >= 2 && self.target_ms < AUTO_MAX_MS {
            self.target_ms = (self.target_ms + AUTO_STEP_MS).min(AUTO_MAX_MS);
            self.underrun_clock.clear();
        }
    }

    /// Underruns within the last ten seconds of playback time.
    pub fn recent_underruns(&mut self) -> usize {
        let window = (AUTO_WINDOW_S * self.out_rate as f64) as u64;
        let now = self.clock;
        self.underrun_clock
            .retain(|&t| now.saturating_sub(t) < window);
        self.stats.recent_underruns = self.underrun_clock.len();
        self.stats.recent_underruns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, start: usize, rate: f32) -> Vec<[f32; 2]> {
        (start..start + n)
            .map(|i| {
                let v = (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / rate).sin() * 0.5;
                [v, -v]
            })
            .collect()
    }

    /// Feed `seconds` of audio in 128-frame packets while pulling 480-frame
    /// blocks, with the sender clock `drift` faster than the receiver.
    fn simulate(
        p: &mut Playout,
        in_rate: u32,
        out_rate: u32,
        seconds: f64,
        drift: f64,
    ) -> Vec<f64> {
        let mut out = vec![0.0f32; 480 * 2];
        let mut produced = 0usize;
        let mut seq = 0u32;
        let mut consumed_time = 0.0f64;
        let mut fills = Vec::new();
        while consumed_time < seconds {
            let due = ((consumed_time + 0.01) * in_rate as f64 * (1.0 + drift)) as usize;
            while produced + 128 <= due {
                p.push(&tone(128, produced, in_rate as f32), seq, in_rate);
                produced += 128;
                seq = seq.wrapping_add(1);
            }
            p.pull(&mut out, 2);
            consumed_time += 480.0 / out_rate as f64;
            fills.push(p.buffered_ms());
        }
        fills
    }

    #[test]
    fn starts_after_target_and_holds_it() {
        let mut p = Playout::new(40.0, false);
        let fills = simulate(&mut p, 48000, 48000, 5.0, 0.0);
        assert_eq!(p.phase(), Phase::Playing);
        assert_eq!(p.stats.underruns, 0);
        let tail = &fills[fills.len() - 100..];
        let avg = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!((avg - 40.0).abs() < 12.0, "average fill {avg}");
    }

    #[test]
    fn follows_clock_drift() {
        for drift in [0.0005, -0.0005] {
            let mut p = Playout::new(40.0, false);
            let fills = simulate(&mut p, 48000, 48000, 60.0, drift);
            assert_eq!(p.stats.underruns, 0, "drift {drift}");
            assert_eq!(p.stats.skips, 0, "drift {drift}");
            let tail = &fills[fills.len() - 200..];
            let avg = tail.iter().sum::<f64>() / tail.len() as f64;
            assert!(
                (avg - 40.0).abs() < 15.0,
                "drift {drift}: average fill {avg}"
            );
        }
    }

    #[test]
    fn converts_44100_to_48000() {
        let mut p = Playout::new(30.0, false);
        simulate(&mut p, 44100, 48000, 3.0, 0.0);
        assert_eq!(p.stats.underruns, 0);
        assert_eq!(p.phase(), Phase::Playing);
    }

    #[test]
    fn underrun_then_recovers_and_auto_grows() {
        let mut p = Playout::new(20.0, true);
        simulate(&mut p, 48000, 48000, 1.0, 0.0);
        let mut out = vec![0.0f32; 480 * 2];
        for _ in 0..10 {
            p.pull(&mut out, 2);
        }
        assert_eq!(p.phase(), Phase::Buffering);
        assert!(p.stats.underruns >= 1);
        // second starvation inside ten seconds raises the target
        p.push(&tone(128 * 10, 0, 48000.0), 100_000, 48000);
        for _ in 0..10 {
            p.pull(&mut out, 2);
        }
        assert!(p.target_ms() > 20.0);
        p.new_stream();
        assert_eq!(p.target_ms(), 20.0);
    }

    #[test]
    fn counts_and_conceals_lost_packets() {
        let mut p = Playout::new(20.0, false);
        let mut out = vec![0.0f32; 480 * 2];
        for seq in 0..20u32 {
            p.push(&tone(128, seq as usize * 128, 48000.0), seq, 48000);
        }
        p.pull(&mut out, 2);
        let before = p.buffered_ms();
        p.push(&tone(128, 0, 48000.0), 23, 48000);
        assert_eq!(p.stats.lost_packets, 3);
        let added = p.buffered_ms() - before;
        assert!((added - 4.0 * 128.0 / 48.0).abs() < 0.1, "added {added} ms");
        p.push(&tone(128, 0, 48000.0), 22, 48000);
        assert_eq!(p.stats.late_packets, 1);
    }

    #[test]
    fn trims_a_burst() {
        let mut p = Playout::new(20.0, false);
        let mut out = vec![0.0f32; 480 * 2];
        for seq in 0..10u32 {
            p.push(&tone(128, 0, 48000.0), seq, 48000);
        }
        p.pull(&mut out, 2);
        for seq in 10..200u32 {
            p.push(&tone(128, 0, 48000.0), seq, 48000);
        }
        assert!(p.stats.skips >= 1);
        assert!(p.buffered_ms() < 90.0);
    }

    #[test]
    fn volume_mute_and_channel_layouts() {
        let mut p = Playout::new(10.0, false);
        for seq in 0..20u32 {
            p.push(&vec![[0.5, -0.5]; 128], seq, 48000);
        }
        let mut six = vec![9.0f32; 64 * 6];
        p.pull(&mut six, 6);
        assert!((six[6] - 0.5).abs() < 1e-3 && (six[7] + 0.5).abs() < 1e-3);
        assert!(six[8..12].iter().all(|&s| s == 0.0));
        p.gain = 0.5;
        let mut mono = vec![1.0f32; 64];
        p.pull(&mut mono, 1);
        assert!(mono[10].abs() < 1e-3);
        p.muted = true;
        let mut two = vec![1.0f32; 128];
        p.pull(&mut two, 2);
        assert!(two.iter().all(|&s| s == 0.0));
        assert!(p.take_peak() > 0.4);
        assert_eq!(p.take_peak(), 0.0);
    }

    #[test]
    fn stays_bounded_when_nothing_pulls() {
        // Sound card gone: packets keep arriving, nothing is played.
        let mut p = Playout::new(40.0, false);
        for seq in 0..20_000u32 {
            p.push(&tone(128, 0, 48000.0), seq, 48000);
        }
        assert_eq!(p.phase(), Phase::Buffering);
        assert!(p.buffered_ms() <= 101.0, "buffered {}", p.buffered_ms());
        assert!(p.queue.len() < 48000);
    }

    #[test]
    fn ignores_a_target_that_is_not_a_number() {
        let mut p = Playout::new(40.0, true);
        p.set_target(f64::NAN, true);
        p.set_target(f64::INFINITY, true);
        assert_eq!(p.target_ms(), 40.0);
    }

    #[test]
    fn sender_restart_starts_over() {
        let mut p = Playout::new(20.0, false);
        for seq in 0..20u32 {
            p.push(&tone(128, 0, 48000.0), seq, 48000);
        }
        p.push(&tone(128, 0, 48000.0), 5_000_000, 48000);
        assert!(p.buffered_ms() < 3.0);
        assert_eq!(p.stats.lost_packets, 0);
    }
}

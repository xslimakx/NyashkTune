//! NyashkTune DSP core (no plugin-framework dependency).
//!
//! Pipeline
//! --------
//! input -> high-pass -> decimate -> YIN (with continuity) -> pitch *frames* (every ~4 ms)
//!       -> pitch-mark tracker (waveform-correlation aligned marks, one per period)
//!       -> grain synthesiser (PSOLA): for every output grain we
//!            1. pick the analysis mark that continues the previous one,
//!            2. look up the pitch *at that moment of the audio* (centred median of frames),
//!            3. decide target note / retune smoothing -> pitch ratio,
//!            4. overlap-add a Hann grain centred on the mark at the new spacing.
//! Because the audio is delayed by `latency` samples, step 2 can use pitch measurements from
//! both before and after the grain -> much steadier decisions than a purely causal tracker.
//! Formants stay put because every grain keeps its own spectral envelope.

use std::f64::consts::PI;

// ------------------------------------------------------------------------------------------
// Scales
// ------------------------------------------------------------------------------------------

pub const SCALE_NAMES: [&str; 17] = [
    "Chromatic",
    "Major",
    "Natural Minor",
    "Harmonic Minor",
    "Melodic Minor",
    "Major Pentatonic",
    "Minor Pentatonic",
    "Blues",
    "Dorian",
    "Phrygian",
    "Lydian",
    "Mixolydian",
    "Locrian",
    "Whole Tone",
    "Diminished",
    "Arabic (Hijaz)",
    "Custom",
];
pub const CUSTOM_SCALE: usize = 16;

const fn mask(notes: &[u8]) -> u16 {
    let mut m = 0u16;
    let mut i = 0;
    while i < notes.len() {
        m |= 1 << notes[i];
        i += 1;
    }
    m
}

pub const SCALE_MASKS: [u16; 17] = [
    0xFFF,
    mask(&[0, 2, 4, 5, 7, 9, 11]),
    mask(&[0, 2, 3, 5, 7, 8, 10]),
    mask(&[0, 2, 3, 5, 7, 8, 11]),
    mask(&[0, 2, 3, 5, 7, 9, 11]),
    mask(&[0, 2, 4, 7, 9]),
    mask(&[0, 3, 5, 7, 10]),
    mask(&[0, 3, 5, 6, 7, 10]),
    mask(&[0, 2, 3, 5, 7, 9, 10]),
    mask(&[0, 1, 3, 5, 7, 8, 10]),
    mask(&[0, 2, 4, 6, 7, 9, 11]),
    mask(&[0, 2, 4, 5, 7, 9, 10]),
    mask(&[0, 1, 3, 5, 6, 8, 10]),
    mask(&[0, 2, 4, 6, 8, 10]),
    mask(&[0, 2, 3, 5, 6, 8, 9, 11]),
    mask(&[0, 1, 4, 5, 7, 8, 10]),
    0xFFF,
];

/// Absolute pitch-class mask (bit 0 = C) for a scale/key, or the custom mask.
pub fn allowed_mask(scale: usize, key: usize, custom: u16) -> u16 {
    if scale == CUSTOM_SCALE {
        return custom & 0xFFF;
    }
    let m = SCALE_MASKS[scale.min(CUSTOM_SCALE)];
    let mut out = 0u16;
    for i in 0..12 {
        if m & (1 << i) != 0 {
            out |= 1 << ((i + key) % 12);
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// Parameters
// ------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Params {
    pub bypass: bool,
    pub key: usize,
    pub scale: usize,
    pub custom_mask: u16,
    pub ref_a: f32,
    pub retune_ms: f32,   // how fast the pitch settles onto the note
    pub transition_ms: f32, // how fast it glides when the target note changes
    pub amount: f32,      // 0..1
    pub flex_cents: f32,
    pub humanize: f32,    // 0..1
    pub natural_vib: f32, // 0..1
    pub transpose: f32,   // semitones
    pub fine_cents: f32,
    pub formant_semis: f32,
    pub formant_preserve: bool,
    pub vib_on: bool,
    pub vib_depth_cents: f32,
    pub vib_rate_hz: f32,
    pub vib_delay_ms: f32,
    pub midi_mode: bool,
    pub min_hz: f32,
    pub max_hz: f32,
    pub gate_db: f32,
    pub mix: f32, // 0..1
    pub in_gain_db: f32,
    pub out_gain_db: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            bypass: false,
            key: 0,
            scale: 1,
            custom_mask: 0xFFF,
            ref_a: 440.0,
            retune_ms: 50.0,
            transition_ms: 50.0,
            amount: 1.0,
            flex_cents: 0.0,
            humanize: 0.0,
            natural_vib: 0.0,
            transpose: 0.0,
            fine_cents: 0.0,
            formant_semis: 0.0,
            formant_preserve: true,
            vib_on: false,
            vib_depth_cents: 30.0,
            vib_rate_hz: 5.5,
            vib_delay_ms: 250.0,
            midi_mode: false,
            min_hz: 80.0,
            max_hz: 1000.0,
            gate_db: -60.0,
            mix: 1.0,
            in_gain_db: 0.0,
            out_gain_db: 0.0,
        }
    }
}

// ------------------------------------------------------------------------------------------
// Engine
// ------------------------------------------------------------------------------------------

pub const MAX_CH: usize = 2;
const YIN_W: usize = 320; // integration window on the decimated signal
const YIN_HOP: usize = 64; // decimated samples between pitch frames (~4 ms)
const FRAME_RING: usize = 1024;
const MARK_RING: usize = 1024;

#[derive(Clone, Copy, Default)]
struct Frame {
    time: i64,   // input sample index at which the frame was computed
    period: f32, // in input samples
    voiced: bool,
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}
impl Biquad {
    fn highpass(fs: f64, fc: f64) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let (s, c) = w0.sin_cos();
        let alpha = s / (2.0 * 0.70710678);
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + c) / 2.0 / a0,
            b1: -(1.0 + c) / a0,
            b2: (1.0 + c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    #[inline]
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

pub struct Meters {
    pub voiced: bool,
    pub hz: f32,
    pub target: f32, // MIDI note number (0 = none)
    pub corr: f32,   // applied shift in semitones
}

pub struct Engine {
    p: Params,
    allowed_pc: u16,
    pub held: [bool; 128],
    pub meters: Meters,
    pub dbg_lag: f64,
    pub pull: f64,
    pub debug: bool,

    fs: f64,
    fs_dec: f64,
    decim: usize,
    latency: i64,

    ring_mask: i64,
    ring: [Vec<f32>; MAX_CH],
    mono: Vec<f32>,
    ola: [Vec<f32>; MAX_CH],
    w: i64, // number of input samples written

    // analysis
    hp: Biquad,
    hp_fc: f64,
    dec: Vec<f32>,
    dec_w: i64,
    dec_acc: f64,
    dec_cnt: usize,
    since_yin: usize,
    yin_buf: Vec<f32>,
    yin_d: Vec<f32>,
    prev_tau: f32,
    unvoiced_run: u32,
    frames: Vec<Frame>,
    nframes: u64,
    last_period: f64,

    // pitch marks
    marks: Vec<i64>,
    nmarks: u64,

    // synthesis
    started: bool,
    next_centre: f64,
    next_decision: f64,
    content_nom: f64,
    prev_hs: f64,

    // tuning state
    corr_sm: f64,
    slow_midi: f64,
    note_age: f64,
    vib_phase: f64,
    have_note: bool,
    cur_note: i32,
    was_voiced: bool,
    transition: bool, // gliding towards a freshly selected note
}

#[inline]
fn rd(r: &[f32], mask: i64, k: i64) -> f32 {
    r[(k & mask) as usize]
}

impl Engine {
    pub fn new(sample_rate: f64, lowest_hz: f32) -> Self {
        let fs = sample_rate;
        let max_latency = (2.4 * fs / 40.0).ceil() as usize;
        let mut ring_size = 1usize;
        while ring_size < max_latency * 4 + 8192 {
            ring_size <<= 1;
        }
        let decim = ((fs / 16000.0).round() as usize).max(1);
        let mut e = Self {
            p: Params::default(),
            allowed_pc: 0xFFF,
            held: [false; 128],
            meters: Meters { voiced: false, hz: 0.0, target: 0.0, corr: 0.0 },
            dbg_lag: 0.0,
            pull: 0.6,
            debug: false,
            fs,
            fs_dec: fs / decim as f64,
            decim,
            latency: 1024,
            ring_mask: ring_size as i64 - 1,
            ring: [vec![0.0; ring_size], vec![0.0; ring_size]],
            mono: vec![0.0; ring_size],
            ola: [vec![0.0; ring_size], vec![0.0; ring_size]],
            w: 0,
            hp: Biquad::default(),
            hp_fc: 0.0,
            dec: vec![0.0; 4096],
            dec_w: 0,
            dec_acc: 0.0,
            dec_cnt: 0,
            since_yin: 0,
            yin_buf: Vec::with_capacity(2048),
            yin_d: Vec::with_capacity(2048),
            prev_tau: 0.0,
            unvoiced_run: 100,
            frames: vec![Frame::default(); FRAME_RING],
            nframes: 0,
            last_period: fs / 150.0,
            marks: vec![0; MARK_RING],
            nmarks: 0,
            started: false,
            next_centre: 0.0,
            next_decision: 0.0,
            content_nom: 0.0,
            prev_hs: fs / 150.0,
            corr_sm: 0.0,
            slow_midi: 60.0,
            note_age: 0.0,
            vib_phase: 0.0,
            have_note: false,
            cur_note: 60,
            was_voiced: false,
            transition: false,
        };
        e.set_lowest_hz(lowest_hz);
        e
    }

    /// Lowest pitch to track; decides the latency (about 2.2 periods). Allocation-free.
    pub fn set_lowest_hz(&mut self, hz: f32) {
        let hz = hz.clamp(40.0, 400.0) as f64;
        self.latency = (2.2 * self.fs / hz).ceil() as i64;
        self.hp_fc = (hz * 0.6).max(25.0);
        self.reset();
    }

    pub fn latency_samples(&self) -> u32 {
        self.latency as u32
    }

    pub fn reset(&mut self) {
        for c in 0..MAX_CH {
            self.ring[c].iter_mut().for_each(|v| *v = 0.0);
            self.ola[c].iter_mut().for_each(|v| *v = 0.0);
        }
        self.mono.iter_mut().for_each(|v| *v = 0.0);
        self.dec.iter_mut().for_each(|v| *v = 0.0);
        self.w = 0;
        self.dec_w = 0;
        self.dec_acc = 0.0;
        self.dec_cnt = 0;
        self.since_yin = 0;
        self.hp = Biquad::highpass(self.fs, self.hp_fc);
        self.prev_tau = 0.0;
        self.unvoiced_run = 100;
        self.nframes = 0;
        self.nmarks = 0;
        self.last_period = self.fs / 150.0;
        self.started = false;
        self.prev_hs = self.fs / 150.0;
        self.corr_sm = 0.0;
        self.slow_midi = 60.0;
        self.note_age = 0.0;
        self.vib_phase = 0.0;
        self.have_note = false;
        self.was_voiced = false;
        self.transition = false;
        self.meters = Meters { voiced: false, hz: 0.0, target: 0.0, corr: 0.0 };
    }

    pub fn set_params(&mut self, p: &Params) {
        self.allowed_pc = allowed_mask(p.scale, p.key, p.custom_mask);
        self.p = p.clone();
    }

    pub fn allowed_pitch_classes(&self) -> u16 {
        self.allowed_pc
    }

    #[inline]
    fn fallback(&self) -> f64 {
        self.fs / 150.0
    }

    // --------------------------------------------------------------------------------------
    // Per-sample processing
    // --------------------------------------------------------------------------------------
    pub fn process_frame(&mut self, inp: [f32; MAX_CH], nch: usize) -> [f32; MAX_CH] {
        let nch = nch.clamp(1, MAX_CH);
        let in_gain = 10f32.powf(self.p.in_gain_db / 20.0);
        let out_gain = 10f32.powf(self.p.out_gain_db / 20.0);
        let mix = self.p.mix.clamp(0.0, 1.0);

        // write input
        let mut m = 0.0f32;
        for c in 0..nch {
            let xi = if inp[c].is_finite() { inp[c] } else { 0.0 };
            let x = xi * in_gain;
            self.ring[c][(self.w & self.ring_mask) as usize] = x;
            m += x;
        }
        m /= nch as f32;
        self.mono[(self.w & self.ring_mask) as usize] = m;

        // analysis path: high-pass then decimate
        let h = self.hp.tick(m as f64);
        self.dec_acc += h;
        self.dec_cnt += 1;
        if self.dec_cnt == self.decim {
            self.dec[(self.dec_w & 4095) as usize] = (self.dec_acc / self.decim as f64) as f32;
            self.dec_w += 1;
            self.dec_acc = 0.0;
            self.dec_cnt = 0;
            self.since_yin += 1;
            if self.since_yin >= YIN_HOP {
                self.since_yin = 0;
                self.run_yin();
            }
        }
        self.w += 1;
        let o = self.w - 1; // output time index == current input sample

        if self.w & 15 == 0 {
            self.advance_marks();
        }

        if !self.started && self.w > 32 {
            self.started = true;
            self.next_centre = o as f64 + self.fallback();
            self.next_decision = o as f64;
            self.content_nom = self.next_centre - self.latency as f64;
            self.nmarks = 0; // resync marks to the stream
            self.advance_marks();
        }
        if self.started && o as f64 >= self.next_decision {
            self.place_grain(o, nch);
        }

        // output
        let mut out = [0.0f32; MAX_CH];
        let oi = (o & self.ring_mask) as usize;
        for c in 0..nch {
            let wet = self.ola[c][oi];
            self.ola[c][oi] = 0.0;
            let dry = rd(&self.ring[c], self.ring_mask, o - self.latency);
            let y = if self.p.bypass { dry } else { dry * (1.0 - mix) + wet * mix };
            out[c] = y * out_gain;
        }
        out
    }

    // --------------------------------------------------------------------------------------
    // YIN pitch frames
    // --------------------------------------------------------------------------------------
    fn run_yin(&mut self) {
        let min_tau = ((self.fs_dec / self.p.max_hz.clamp(200.0, 2000.0) as f64) as usize).max(2);
        let max_tau = (((self.fs_dec / self.p.min_hz.max(40.0) as f64) as usize) + 2).min(600);
        let need = YIN_W + max_tau + 2;
        if (self.dec_w as usize) < need {
            return;
        }

        self.yin_buf.clear();
        for i in 0..need {
            let k = self.dec_w - need as i64 + i as i64;
            self.yin_buf.push(self.dec[(k & 4095) as usize]);
        }
        let mut e = 0.0f64;
        for i in (need - YIN_W)..need {
            e += (self.yin_buf[i] as f64) * (self.yin_buf[i] as f64);
        }
        let rms_db = 10.0 * (e / YIN_W as f64 + 1e-12).log10();

        self.yin_d.clear();
        self.yin_d.resize(max_tau + 2, 1.0);
        let mut running = 0.0f64;
        for tau in 1..=max_tau {
            let x0 = &self.yin_buf[(need - YIN_W - tau)..(need - tau)];
            let x1 = &self.yin_buf[(need - YIN_W)..need];
            let mut d = 0.0f32;
            for j in 0..YIN_W {
                let df = x0[j] - x1[j];
                d += df * df;
            }
            running += d as f64;
            self.yin_d[tau] = (d as f64 * tau as f64 / (running + 1e-12)) as f32;
        }

        // candidates = local minima
        let mut cands: [(usize, f32); 12] = [(0, 0.0); 12];
        let mut nc = 0;
        for tau in min_tau..max_tau {
            let v = self.yin_d[tau];
            if v < 0.35 && v <= self.yin_d[tau - 1] && v < self.yin_d[tau + 1] && nc < cands.len() {
                cands[nc] = (tau, v);
                nc += 1;
            }
        }
        let cands = &cands[..nc];

        let mut best: Option<(usize, f32)> = cands.iter().copied().find(|c| c.1 < 0.15);
        if best.is_none() {
            best = cands.iter().copied().filter(|c| c.1 < 0.30).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        }
        // continuity: prefer a candidate within ~12 % of the previous period (kills octave jumps)
        if self.prev_tau > 0.0 {
            let prev = self.prev_tau;
            let near = cands
                .iter()
                .copied()
                .filter(|c| (c.0 as f32 / prev).ln().abs() < 0.12)
                .min_by(|a, b| {
                    (a.0 as f32 / prev).ln().abs().partial_cmp(&(b.0 as f32 / prev).ln().abs()).unwrap()
                });
            if let Some(n) = near {
                if best.map_or(true, |b| n.1 < b.1 + 0.12) {
                    best = Some(n);
                }
            }
        }

        let gate_open = rms_db > self.p.gate_db as f64;
        let mut frame = Frame { time: self.w, period: self.last_period as f32, voiced: false };
        if let (Some((tau, _)), true) = (best, gate_open) {
            let (a, b, c) = (self.yin_d[tau - 1] as f64, self.yin_d[tau] as f64, self.yin_d[tau + 1] as f64);
            let den = a - 2.0 * b + c;
            let shift = if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
            let tau_f = tau as f64 + shift;
            frame.period = (tau_f * self.decim as f64) as f32;
            frame.voiced = true;
            self.prev_tau = tau_f as f32;
            self.unvoiced_run = 0;
        } else {
            self.unvoiced_run += 1;
            if self.unvoiced_run > 4 {
                self.prev_tau = 0.0;
            }
        }
        self.frames[(self.nframes % FRAME_RING as u64) as usize] = frame;
        self.nframes += 1;
    }

    /// Pitch of the audio around input time `t` (centred median of nearby frames).
    fn query(&self, t: f64) -> (bool, f64) {
        if self.nframes == 0 {
            return (false, self.fallback());
        }
        let hop = (YIN_HOP * self.decim) as f64;
        let target = t + 0.5 * (YIN_W * self.decim) as f64 + 0.5 * self.last_period;
        let mut periods = [0.0f32; 8];
        let (mut n, mut nv) = (0usize, 0usize);

        let newest = self.frames[((self.nframes - 1) % FRAME_RING as u64) as usize];
        let target = target.min(newest.time as f64);

        let mut i = self.nframes;
        let mut scanned = 0;
        while i > 0 && scanned < 64 && n < 8 {
            i -= 1;
            scanned += 1;
            let f = self.frames[(i % FRAME_RING as u64) as usize];
            let dtm = f.time as f64 - target;
            if dtm > 2.5 * hop {
                continue;
            }
            if dtm < -2.5 * hop {
                break;
            }
            n += 1;
            if f.voiced {
                periods[nv] = f.period;
                nv += 1;
            }
        }
        if n == 0 || nv * 2 < n || nv == 0 {
            return (false, self.fallback());
        }
        let s = &mut periods[..nv];
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (true, s[nv / 2] as f64)
    }

    // --------------------------------------------------------------------------------------
    // Pitch marks (waveform-aligned, one per period)
    // --------------------------------------------------------------------------------------
    fn newest_mark(&self) -> i64 {
        self.marks[((self.nmarks - 1) % MARK_RING as u64) as usize]
    }
    fn push_mark(&mut self, m: i64) {
        self.marks[(self.nmarks % MARK_RING as u64) as usize] = m;
        self.nmarks += 1;
    }

    fn advance_marks(&mut self) {
        let max_p = self.fs / 40.0;
        if self.nmarks == 0 {
            self.push_mark(self.w - self.latency - 2 * self.fallback() as i64);
        }
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 64 {
                break;
            }
            let last = self.newest_mark();
            if (self.w - last) as f64 > 6.0 * max_p {
                self.push_mark(self.w - self.latency - self.fallback() as i64);
                continue;
            }
            let (voiced, pq) = self.query(last as f64);
            let p = if voiced { pq } else { self.fallback() };
            if voiced {
                self.last_period += 0.2 * (p - self.last_period);
            }
            let cand = (last as f64 + p).round() as i64;
            let r = if voiced { ((0.2 * p) as i64).max(2) } else { 0 };
            let half = (p * 0.5) as i64;
            if cand + r + half + 2 >= self.w {
                break;
            }
            let mut best = cand;
            if voiced {
                best = self.refine_mark(last, cand, r, p as usize);
            }
            if best <= last + (0.5 * p) as i64 {
                best = cand;
            }
            self.push_mark(best);
        }
    }

    /// Choose the offset around `cand` whose waveform segment best matches the segment at `last`.
    fn refine_mark(&self, last: i64, cand: i64, r: i64, len: usize) -> i64 {
        let len = len.max(16) as i64;
        let h = len / 2;
        let m = self.ring_mask;
        let mut e_ref = 1e-12f64;
        let mut j = 0;
        while j < len {
            let v = rd(&self.mono, m, last - h + j) as f64;
            e_ref += v * v;
            j += 2;
        }
        let mut best = cand;
        let mut best_score = f64::MIN;
        for d in -r..=r {
            let (mut dot, mut e) = (0.0f64, 1e-12f64);
            let mut j = 0;
            while j < len {
                let a = rd(&self.mono, m, last - h + j) as f64;
                let b = rd(&self.mono, m, cand + d - h + j) as f64;
                dot += a * b;
                e += b * b;
                j += 2;
            }
            let score = dot / (e_ref * e).sqrt() - 0.0004 * (d.abs() as f64) / (r as f64);
            if score > best_score {
                best_score = score;
                best = cand + d;
            }
        }
        best
    }

    fn pick_mark(&self, nom: f64) -> i64 {
        let n = self.nmarks;
        if n == 0 {
            return nom.round() as i64;
        }
        let oldest = n.saturating_sub(MARK_RING as u64 - 2);
        let mut i = n;
        while i > oldest {
            i -= 1;
            let m = self.marks[(i % MARK_RING as u64) as usize];
            if (m as f64) <= nom {
                if i + 1 < n {
                    let up = self.marks[((i + 1) % MARK_RING as u64) as usize];
                    if (up as f64 - nom).abs() < (nom - m as f64).abs() {
                        return up;
                    }
                }
                return m;
            }
        }
        self.marks[(oldest % MARK_RING as u64) as usize]
    }

    // --------------------------------------------------------------------------------------
    // Tuning logic
    // --------------------------------------------------------------------------------------
    fn is_allowed(&self, m: i32) -> bool {
        let pc = m.rem_euclid(12) as usize;
        if self.p.midi_mode {
            let mut n = pc;
            while n < 128 {
                if self.held[n] {
                    return true;
                }
                n += 12;
            }
            return false;
        }
        (self.allowed_pc >> pc) & 1 == 1
    }

    fn pick_note(&mut self, basis: f64) -> bool {
        let base = basis.floor() as i32;
        let mut best = None;
        let mut bd = 1e9;
        for m in (base - 7)..=(base + 8) {
            if self.is_allowed(m) {
                let d = (basis - m as f64).abs();
                if d < bd {
                    bd = d;
                    best = Some(m);
                }
            }
        }
        let Some(best) = best else {
            self.have_note = false;
            return false;
        };
        const HYST: f64 = 0.15;
        if self.have_note && self.is_allowed(self.cur_note) {
            if best != self.cur_note && (basis - self.cur_note as f64).abs() > bd + HYST {
                self.cur_note = best;
                self.note_age = 0.0;
                self.transition = true;
            }
        } else {
            self.cur_note = best;
            self.have_note = true;
            self.note_age = 0.0;
            self.transition = true; // a new note after a pause counts as a note transition
        }
        true
    }

    fn compute_ratio(&mut self, dt: f64, voiced: bool, period: f64) -> f64 {
        let user = self.p.transpose as f64 + self.p.fine_cents as f64 / 100.0;

        if !voiced {
            self.was_voiced = false;
            self.corr_sm += (0.0 - self.corr_sm) * (1.0 - (-dt / 0.02).exp());
            self.meters.voiced = false;
            self.meters.corr = 0.0;
            // consonants / breath: pass through untouched (shifting noise only adds roughness)
            return 1.0;
        }

        let f0 = self.fs / period;
        let midi = 69.0 + 12.0 * (f0 / self.p.ref_a as f64).log2();
        if !self.was_voiced {
            self.slow_midi = midi;
            self.have_note = false;
            self.note_age = 0.0;
            self.transition = true;
            self.was_voiced = true;
        }
        self.slow_midi += (midi - self.slow_midi) * (1.0 - (-dt / 0.12).exp());

        let nv = (self.p.natural_vib as f64).clamp(0.0, 1.0);
        let basis = midi + (self.slow_midi - midi) * nv;

        let mut err = 0.0;
        if self.pick_note(basis) {
            err = self.cur_note as f64 - basis;
            let fl = self.p.flex_cents as f64 / 100.0;
            err = if err > fl { err - fl } else if err < -fl { err + fl } else { 0.0 };
            err *= (self.p.amount as f64).clamp(0.0, 1.0);
        }

        // glide time: Note Transition while moving onto a newly selected note,
        // Retune Speed for settling inside a note
        let mut tau = if self.transition { self.p.transition_ms } else { self.p.retune_ms } as f64 / 1000.0;
        if self.p.humanize > 0.0 {
            tau *= 1.0 + self.p.humanize as f64 * 4.0 * (1.0 - (err.abs() / 0.5).min(1.0));
        }
        if tau < 0.0005 {
            self.corr_sm = err;
        } else {
            self.corr_sm += (err - self.corr_sm) * (1.0 - (-dt / tau).exp());
        }
        // the transition is over once the correction has arrived (within ~5 cents)
        // (and the pitch measurement has had ~80 ms to settle after the note change)
        if self.transition && self.note_age > 0.08 && (err - self.corr_sm).abs() < 0.05 {
            self.transition = false;
        }

        self.note_age += dt;
        let mut vib = 0.0;
        if self.p.vib_on {
            self.vib_phase += 2.0 * PI * self.p.vib_rate_hz as f64 * dt;
            if self.vib_phase > 2.0 * PI {
                self.vib_phase -= 2.0 * PI;
            }
            let ramp = ((self.note_age - self.p.vib_delay_ms as f64 / 1000.0) / 0.3).clamp(0.0, 1.0);
            vib = self.p.vib_depth_cents as f64 / 100.0 * ramp * self.vib_phase.sin();
        }

        let total = (self.corr_sm + vib + user).clamp(-12.0, 12.0);
        self.meters.voiced = true;
        self.meters.hz = f0 as f32;
        self.meters.target = if self.have_note { self.cur_note as f32 } else { 0.0 };
        self.meters.corr = total as f32;
        2f64.powf(total / 12.0)
    }

    // --------------------------------------------------------------------------------------
    // Grain synthesis
    // --------------------------------------------------------------------------------------
    fn read_cubic(&self, ch: usize, pos: f64) -> f32 {
        let i = pos.floor() as i64;
        let f = (pos - i as f64) as f32;
        let last = self.w - 1;
        let at = |k: i64| rd(&self.ring[ch], self.ring_mask, k.min(last));
        let (y0, y1, y2, y3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let c1 = 0.5 * (y2 - y0);
        let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
        let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
        ((c3 * f + c2) * f + c1) * f + y1
    }

    fn place_grain(&mut self, o: i64, nch: usize) {
        // choose the analysis centre: the nearest pitch mark, unless marks are too sparse here
        // (e.g. just after an unvoiced stretch) - then use the wanted position itself.
        let (_, p0) = self.query(self.content_nom);
        let mut m = self.pick_mark(self.content_nom);
        if (m as f64 - self.content_nom).abs() > 0.6 * p0 {
            m = self.content_nom.round() as i64;
        }
        let (voiced, pq) = self.query(m as f64);
        let p = if voiced { pq } else { self.fallback() };

        let dt = self.prev_hs / self.fs;
        let r = self.compute_ratio(dt, voiced, p);

        let hs = p / r;
        let hg = (p.max(hs).ceil() as i64).max(8);
        let s = (2f64.powf(self.p.formant_semis as f64 / 12.0)
            * if self.p.formant_preserve { 1.0 } else { r })
        .clamp(0.5, 2.0);

        let tc = self.next_centre;
        self.dbg_lag = (tc - m as f64) - self.latency as f64;
        let oi = tc.floor() as i64;
        let f = tc - oi as f64;
        let scale = hs / hg as f64;

        let mut kmin = -hg;
        let clipped = oi + kmin < o;
        if clipped {
            kmin = o - oi;
        }
        if self.debug {
            eprintln!("o={:.4}s tc={:.0} m={} voiced={} p={:.0} r={:.3} hs={:.0} hg={} lag={:.0} clipped={} (lost {} samp)", o as f64 / self.fs, tc, m, voiced, p, r, hs, hg, self.dbg_lag, clipped, if clipped { o - (oi - hg) } else { 0 });
        }
        for k in kmin..hg {
            let u = k as f64 - f;
            let wv = (scale * 0.5 * (1.0 + (PI * u / hg as f64).cos())) as f32;
            let sp = m as f64 + u * s;
            let idx = ((oi + k) & self.ring_mask) as usize;
            for ch in 0..nch {
                let v = self.read_cubic(ch, sp);
                self.ola[ch][idx] += wv * v;
            }
        }

        self.next_centre = tc + hs;
        // continue from the chosen mark; slowly pull back towards the nominal latency
        let nom = m as f64 + hs;
        let lag_err = (self.next_centre - self.latency as f64) - nom;
        self.content_nom = nom + (self.pull * lag_err).clamp(-0.5 * hs, 0.5 * hs);
        self.prev_hs = hs;
        self.next_decision = self.next_centre - (1.3 * p.max(hs).max(self.fallback()) + 4.0);
        if self.next_decision < o as f64 - hs {
            self.next_centre = o as f64 + hs;
            self.next_decision = o as f64;
            self.content_nom = self.next_centre - self.latency as f64;
        }
    }
}

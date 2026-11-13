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

use crate::formant::{FormantMode, FormantShifter};
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

/// Number of extra (harmony / double) voices next to the corrected lead voice.
pub const NUM_VOICES: usize = 2;

/// One extra voice, sung from the same analysed signal as the lead.
#[derive(Clone, Copy, Debug)]
pub struct VoiceParams {
    pub on: bool,
    pub interval: f32,      // semitones relative to the corrected lead pitch
    pub snap: bool,         // keep the interval inside the scale
    pub level_db: f32,
    pub pan: f32,           // -1..1
    pub detune_cents: f32,
    pub formant_semis: f32, // on top of the main formant shift
    pub delay_ms: f32,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self { on: false, interval: 0.0, snap: false, level_db: -9.0, pan: 0.0, detune_cents: 0.0, formant_semis: 0.0, delay_ms: 0.0 }
    }
}

#[derive(Clone, Debug)]
pub struct Params {
    pub bypass: bool,
    pub key: usize,
    pub scale: usize,
    pub custom_mask: u16,
    pub ref_a: f32,
    pub retune_ms: f32,   // how fast the pitch settles onto the note
    pub transition_ms: f32, // how fast it glides when the target note changes
    pub amount: f32,      // 0..1 (Correction %)
    pub transpose: f32,   // semitones
    pub fine_cents: f32,
    pub formant_semis: f32,
    pub formant_preserve: bool,
    pub formant_on: bool,    // off: formants are never touched (and no FFT is run)
    pub voices_on: bool,     // master switch of the extra voices (off: nothing is computed for them)
    pub colour_on: bool,     // off: Air / Drive are skipped
    pub formant_mode: FormantMode,
    pub formant_tilt: f32,   // -1..1, formant shift applied more to highs (+) or lows (-)
    pub formant_follow: f32, // 0..1, how much the formants follow the pitch correction when not preserved
    /// Per pitch class (C = 0): 0 follow the scale, 1 legal, 2 illegal (to the nearest legal note),
    /// 3 illegal - pitch up (to the next legal note above), 4 illegal - pitch down, 5 bypass
    /// (legal, but sung as it is).
    pub note_override: [u8; 12],
    /// Tolerance: the voice must pass 50 + `tol_cents` cents from the current note, for `tol_ms`,
    /// before the target note changes (0 / 0 = straight to the nearest legal note).
    pub tol_cents: f32,
    pub tol_ms: f32,
    /// Vibrato On: the note centre is corrected and the singer's own vibrato is kept, scaled by
    /// `vib_amount` (0 flat .. 1 as sung .. 2 exaggerated). Off: the whole pitch is corrected.
    pub keep_vibrato: bool,
    pub vib_amount: f32,
    /// Correction range (MIDI notes): outside it the voice passes uncorrected.
    pub range_lo: f32,
    pub range_hi: f32,
    pub vib_shape: usize,    // 0 sine, 1 triangle, 2 saw, 3 random
    pub air_db: f32,         // high shelf on the wet signal
    pub drive: f32,          // 0..1 soft saturation on the wet signal
    pub voices: [VoiceParams; NUM_VOICES],
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
            retune_ms: 15.0,
            transition_ms: 120.0,
            amount: 1.0,
            transpose: 0.0,
            fine_cents: 0.0,
            formant_semis: 0.0,
            formant_preserve: true,
            formant_on: true,
            voices_on: true,
            colour_on: true,
            formant_mode: FormantMode::Classic,
            formant_tilt: 0.0,
            formant_follow: 1.0,
            note_override: [0; 12],
            tol_cents: 0.0,
            tol_ms: 0.0,
            keep_vibrato: true,
            vib_amount: 1.0,
            range_lo: 0.0,
            range_hi: 127.0,
            vib_shape: 0,
            air_db: 0.0,
            drive: 0.0,
            voices: [VoiceParams::default(); NUM_VOICES],
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
struct Synth {
    next_centre: f64,
    next_decision: f64,
    content_nom: f64,
    prev_hs: f64,
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
    /// RBJ high shelf (S = 1).
    fn highshelf(fs: f64, fc: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * fc.min(fs * 0.45) / fs;
        let (sn, cs) = w0.sin_cos();
        let alpha = sn / 2.0 * 2f64.sqrt();
        let sq = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cs + sq;
        Self {
            b0: a * ((a + 1.0) + (a - 1.0) * cs + sq) / a0,
            b1: -2.0 * a * ((a - 1.0) + (a + 1.0) * cs) / a0,
            b2: a * ((a + 1.0) + (a - 1.0) * cs - sq) / a0,
            a1: 2.0 * ((a - 1.0) - (a + 1.0) * cs) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cs - sq) / a0,
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
    /// effective note states (1 legal, 2 illegal, 3 up, 4 down, 5 bypass) per pitch class
    states: [u8; 12],
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

    // synthesis: voice 0 is the corrected lead, 1.. are the extra voices
    started: bool,
    syn: [Synth; NUM_VOICES + 1],
    vact: [bool; NUM_VOICES + 1],
    vcomp: f32,                // smoothed output level compensation while voices are on
    vdelta: [f64; NUM_VOICES], // smoothed interval (semitones) of the extra voices
    cur_r: f64,                // pitch ratio chosen for the lead's latest grain
    shifter: FormantShifter,
    gbuf: [Vec<f32>; MAX_CH],

    // wet-signal colour
    air: [Biquad; MAX_CH],
    air_cache: f32,
    rng: u32,
    rnd_prev: f64,
    rnd_next: f64,

    // tuning state
    corr_sm: f64,
    slow_midi: f64,
    note_age: f64,
    vib_phase: f64,
    have_note: bool,
    cur_note: i32,
    was_voiced: bool,
    transition: bool, // gliding towards a freshly selected note
    cand_note: i32,   // a different note the pitch is moving to (must hold a moment before we switch)
    cand_age: f64,
    far_note: i32, // the voice is clearly nearer to another note (see pick_note)
    far_age: f64,
    note_sm: f64,
    centre: f64,      // note centre: vibrato taken out (midpoint of its last peak and trough)
    vib_sm: f64,      // lightly smoothed pitch the vibrato turning points are found on
    ext: f64,         // running extreme since the last turning point
    ext_t: f64,
    rising: bool,
    tp_max: (f64, f64), // last peak (value, time)
    tp_min: (f64, f64), // last trough
    clock: f64,     // lightly smoothed pitch used only to *choose* the note (not to correct it)
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
            states: [1; 12],
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
            syn: [Synth::default(); NUM_VOICES + 1],
            vact: [false; NUM_VOICES + 1],
            vdelta: [0.0; NUM_VOICES],
            vcomp: 1.0,
            cur_r: 1.0,
            shifter: FormantShifter::new(((8.0 * fs / 40.0).ceil() as usize + 64).next_power_of_two()),
            gbuf: [vec![0.0; (4.0 * fs / 40.0).ceil() as usize + 64], vec![0.0; (4.0 * fs / 40.0).ceil() as usize + 64]],
            air: [Biquad::default(); MAX_CH],
            air_cache: f32::NAN,
            rng: 0x1234_5678,
            rnd_prev: 0.0,
            rnd_next: 0.0,
            corr_sm: 0.0,
            slow_midi: 60.0,
            note_age: 0.0,
            vib_phase: 0.0,
            have_note: false,
            cur_note: 60,
            was_voiced: false,
            transition: false,
            cand_note: i32::MIN,
            cand_age: 0.0,
            far_note: i32::MIN,
            far_age: 0.0,
            note_sm: 60.0,
            centre: 60.0,
            vib_sm: 60.0,
            ext: 60.0,
            ext_t: 0.0,
            rising: true,
            tp_max: (f64::NAN, -1.0),
            tp_min: (f64::NAN, -1.0),
            clock: 0.0,
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
        self.syn = [Synth { prev_hs: self.fs / 150.0, ..Synth::default() }; NUM_VOICES + 1];
        self.vact = [false; NUM_VOICES + 1];
        self.vdelta = [0.0; NUM_VOICES];
        self.vcomp = 1.0;
        self.cur_r = 1.0;
        for a in self.air.iter_mut() {
            a.z1 = 0.0;
            a.z2 = 0.0;
        }
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
        for pc in 0..12 {
            let ov = p.note_override[pc];
            self.states[pc] = if (1..=5).contains(&ov) {
                ov
            } else if (self.allowed_pc >> pc) & 1 == 1 {
                1
            } else {
                2
            };
        }
        if p.air_db != self.air_cache {
            self.air_cache = p.air_db;
            for a in self.air.iter_mut() {
                let (z1, z2) = (a.z1, a.z2);
                *a = Biquad::highshelf(self.fs, 7000.0, p.air_db as f64);
                a.z1 = z1;
                a.z2 = z2;
            }
        }
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
            let nc = o as f64 + self.fallback();
            let s0 = Synth { next_centre: nc, next_decision: o as f64, content_nom: nc - self.latency as f64, prev_hs: self.fallback() };
            self.syn = [s0; NUM_VOICES + 1];
            self.nmarks = 0; // resync marks to the stream
            self.advance_marks();
        }
        if self.started && o as f64 >= self.syn[0].next_decision {
            self.place_grain(0, o, nch);
        }
        if self.started {
            for v in 1..=NUM_VOICES {
                if self.p.voices_on && self.p.voices[v - 1].on && !self.p.bypass {
                    if !self.vact[v] {
                        // join in step with the lead
                        self.syn[v] = self.syn[0];
                        self.vact[v] = true;
                    }
                    if o as f64 >= self.syn[v].next_decision {
                        self.place_grain(v, o, nch);
                    }
                } else {
                    self.vact[v] = false;
                }
            }
        }

        // output
        let mut out = [0.0f32; MAX_CH];
        let oi = (o & self.ring_mask) as usize;
        let drive = if self.p.colour_on { self.p.drive.clamp(0.0, 1.0) } else { 0.0 };
        let air_on = self.p.colour_on && self.p.air_db.abs() > 0.05;
        // Voices add to the lead: keep the sum from running away (power sum, only when it would
        // be well above unity), then a soft knee as a last safety so nothing hard-clips.
        let mut pw = 1.0f32;
        let mut any_v = false;
        for v in 0..NUM_VOICES {
            if self.p.voices_on && self.p.voices[v].on && !self.p.bypass {
                let lv = 10f32.powf(self.p.voices[v].level_db / 20.0);
                pw += lv * lv;
                any_v = true;
            }
        }
        let comp_t = if any_v { (1.25 / pw.sqrt()).min(1.0) } else { 1.0 };
        self.vcomp += (comp_t - self.vcomp) * (1.0 / (0.03 * self.fs as f32)).min(1.0);
        for c in 0..nch {
            let mut wet = self.ola[c][oi];
            self.ola[c][oi] = 0.0;
            if any_v || self.vcomp != 1.0 {
                wet *= self.vcomp;
                let a = wet.abs();
                if a > 0.85 {
                    wet = wet.signum() * (0.85 + 0.15 * ((a - 0.85) / 0.15).tanh());
                }
            }
            if air_on {
                wet = self.air[c].tick(wet as f64) as f32;
            }
            if drive > 0.0 {
                let g = 1.0 + 3.0 * drive;
                wet = (wet * g).tanh() / g.sqrt();
            }
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
                // YIN also dips at 2x / 3x the true period: after a leap up an octave the old period
                // is exactly such a multiple. Continuity must not keep us on it while a clear dip at
                // 1/2 or 1/3 of it is there - that one is the real fundamental.
                let sub_of_fund = cands.iter().any(|c| {
                    c.1 < 0.15 && c.1 < n.1 + 0.08 && [2.0f32, 3.0].iter().any(|k| ((n.0 as f32 / c.0 as f32) / k).ln().abs() < 0.05)
                });
                if !sub_of_fund && best.map_or(true, |b| n.1 < b.1 + 0.12) {
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

    /// The pitch mark right after `m`, if it is already known.
    fn mark_after(&self, m: i64) -> Option<i64> {
        let n = self.nmarks;
        let oldest = n.saturating_sub(MARK_RING as u64 - 2);
        let mut i = n;
        let mut after = None;
        while i > oldest {
            i -= 1;
            let k = self.marks[(i % MARK_RING as u64) as usize];
            if k > m {
                after = Some(k);
            } else {
                break;
            }
        }
        after
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
        matches!(self.states[pc], 1 | 5)
    }

    fn nearest_allowed(&self, v: f64) -> Option<(i32, f64)> {
        let base = v.floor() as i32;
        let mut best = None;
        let mut bd = 1e9;
        for m in (base - 7)..=(base + 8) {
            if self.is_allowed(m) {
                let d = (v - m as f64).abs();
                if d < bd {
                    bd = d;
                    best = Some(m);
                }
            }
        }
        best.map(|m| (m, bd))
    }

    /// The target for a pitch `v` by the note states: a legal (or bypass) note is its own target,
    /// an illegal one goes to the nearest legal note, "pitch up" / "pitch down" to the next legal
    /// note in that direction.
    fn target_for(&self, v: f64) -> Option<(i32, f64)> {
        if !self.p.midi_mode {
            let n = v.round() as i32;
            match self.states[n.rem_euclid(12) as usize] {
                1 | 5 => return Some((n, (v - n as f64).abs())),
                3 => return ((n + 1)..=(n + 12)).find(|m| self.is_allowed(*m)).map(|m| (m, (v - m as f64).abs())),
                4 => return ((n - 12)..n).rev().find(|m| self.is_allowed(*m)).map(|m| (m, (v - m as f64).abs())),
                _ => {}
            }
        }
        self.nearest_allowed(v)
    }

    /// Choose the target note (Waves-Tune-style): the target for the current pitch by the note
    /// states; it changes once the voice is past 50 + Tolerance-cents from the current note and has
    /// stayed there for Tolerance-time (both off: almost at once, with a small guard against flicker).
    /// `basis` is what the choice is made on (the note centre with Vibrato On), `raw` the plain pitch.
    fn pick_note(&mut self, basis: f64, raw: f64, dt: f64) -> bool {
        let Some((best, _)) = self.target_for(basis) else {
            self.have_note = false;
            return false;
        };
        if !(self.have_note && self.is_allowed(self.cur_note)) {
            self.cur_note = best;
            self.have_note = true;
            self.note_age = 0.0;
            self.transition = true; // a new note after a pause counts as a note transition
            self.cand_note = i32::MIN;
            self.far_note = i32::MIN;
            return true;
        }
        let cur = self.cur_note;
        let switch = |e: &mut Self, m: i32| {
            e.cur_note = m;
            e.note_age = 0.0;
            e.transition = true;
            e.cand_note = i32::MIN;
            e.cand_age = 0.0;
            e.far_note = i32::MIN;
            e.far_age = 0.0;
        };
        // Safety, whatever the Tolerance: never pull the voice further than to its own target.
        // When the plain pitch is clearly (> 0.9 st) nearer its target than the current note for
        // ~12 ms, switch - fast slides and detector octave slips used to hold the old note and pull
        // the voice 2-12 st. Being away is what must last, not one particular new note.
        let tol = (self.p.tol_cents as f64 / 100.0).clamp(0.0, 0.4);
        if let Some((rb, rbd)) = self.target_for(raw) {
            if rb != cur && (raw - cur as f64).abs() - rbd > 0.9 + tol {
                self.far_note = rb;
                self.far_age += dt;
            } else {
                self.far_note = i32::MIN;
                self.far_age = 0.0;
            }
            if self.far_note == rb && self.far_age >= 0.012 {
                switch(self, rb);
                self.note_sm = raw;
                self.centre = raw;
                return true;
            }
        }
        // Tolerance (an 8 c / 15 ms guard even when off, against flicker right on a note border)
        let d_cur = (basis - cur as f64).abs();
        let past = d_cur > 0.5 + tol.max(0.08);
        let hold = (self.p.tol_ms as f64 / 1000.0).clamp(0.0, 0.3).max(0.015);
        // past the Tolerance border, but not past the small guard: allowed after a longer while, so
        // the guard only stops quick flicker and never keeps a note the voice has clearly left
        let past_border = d_cur > 0.5 + tol;
        if best != cur && past_border {
            if self.cand_note == best {
                self.cand_age += dt;
            } else {
                self.cand_note = best;
                self.cand_age = 0.0;
            }
            if (past && self.cand_age >= hold) || self.cand_age >= hold + 0.12 {
                switch(self, best);
            }
        } else {
            self.cand_note = i32::MIN;
            self.cand_age = 0.0;
        }
        true
    }

    /// Follow the note centre: the midpoint of the last vibrato peak and trough while the voice
    /// oscillates like vibrato (2-16 Hz, under 2 st wide, still inside it), the pitch itself otherwise
    /// (a slide or a jump is followed at once).
    fn track_centre(&mut self, midi: f64, dt: f64) {
        self.clock += dt;
        let t = self.clock;
        self.vib_sm += (midi - self.vib_sm) * (1.0 - (-dt / 0.008).exp());
        let s = self.vib_sm;
        const TURN: f64 = 0.06; // 6 c back from the extreme = a turning point
        if self.rising {
            if s >= self.ext {
                self.ext = s;
                self.ext_t = t;
            } else if s < self.ext - TURN {
                self.tp_max = (self.ext, self.ext_t);
                self.rising = false;
                self.ext = s;
                self.ext_t = t;
            }
        } else if s <= self.ext {
            self.ext = s;
            self.ext_t = t;
        } else if s > self.ext + TURN {
            self.tp_min = (self.ext, self.ext_t);
            self.rising = true;
            self.ext = s;
            self.ext_t = t;
        }
        let (hi, th) = self.tp_max;
        let (lo, tl) = self.tp_min;
        let half = (th - tl).abs();
        let vibrato = hi.is_finite()
            && lo.is_finite()
            && hi > lo
            && hi - lo < 2.0
            && (0.03..=0.25).contains(&half)
            && t - th.max(tl) < 0.3
            // still inside the band of its last peak and trough: after a step to another note the
            // old peak / trough must not pull the centre back half-way
            && s > lo - 0.15
            && s < hi + 0.15;
        let target = if vibrato { 0.5 * (hi + lo) } else { s };
        self.centre += (target - self.centre) * (1.0 - (-dt / 0.025).exp());
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
            self.note_sm = midi;
            self.centre = midi;
            self.vib_sm = midi;
            self.ext = midi;
            self.tp_max = (f64::NAN, -1.0);
            self.tp_min = (f64::NAN, -1.0);
            self.have_note = false;
            self.note_age = 0.0;
            self.transition = true;
            self.was_voiced = true;
        }
        self.slow_midi += (midi - self.slow_midi) * (1.0 - (-dt / 0.12).exp());
        self.track_centre(midi, dt);

        // a lightly smoothed pitch; a step of ~a semitone or more is taken at once (smoothing it
        // would crawl through the notes in between and could settle on one of them)
        if (midi - self.note_sm).abs() > 0.8 {
            self.note_sm = midi;
        } else {
            self.note_sm += (midi - self.note_sm) * (1.0 - (-dt / 0.010).exp());
        }
        if (self.note_sm - self.centre).abs() > 1.2 {
            self.centre = self.note_sm; // left the vibrato: follow at once
        }
        let keep = self.p.keep_vibrato;
        // the note is always chosen on the centre: vibrato never flips it to a neighbour, in either
        // mode (Vibrato On / Off only decides whether the vibrato itself is corrected away)
        let basis = self.centre;
        let mut err = 0.0;
        if self.pick_note(basis, midi, dt) {
            // Vibrato On: the centre goes onto the note and the singer's vibrato rides on top,
            // scaled by Depth (0 flat, 1 as sung, 2 twice as wide). Off: the pitch itself is corrected.
            err = if keep {
                let d = (self.p.vib_amount as f64).clamp(0.0, 2.0);
                self.cur_note as f64 - self.centre + (d - 1.0) * (midi - self.centre)
            } else {
                self.cur_note as f64 - midi
            };
            // the correction never needs to move the voice by an octave: taken modulo 12, a
            // detector octave slip or a register flip costs nothing
            err -= 12.0 * (err / 12.0).round();
            err *= (self.p.amount as f64).clamp(0.0, 1.0);
            let cn = self.cur_note as f32;
            if self.states[self.cur_note.rem_euclid(12) as usize] == 5 || cn < self.p.range_lo || cn > self.p.range_hi {
                err = 0.0; // BYPASS note, or outside the correction range: sung as it is
            }
        }

        // glide time: Note Transition while moving onto a newly selected note,
        // Speed for settling inside a note
        let tau = if self.transition { self.p.transition_ms } else { self.p.retune_ms } as f64 / 1000.0;
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
                self.rnd_prev = self.rnd_next;
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 17;
                self.rng ^= self.rng << 5;
                self.rnd_next = self.rng as f64 / u32::MAX as f64 * 2.0 - 1.0;
            }
            let ph = self.vib_phase;
            let shape = match self.p.vib_shape {
                1 => 2.0 / PI * ph.sin().asin(),                // triangle
                2 => (ph / PI - 1.0) * 0.9,                      // saw
                3 => {
                    let t = ph / (2.0 * PI);
                    self.rnd_prev + (self.rnd_next - self.rnd_prev) * t * t * (3.0 - 2.0 * t) // wander
                }
                _ => ph.sin(),
            };
            let ramp = ((self.note_age - self.p.vib_delay_ms as f64 / 1000.0) / 0.3).clamp(0.0, 1.0);
            vib = self.p.vib_depth_cents as f64 / 100.0 * ramp * shape;
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

    /// Interval (in semitones) from `base` to the allowed note nearest to base + iv.
    fn snap_interval(&self, base: i32, iv: f32) -> f64 {
        let k = iv.round() as i32;
        let sg = if iv < 0.0 { -1 } else { 1 };
        for d in [0, -sg, sg, -2 * sg, 2 * sg] {
            if self.is_allowed(base + k + d) {
                return (k + d) as f64;
            }
        }
        iv as f64
    }

    /// Classic time-domain grain: read the source at rate `s` (moves formants with the resampling).
    #[allow(clippy::too_many_arguments)]
    fn add_classic(&mut self, nch: usize, m: i64, f: f64, hg: i64, kmin: i64, oi: i64, scale: f64, s: f64, gain: [f32; MAX_CH], dly: i64) {
        for k in kmin..hg {
            let u = k as f64 - f;
            let wv = (scale * 0.5 * (1.0 + (PI * u / hg as f64).cos())) as f32;
            let sp = m as f64 + u * s;
            let idx = ((oi + k + dly) & self.ring_mask) as usize;
            for ch in 0..nch {
                let v = self.read_cubic(ch, sp);
                self.ola[ch][idx] += wv * v * gain[ch];
            }
        }
    }

    fn place_grain(&mut self, vi: usize, o: i64, nch: usize) {
        let mut sy = self.syn[vi];
        // choose the analysis centre: the nearest pitch mark, unless marks are too sparse here
        // (e.g. just after an unvoiced stretch) - then use the wanted position itself.
        let (_, p0) = self.query(sy.content_nom);
        let mut m = self.pick_mark(sy.content_nom);
        if (m as f64 - sy.content_nom).abs() > 0.6 * p0 {
            m = sy.content_nom.round() as i64;
        }
        let (voiced, pq) = self.query(m as f64);
        let p = if voiced { pq } else { self.fallback() };
        let dt = sy.prev_hs / self.fs;

        let mut gain = [1.0f32; MAX_CH];
        let mut dly = 0i64;
        let mut vformant = 0.0f32;
        let r;
        if vi == 0 {
            r = self.compute_ratio(dt, voiced, p);
            self.cur_r = r;
            for v in 0..NUM_VOICES {
                let vp = self.p.voices[v];
                let target = if vp.snap && self.have_note && voiced { self.snap_interval(self.cur_note, vp.interval) } else { vp.interval as f64 };
                let tau = (self.p.transition_ms as f64 / 1000.0).max(0.01);
                self.vdelta[v] += (target - self.vdelta[v]) * (1.0 - (-dt / tau).exp());
            }
        } else {
            let vp = self.p.voices[vi - 1];
            r = if voiced { self.cur_r * 2f64.powf((self.vdelta[vi - 1] + vp.detune_cents as f64 / 100.0) / 12.0) } else { 1.0 };
            let lv = 10f32.powf(vp.level_db / 20.0);
            if nch == 2 {
                let a = (vp.pan.clamp(-1.0, 1.0) + 1.0) * 0.25 * std::f32::consts::PI;
                gain = [lv * a.cos() * std::f32::consts::SQRT_2, lv * a.sin() * std::f32::consts::SQRT_2];
            } else {
                gain = [lv, lv];
            }
            dly = (vp.delay_ms as f64 * self.fs / 1000.0) as i64;
            vformant = vp.formant_semis;
        }

        // Output hop = the real distance to the next pitch mark (not the detector's period
        // estimate) divided by the ratio: with no shift the grains then land exactly where they
        // came from and the voice is rebuilt unchanged. With the estimate, grains drifted against
        // the original, overlapped out of phase and periods were dropped / repeated even at
        // ratio 1 - a rough, not-quite-live sound.
        let d = if voiced {
            self.mark_after(m).map(|n| (n - m) as f64).filter(|d| *d > 0.6 * p && *d < 1.6 * p).unwrap_or(p)
        } else {
            p
        };
        let hs = d / r;
        let hg = (p.max(hs).ceil() as i64).max(8);
        let follow = if self.p.formant_preserve { 0.0 } else { self.p.formant_follow as f64 };
        let s_raw = if self.p.formant_on { 2f64.powf((self.p.formant_semis + vformant) as f64 / 12.0) * r.powf(follow) } else { 1.0 };
        let mode = self.p.formant_mode;
        let spectral = mode != FormantMode::Classic && s_raw.ln().abs() > 0.002 && (2 * hg) as usize <= self.gbuf[0].len();
        let s_cl = if mode == FormantMode::Classic { s_raw.clamp(0.5, 2.0) } else { 1.0 };

        let tc = sy.next_centre;
        if vi == 0 {
            self.dbg_lag = (tc - m as f64) - self.latency as f64;
        }
        let oi = tc.floor() as i64;
        let f = tc - oi as f64;
        let scale = hs / hg as f64;

        let mut kmin = -hg;
        let clipped = oi + kmin < o;
        if clipped {
            kmin = o - oi;
        }
        if self.debug && vi == 0 {
            eprintln!("o={:.4}s tc={:.0} m={} voiced={} p={:.0} r={:.3} hs={:.0} hg={} lag={:.0} clipped={} (lost {} samp)", o as f64 / self.fs, tc, m, voiced, p, r, hs, hg, self.dbg_lag, clipped, if clipped { o - (oi - hg) } else { 0 });
        }

        if vi > 0 && !voiced {
            // extra voices stay silent on consonants and breaths
        } else if !spectral {
            self.add_classic(nch, m, f, hg, kmin, oi, scale, s_cl, gain, dly);
        } else {
            let l = (2 * hg) as usize;
            let s = s_raw.clamp(0.4, 2.5);
            let tilt = self.p.formant_tilt as f64;
            for ch in 0..nch {
                for j in 0..l {
                    let u = (j as i64 - hg) as f64 - f;
                    let wv = (scale * 0.5 * (1.0 + (PI * u / hg as f64).cos())) as f32;
                    self.gbuf[ch][j] = wv * self.read_cubic(ch, m as f64 + u);
                }
            }
            let ok = {
                let (g0, g1) = self.gbuf.split_at_mut(1);
                let right = if nch == 2 { Some(&mut g1[0][..l]) } else { None };
                self.shifter.process2(&mut g0[0][..l], right, p, self.fs, mode, s, tilt)
            };
            if ok {
                for k in kmin..hg {
                    let idx = ((oi + k + dly) & self.ring_mask) as usize;
                    for ch in 0..nch {
                        self.ola[ch][idx] += self.gbuf[ch][(k + hg) as usize] * gain[ch];
                    }
                }
            } else {
                self.add_classic(nch, m, f, hg, kmin, oi, scale, s.clamp(0.5, 2.0), gain, dly);
            }
        }

        sy.next_centre = tc + hs;
        // continue from the chosen mark; slowly pull back towards the nominal latency
        // Normally continue from the chosen mark. When the grains are much denser than the
        // pitch marks (strong upward shift, hs < p), the nearest mark would be picked again and
        // again and the content would fall behind real time - so then advance the nominal
        // position by exactly one hop and let the mark picker repeat marks as needed.
        let nom = if hs < 0.75 * d { sy.content_nom + hs } else { m as f64 + hs };
        let lag_err = (sy.next_centre - self.latency as f64) - nom;
        sy.content_nom = nom + (self.pull * lag_err).clamp(-0.5 * hs, 0.5 * hs);
        sy.prev_hs = hs;
        sy.next_decision = sy.next_centre - (1.3 * p.max(hs).max(self.fallback()) + 4.0);
        if sy.next_decision < o as f64 - hs {
            sy.next_centre = o as f64 + hs;
            sy.next_decision = o as f64;
            sy.content_nom = sy.next_centre - self.latency as f64;
        }
        self.syn[vi] = sy;
    }
}

#[cfg(test)]
mod fuzz_tests {
    use super::*;

    fn xorshift(s: &mut u32) -> f32 {
        *s ^= *s << 13;
        *s ^= *s >> 17;
        *s ^= *s << 5;
        *s as f32 / u32::MAX as f32
    }

    fn voice_signal(fs: f64, secs: f64, seed: &mut u32) -> Vec<f32> {
        let n = (fs * secs) as usize;
        let mut ph = 0.0f64;
        let mut f0 = 150.0f64;
        let mut out = Vec::with_capacity(n);
        let (mut y1, mut y2) = (0.0f64, 0.0f64);
        for i in 0..n {
            if i % (fs as usize / 3) == 0 {
                f0 = 90.0 * 2f64.powf(xorshift(seed) as f64 * 2.5);
            }
            ph += f0 / fs;
            let src = if ph % 1.0 < 0.05 { 1.0 } else { -0.05 } + 0.02 * (xorshift(seed) as f64 - 0.5);
            let th = 2.0 * PI * 700.0 / fs;
            let r = 0.985;
            let y = src * 0.1 + 2.0 * r * th.cos() * y1 - r * r * y2;
            y2 = y1;
            y1 = y;
            // silence gaps, now and then
            let gate = if (i / (fs as usize / 5)) % 7 == 6 { 0.0 } else { 1.0 };
            out.push((y * 0.3 * gate) as f32);
        }
        out
    }

    /// A voice far above the lead (hs << pitch period) must stay locked to real time and clean,
    /// and the sum of loud voices must not clip.
    #[test]
    fn high_voice_stays_in_time_and_sum_does_not_clip() {
        let fs = 48000.0f64;
        let n = (fs * 3.0) as usize;
        let f0 = 220.0f64;
        // simple vowel-like pulse train, 2 resonances
        let (mut y1, mut y2, mut z1, mut z2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let ph = (i as f64 * f0 / fs) % 1.0;
                let src = if ph < 0.05 { 1.0 } else { -0.05 };
                let r = 0.985;
                let th = 2.0 * PI * 700.0 / fs;
                let y = src * 0.1 + 2.0 * r * th.cos() * y1 - r * r * y2;
                y2 = y1;
                y1 = y;
                let th2 = 2.0 * PI * 1500.0 / fs;
                let z = y * 0.1 + 2.0 * r * th2.cos() * z1 - r * r * z2;
                z2 = z1;
                z1 = z;
                (z * 3.0) as f32
            })
            .collect();
        let peak_in = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let x: Vec<f32> = x.iter().map(|v| v * 0.5 / peak_in).collect();
        let run = |iv: f32, lvl: f32, n_on: usize| -> Vec<f32> {
            let mut e = Engine::new(fs, 70.0);
            let mut p = Params::default();
            p.scale = 0; // chromatic
            p.retune_ms = 50.0;
            for k in 0..n_on {
                p.voices[k] = VoiceParams { on: true, interval: iv, level_db: lvl, ..VoiceParams::default() };
            }
            let mut out = Vec::with_capacity(n);
            for ch in x.chunks(128) {
                e.set_params(&p);
                for &s in ch {
                    out.push(e.process_frame([s, s], 1)[0]);
                }
            }
            out
        };
        // voice-only signal = with voice minus without
        let base = run(0.0, -9.0, 0);
        let hi = run(24.0, 0.0, 1);
        let v: Vec<f32> = hi.iter().zip(&base).skip(fs as usize).map(|(a, b)| a - b).collect();
        let fv = f0 * 4.0;
        let (mut harm, mut tot) = (0.0f64, 0.0f64);
        for k in 1..=18 {
            // Goertzel-ish single-bin DFT energy around k*fv and in between
            for (fr, is_h) in [(k as f64 * fv, true), (k as f64 * fv + fv / 2.0, false)] {
                if fr > 8000.0 {
                    continue;
                }
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, s) in v.iter().enumerate() {
                    let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / v.len() as f64).cos();
                    let a = 2.0 * PI * fr * i as f64 / fs;
                    re += *s as f64 * w * a.cos();
                    im += *s as f64 * w * a.sin();
                }
                let e = re * re + im * im;
                tot += e;
                if is_h {
                    harm += e;
                }
            }
        }
        assert!(harm / tot > 0.9, "+24 st voice is not harmonic: {:.2}", harm / tot);
        // two loud voices: no hard clipping
        let loud = run(12.0, 6.0, 2);
        let pk = loud.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(pk < 1.0, "peak {pk}");
    }


    /// Quality check on a vowel-like voice 40 cents sharp: the output must sit on the note (within 3 cents),
    /// keep its level (+-1.5 dB) and stay harmonic (little non-harmonic energy = no grit / warble).
    #[test]
    fn corrected_voice_is_in_tune_loud_enough_and_clean() {
        let fs = 48000.0f64;
        let n = (fs * 3.0) as usize;
        let f_in = 220.0 * 2f64.powf(40.0 / 1200.0);
        let (mut y1, mut y2) = (0.0f64, 0.0f64);
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let ph = (i as f64 * f_in / fs) % 1.0;
                let src = if ph < 0.05 { 1.0 } else { -0.05 };
                let r = 0.985;
                let th = 2.0 * PI * 800.0 / fs;
                let y = src * 0.1 + 2.0 * r * th.cos() * y1 - r * r * y2;
                y2 = y1;
                y1 = y;
                (y * 2.0) as f32
            })
            .collect();
        let pk = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let x: Vec<f32> = x.iter().map(|v| v * 0.4 / pk).collect();
        let mut e = Engine::new(fs, 70.0);
        let mut p = Params::default();
        p.scale = 0;
        p.retune_ms = 0.0;
        p.amount = 1.0;
        p.mix = 1.0;
        let mut out = Vec::with_capacity(n);
        for ch in x.chunks(128) {
            e.set_params(&p);
            for &s in ch {
                out.push(e.process_frame([s, s], 1)[0]);
            }
        }
        let seg = &out[fs as usize..];
        let rms = |v: &[f32]| (v.iter().map(|a| (*a as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let db = 20.0 * (rms(seg) / rms(&x[fs as usize..])).log10();
        assert!(db.abs() < 1.5, "level change {db:.2} dB");
        // fundamental by scanning DFT around 220 Hz in 0.5 cent steps
        let dft = |fr: f64| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, s) in seg.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / seg.len() as f64).cos();
                let a = 2.0 * PI * fr * i as f64 / fs;
                re += *s as f64 * w * a.cos();
                im += *s as f64 * w * a.sin();
            }
            re * re + im * im
        };
        let (mut best, mut bf) = (0.0f64, 0.0f64);
        for c in -300..=300 {
            let fr = 220.0 * 2f64.powf(c as f64 * 0.1 / 1200.0);
            let en = dft(fr);
            if en > best {
                best = en;
                bf = fr;
            }
        }
        let cents = 1200.0 * (bf / 220.0).log2();
        assert!(cents.abs() < 3.0, "off by {cents:.2} cents (f {bf:.2})");
        let (mut harm, mut inter) = (0.0f64, 0.0f64);
        for k in 1..=15 {
            harm += dft(220.0 * k as f64);
            inter += dft(220.0 * (k as f64 + 0.5));
        }
        let ratio = 10.0 * (harm / inter.max(1e-30)).log10();
        assert!(ratio > 25.0, "harmonic / in-between = {ratio:.1} dB");
    }

    #[test]
    fn random_parameters_never_blow_up() {
        let mut seed = 0xC0FFEEu32;
        for &fs in &[44100.0f64, 48000.0, 96000.0] {
            for nch in 1..=2usize {
                let x = voice_signal(fs, 6.0, &mut seed);
                let mut e = Engine::new(fs, 70.0);
                let mut p = Params::default();
                let mut peak = 0.0f32;
                for (bi, chunk) in x.chunks(257).enumerate() {
                    if bi % 4 == 0 {
                        p.scale = (xorshift(&mut seed) * 17.0) as usize % 17;
                        p.key = (xorshift(&mut seed) * 12.0) as usize % 12;
                        p.retune_ms = xorshift(&mut seed) * 400.0;
                        p.transition_ms = xorshift(&mut seed) * 400.0;
                        p.amount = xorshift(&mut seed);
                        p.tol_cents = xorshift(&mut seed) * 40.0;
                        p.tol_ms = xorshift(&mut seed) * 300.0;
                        p.keep_vibrato = xorshift(&mut seed) > 0.5;
                        p.vib_amount = xorshift(&mut seed) * 2.0;
                        p.range_lo = xorshift(&mut seed) * 60.0 + 24.0;
                        p.range_hi = p.range_lo + xorshift(&mut seed) * 50.0;
                        p.transpose = (xorshift(&mut seed) * 24.0 - 12.0).round();
                        p.fine_cents = xorshift(&mut seed) * 200.0 - 100.0;
                        p.formant_semis = xorshift(&mut seed) * 24.0 - 12.0;
                        p.formant_preserve = xorshift(&mut seed) > 0.5;
                        p.formant_mode = FormantMode::from_index((xorshift(&mut seed) * 5.0) as usize % 5);
                        p.formant_tilt = xorshift(&mut seed) * 2.0 - 1.0;
                        p.formant_follow = xorshift(&mut seed);
                        for n in p.note_override.iter_mut() {
                            *n = (xorshift(&mut seed) * 6.0) as u8 % 6;
                        }
                        p.vib_on = xorshift(&mut seed) > 0.5;
                        p.vib_shape = (xorshift(&mut seed) * 4.0) as usize % 4;
                        p.air_db = xorshift(&mut seed) * 18.0 - 6.0;
                        p.drive = xorshift(&mut seed);
                        p.bypass = xorshift(&mut seed) > 0.9;
                        p.mix = xorshift(&mut seed);
                        for v in p.voices.iter_mut() {
                            v.on = xorshift(&mut seed) > 0.4;
                            v.interval = (xorshift(&mut seed) * 48.0 - 24.0).round();
                            v.snap = xorshift(&mut seed) > 0.5;
                            v.level_db = xorshift(&mut seed) * 36.0 - 30.0;
                            v.pan = xorshift(&mut seed) * 2.0 - 1.0;
                            v.detune_cents = xorshift(&mut seed) * 100.0 - 50.0;
                            v.formant_semis = xorshift(&mut seed) * 24.0 - 12.0;
                            v.delay_ms = xorshift(&mut seed) * 60.0;
                        }
                    }
                    e.set_params(&p);
                    for &s in chunk {
                        let o = e.process_frame([s, s * 0.7], nch);
                        for c in 0..nch {
                            assert!(o[c].is_finite(), "non-finite output (fs {fs}, nch {nch})");
                            peak = peak.max(o[c].abs());
                        }
                    }
                }
                let inpk = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
                assert!(peak < 8.0 * inpk.max(0.1), "peak {peak} input peak {inpk} (fs {fs}, nch {nch})");
            }
        }
    }


    /// Vowel-like test voice following a pitch curve (MIDI note per sample).
    fn sung(fs: f64, midi: impl Fn(f64) -> f64, secs: f64) -> Vec<f32> {
        let n = (fs * secs) as usize;
        let (mut ph, mut y1, mut y2) = (0.0f64, 0.0f64, 0.0f64);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f64 / fs;
            let f0 = 440.0 * 2f64.powf((midi(t) - 69.0) / 12.0);
            ph = (ph + f0 / fs) % 1.0;
            let src = if ph < 0.05 { 1.0 } else { -0.05 };
            let r = 0.985;
            let th = 2.0 * PI * 750.0 / fs;
            let y = src * 0.1 + 2.0 * r * th.cos() * y1 - r * r * y2;
            y2 = y1;
            y1 = y;
            out.push((y * 2.0) as f32);
        }
        let pk = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        out.iter().map(|v| v * 0.4 / pk).collect()
    }

    /// Short-time pitch (MIDI) of `x` around sample `c` by normalised autocorrelation, parabolic peak.
    fn pitch_at(x: &[f32], c: usize, fs: f64) -> f64 {
        pitch_in(x, c, fs, 120.0, 700.0)
    }

    fn pitch_in(x: &[f32], c: usize, fs: f64, f_lo: f64, f_hi: f64) -> f64 {
        let w = 2048usize;
        let a = c - w / 2;
        let (lo, hi) = ((fs / f_hi) as usize, (fs / f_lo) as usize);
        let ac = |lag: usize| -> f64 {
            let (mut s, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
            for i in a..a + w {
                s += x[i] as f64 * x[i + lag] as f64;
                e0 += (x[i] as f64).powi(2);
                e1 += (x[i + lag] as f64).powi(2);
            }
            s / (e0 * e1).sqrt().max(1e-12)
        };
        let vals: Vec<f64> = (lo..=hi + 1).map(ac).collect();
        let mut best = 1;
        for k in 1..vals.len() - 1 {
            if vals[k] > vals[best] {
                best = k;
            }
        }
        // prefer the shortest lag that is nearly as good (no octave-down errors)
        for k in 1..vals.len() - 1 {
            if vals[k] > 0.9 * vals[best] && vals[k] >= vals[k - 1] && vals[k] >= vals[k + 1] {
                best = k;
                break;
            }
        }
        let (y0, y1, y2) = (vals[best - 1], vals[best], vals[best + 1]);
        let den = y0 - 2.0 * y1 + y2;
        let sh = if den.abs() > 1e-12 { (0.5 * (y0 - y2) / den).clamp(-1.0, 1.0) } else { 0.0 };
        let lag = (lo + best) as f64 + sh;
        69.0 + 12.0 * (fs / lag / 440.0).log2()
    }

    fn run_engine(fs: f64, x: &[f32], p: &Params) -> (Vec<f32>, usize) {
        let mut e = Engine::new(fs, 70.0);
        let lat = e.latency_samples() as usize;
        let mut out = Vec::with_capacity(x.len());
        for ch in x.chunks(128) {
            e.set_params(p);
            for &s in ch {
                out.push(e.process_frame([s, s], 1)[0]);
            }
        }
        (out, lat)
    }

    /// Vibrato handling: with Vibrato Off and Speed 0 a wobbly voice comes out dead flat on the note
    /// and a note change is an instant step; with Vibrato On the note centre is corrected and the
    /// singer's vibrato is kept (Depth 100 %), flattened (0 %) or widened (200 %).
    #[test]
    fn vibrato_is_kept_or_flattened() {
        let fs = 48000.0;
        // A3 + 35 c off, 40 c vibrato + slow 15 c drift; at 1.5 s a jump to C4 with the same wobble
        let vib = |t: f64| 0.40 * (2.0 * PI * 5.5 * t).sin();
        let curve = move |t: f64| {
            let base = if t < 1.5 { 57.35 } else { 60.3 };
            base + vib(t) + 0.15 * (2.0 * PI * 0.7 * t).sin()
        };
        let x = sung(fs, curve, 3.0);
        // deviation of the output from note + k * vibrato
        let dev = |out: &[f32], lat: usize, from: f64, to: f64, note: f64, k: f64| -> (f64, f64) {
            let (mut max, mut sum, mut n) = (0.0f64, 0.0f64, 0);
            let mut t = from;
            while t < to {
                let m = pitch_at(out, (t * fs) as usize + lat, fs);
                let d = (m - note - k * vib(t)).abs();
                max = max.max(d);
                sum += d;
                n += 1;
                t += 0.01;
            }
            (max * 100.0, sum / n as f64 * 100.0)
        };
        let mut p = Params::default();
        p.scale = 0;
        // hard: vibrato off, speed 0
        p.keep_vibrato = false;
        p.retune_ms = 0.0;
        p.transition_ms = 0.0;
        let (h, lat) = run_engine(fs, &x, &p);
        let (hmax, havg) = dev(&h, lat, 0.4, 1.4, 57.0, 0.0);
        let (hmax2, havg2) = dev(&h, lat, 1.6, 2.9, 60.0, 0.0);
        // the jump lands on a vibrato peak (~+0.8 st): give the centre a moment
        let m = pitch_at(&h, ((1.5 + 0.2) * fs) as usize + lat, fs);
        println!("Vibrato Off, Speed 0: flat within max {hmax:.1} c avg {havg:.1} c, after the step {hmax2:.1} / {havg2:.1} c, 200 ms after the jump {m:.2}");
        assert!(hmax < 12.0 && havg < 4.0 && hmax2 < 12.0 && havg2 < 4.0, "hard mode not flat");
        assert!((m - 60.0).abs() < 0.15, "step too slow: {m:.2}");
        // natural: vibrato on, default speed / transition; depth 100 / 0 / 200 %
        p = Params::default();
        p.scale = 0;
        // (Depth 0 at the default Speed of 15 ms: the correction trails a 5.5 Hz vibrato a little)
        for (depth, limit) in [(1.0f32, 9.0), (0.0, 14.0), (2.0, 20.0)] {
            p.vib_amount = depth;
            let (y, lat) = run_engine(fs, &x, &p);
            let (mx, avg) = dev(&y, lat, 0.5, 1.4, 57.0, depth as f64);
            let (mx2, avg2) = dev(&y, lat, 1.8, 2.9, 60.0, depth as f64);
            println!("Vibrato On, depth {:.0} %: output = note + {:.0} % vibrato within avg {avg:.1} c (max {mx:.1}), after the step {avg2:.1} c (max {mx2:.1})", depth * 100.0, depth * 100.0);
            assert!(avg < limit && avg2 < limit, "vibrato depth {depth}: avg error {avg:.1} / {avg2:.1} c");
        }
    }

    /// The voice-range switch must not cap high notes: a "Bass" setting still tunes A4 and D5.
    #[test]
    fn range_does_not_cap_high_notes() {
        let fs = 48000.0;
        for (note, label) in [(69.0f64, "A4"), (74.0, "D5")] {
            let x = sung(fs, |_| note + 0.3, 1.5);
            let mut res = Vec::new();
            for max_hz in [340.0f32, 1100.0] {
                let mut p = Params::default();
                p.scale = 0;
                p.retune_ms = 0.0;
                p.min_hz = 60.0;
                p.max_hz = max_hz;
                let mut e = Engine::new(fs, 60.0);
                let lat = e.latency_samples() as usize;
                let mut out = Vec::with_capacity(x.len());
                for ch in x.chunks(128) {
                    e.set_params(&p);
                    for &v in ch {
                        out.push(e.process_frame([v, v], 1)[0]);
                    }
                }
                let mut worst = 0.0f64;
                let mut t = 0.5;
                while t < 1.3 {
                    let m = pitch_in(&out, (t * fs) as usize + lat, fs, 150.0, 1400.0);
                    worst = worst.max((m - note).abs());
                    t += 0.05;
                }
                res.push(worst * 100.0);
            }
            println!("Bass range, {label}: old cap 340 Hz -> worst {:.0} c, now -> worst {:.1} c", res[0], res[1]);
            assert!(res[1] < 15.0, "{label} not tuned with the Bass range: {:.1} c", res[1]);
        }
    }


    /// Gliding up through the old top of the Bass range (F4) must not make the chosen note jump an octave.
    #[test]
    fn bass_range_glide_through_the_top() {
        let fs = 48000.0;
        let x = sung(fs, |t| 60.0 + 12.0 * (t / 3.0).min(1.0) + 0.3, 3.5);
        for max_hz in [340.0f32, 1100.0] {
            let mut p = Params::default();
            p.scale = 0;
            p.min_hz = 60.0;
            p.max_hz = max_hz;
            let mut e = Engine::new(fs, 60.0);
            let (mut last, mut octave_jumps, mut wrong) = (0.0f32, 0, 0);
            for (i, ch) in x.chunks(128).enumerate() {
                e.set_params(&p);
                for &v in ch {
                    e.process_frame([v, v], 1);
                }
                let t = i as f64 * 128.0 / fs;
                let tg = e.meters.target;
                if t > 0.3 && tg > 0.0 {
                    if last > 0.0 && (tg - last).abs() > 6.0 {
                        octave_jumps += 1;
                    }
                    let sung_now = 60.0 + 12.0 * ((t - 0.05) / 3.0).min(1.0) + 0.3;
                    if (tg as f64 - sung_now).abs() > 1.5 {
                        wrong += 1;
                    }
                    last = tg;
                }
            }
            println!("Bass range, glide C4 -> C5, search up to {max_hz} Hz: octave jumps {octave_jumps}, blocks on a wrong note {wrong}");
            if max_hz > 1000.0 {
                assert_eq!(octave_jumps, 0);
                assert!(wrong < 10, "{wrong}");
            }
        }
    }

    /// With the wider search a real bass note must still be found (no octave-up errors).
    #[test]
    fn low_bass_still_tracks_with_wide_search() {
        let fs = 48000.0;
        for note in [40.0f64, 45.0, 50.0] {
            let x = sung(fs, |t| note + 0.3 + 0.2 * (2.0 * PI * 5.0 * t).sin(), 1.5);
            let mut p = Params::default();
            p.scale = 0;
            p.retune_ms = 0.0;
            p.min_hz = 60.0;
            p.max_hz = 1100.0;
            let mut e = Engine::new(fs, 60.0);
            let lat = e.latency_samples() as usize;
            let mut out = Vec::with_capacity(x.len());
            let mut wrong = 0;
            for (i, ch) in x.chunks(128).enumerate() {
                e.set_params(&p);
                for &v in ch {
                    out.push(e.process_frame([v, v], 1)[0]);
                }
                let tg = e.meters.target;
                if i as f64 * 128.0 / fs > 0.3 && tg > 0.0 && (tg as f64 - note).abs() > 0.6 {
                    wrong += 1;
                }
            }
            let m = pitch_in(&out, (1.0 * fs) as usize + lat, fs, 50.0, 400.0);
            println!("bass note {note}: target wrong in {wrong} blocks, output {m:.2}");
            assert_eq!(wrong, 0, "note {note}: octave error");
            assert!((m - note).abs() < 0.15, "note {note}: output {m:.2}");
        }
    }

    /// A voice wobbling right between two notes must not flicker back and forth between them.
    #[test]
    fn note_choice_does_not_flicker() {
        let fs = 48000.0;
        let mut seed = 5u32;
        let jit: Vec<f64> = (0..400).map(|_| (xorshift(&mut seed) as f64 - 0.5) * 0.22).collect();
        // sits at 57.5 (between A3 and A#3) with fast random jitter of +-11 cents
        let x = sung(fs, |t| 57.5 + jit[((t * 120.0) as usize).min(399)], 3.0);
        for (keep, tol_c, tol_ms, limit) in [(false, 0.0f32, 0.0f32, 12usize), (false, 20.0, 0.0, 3), (false, 0.0, 100.0, 3), (true, 20.0, 0.0, 3), (true, 0.0, 100.0, 3)] {
            let mut e = Engine::new(fs, 70.0);
            let mut p = Params::default();
            p.scale = 0;
            p.keep_vibrato = keep;
            p.tol_cents = tol_c;
            p.tol_ms = tol_ms;
            let mut switches = 0;
            let mut last = 0.0f32;
            for (i, ch) in x.chunks(128).enumerate() {
                e.set_params(&p);
                for &s in ch {
                    e.process_frame([s, s], 1);
                }
                let t = e.meters.target;
                if i as f64 * 128.0 / fs > 0.3 && t > 0.0 {
                    if last > 0.0 && t != last {
                        switches += 1;
                    }
                    last = t;
                }
            }
            println!("vibrato {keep} tolerance {tol_c} c / {tol_ms} ms: {switches} note switches in 2.7 s");
            // without any tolerance the target follows the nearest note (a voice right on the
            // border may switch); any tolerance must hold it
            assert!(switches <= limit, "flicker: {switches} switches (vibrato {keep}, tolerance {tol_c} c / {tol_ms} ms)");
        }
    }

    /// A held note sung a bit off (25-40 cents) right after a step must still be corrected to the
    /// nearest note of the scale, at every Tracking setting: Tracking may delay a switch, never
    /// keep the voice on the previous note for the whole held note.
    #[test]
    fn held_notes_land_on_the_nearest_note() {
        let fs = 48000.0;
        // (sung pitch, expected note), 0.6 s each, light 12 cent vibrato
        let seq: [(f64, i32); 7] = [(57.0, 57), (57.65, 58), (60.3, 60), (59.3, 59), (61.35, 61), (60.62, 61), (59.4, 59)];
        let x = sung(fs, |t| {
            let i = ((t / 0.6) as usize).min(seq.len() - 1);
            seq[i].0 + 0.12 * (2.0 * PI * 5.5 * t).sin()
        }, 0.6 * seq.len() as f64);
        for tol_ms in [0.0f32, 50.0, 150.0] {
            for keep in [false, true] {
                let mut e = Engine::new(fs, 70.0);
                let mut p = Params::default();
                p.scale = 0;
                p.tol_ms = tol_ms;
                p.keep_vibrato = keep;
                let mut wrong = Vec::new();
                for (i, ch) in x.chunks(128).enumerate() {
                    e.set_params(&p);
                    for &s in ch {
                        e.process_frame([s, s], 1);
                    }
                    let t = i as f64 * 128.0 / fs;
                    let k = (t / 0.6) as usize;
                    // judge the end of every held note (Tolerance Time delays the switch on purpose)
                    if k < seq.len() && t - k as f64 * 0.6 > 0.3 + tol_ms as f64 / 1000.0 * 0.6 {
                        let tg = e.meters.target as i32;
                        if tg != seq[k].1 {
                            wrong.push((k, tg));
                        }
                    }
                }
                wrong.dedup();
                println!("tolerance time {tol_ms} ms vibrato {keep}: wrong {:?}", wrong);
                assert!(wrong.is_empty(), "tolerance {tol_ms} ms vibrato {keep}: held notes on the wrong note {:?}", wrong);
            }
        }
    }

    /// Realistic note transitions, many of them: steps of 1..12 semitones up and down, sung as an
    /// instant jump, a 40 ms or 120 ms glide or a scoop from below, each note a little off pitch
    /// with vibrato, chromatic and B natural minor, at several Tracking / ULTRA / Retune settings.
    /// For every transition: no note outside the A..B span is ever chosen (no octave errors, no
    /// overshoot), an instant jump goes straight to the new note (no stairs), the new note is
    /// reached quickly and then held without flicker, and with Retune 0 the output really sits on it.
    #[test]
    fn transitions_are_clean() {
        let fs = 48000.0;
        let dur = 0.4;
        let smooth = |x: f64| { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) };
        let mut report = Vec::new();
        for (seed0, scale, key) in [(1234u32, 0usize, 0usize), (1235, 2, 11), (4242, 0, 0), (4243, 2, 11), (31337, 9, 4)] {
            let allowed: Vec<i32> = (50..=72).filter(|m| (allowed_mask(scale, key, 0) >> (m % 12)) & 1 == 1).collect();
            // random walk over the allowed notes with every interval size represented
            let mut seed = 31337u32 + scale as u32;
            let mut notes = vec![allowed[allowed.len() / 2]];
            for k in 0..60 {
                let a = *notes.last().unwrap();
                let want = 1 + (k % 12) as i32;
                let up = xorshift(&mut seed) > 0.5;
                let cands: Vec<i32> = allowed.iter().copied().filter(|&m| m != a && ((m - a).abs() - want).abs() <= 1).collect();
                let pick = cands.iter().copied().filter(|&m| (m > a) == up).next().or(cands.first().copied()).unwrap_or(allowed[0]);
                notes.push(pick);
            }
            let offs: Vec<f64> = (0..notes.len()).map(|_| (xorshift(&mut seed) as f64 - 0.5) * 0.56).collect();
            let kind = |k: usize| k % 4; // 0 jump, 1 glide 40 ms, 2 glide 120 ms, 3 scoop
            let glide = |k: usize| [0.0, 0.04, 0.12, 0.09][kind(k)];
            let curve = |t: f64| -> f64 {
                let k = ((t / dur) as usize).min(notes.len() - 1);
                let tau = t - k as f64 * dur;
                let b = notes[k] as f64 + offs[k];
                let a = if k == 0 { b } else { notes[k - 1] as f64 + offs[k - 1] };
                let base = match kind(k) {
                    0 => b,
                    3 => if k == 0 { b } else { b - 0.6 + 0.6 * smooth(tau / 0.09) },
                    _ => a + (b - a) * smooth(tau / glide(k)),
                };
                base + 0.2 * smooth((tau - 0.12) / 0.08) * (2.0 * PI * 5.5 * tau).sin()
            };
            let x = sung(fs, curve, dur * notes.len() as f64);
            for (tol_ms, keep, retune) in [(0.0f32, false, 0.0f32), (0.0, false, 15.0), (0.0, true, 15.0), (0.0, true, 0.0), (60.0, false, 0.0), (120.0, true, 15.0)] {
                let mut e = Engine::new(fs, 70.0);
                let lat = e.latency_samples() as usize;
                let mut p = Params::default();
                p.scale = scale;
                p.key = key;
                p.tol_ms = tol_ms;
                p.keep_vibrato = keep;
                p.retune_ms = retune;
                p.transition_ms = retune;
                let mut out = Vec::with_capacity(x.len());
                let mut tg: Vec<(f64, i32)> = Vec::new();
                let mut hzs: Vec<f32> = Vec::new();
                // TRACE=1: per-block note-choice state for unsettled notes
                let mut dbg: Vec<(f64, i32, f64)> = Vec::new();

                for (i, ch) in x.chunks(64).enumerate() {
                    e.set_params(&p);
                    for &s in ch {
                        out.push(e.process_frame([s, s], 1)[0]);
                    }
                    tg.push((i as f64 * 64.0 / fs, e.meters.target as i32));
                    hzs.push(if e.meters.voiced { e.meters.hz } else { 0.0 });
                    dbg.push((e.note_sm, e.cand_note, e.cand_age));
                }
                let (mut bad_span, mut stairs, mut unsettled, mut off_out) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                let mut worst_settle = 0.0f64;
                for k in 1..notes.len() {
                    let (a, b) = (notes[k - 1], notes[k]);
                    let t0 = k as f64 * dur;
                    let arrive = t0 + glide(k);
                    let w: Vec<(f64, i32)> = tg.iter().copied().filter(|(t, _)| *t >= t0 && *t < t0 + dur).collect();
                    let lo = a.min(b) - if kind(k) == 3 { 1 } else { 0 };
                    let hi = a.max(b);
                    // a flash shorter than 20 ms while the detector itself is still mid-leap is not
                    // counted (the correction then follows the reading, the output is the dry voice)
                    let outside: Vec<&(f64, i32)> = w.iter().filter(|(_, m)| *m != 0 && (*m < lo || *m > hi)).collect();
                    if outside.len() as f64 * 64.0 / fs > 0.02 {
                        let &(t, m) = outside[0];
                        bad_span.push((k, a, b, m, ((t - t0) * 1000.0) as i32));
                    }
                    if kind(k) == 0 {
                        if let Some(&(_, m)) = w.iter().find(|(_, m)| *m != 0 && *m != a && *m != b) {
                            stairs.push((k, a, b, m));
                        }
                    }
                    // settled: from here to the end of the note the target is b
                    let settle = w.iter().rposition(|(_, m)| *m != b).map(|i| w.get(i + 1).map(|v| v.0)).unwrap_or(Some(t0));
                    match settle {
                        Some(ts) if ts < t0 + dur - 0.1 => worst_settle = worst_settle.max(ts - arrive),
                        _ => {
                            unsettled.push((k, a, b));
                            if std::env::var("TRACE").is_ok() {
                                for (i, (t, m)) in tg.iter().enumerate() {
                                    if *t >= t0 - 0.05 && *t < t0 + dur && i % 2 == 0 {
                                        let d = if hzs[i] > 0.0 { 69.0 + 12.0 * (hzs[i] / 440.0).log2() } else { 0.0 };
                                        println!("   k{k} {:>4.0}ms in {:.2} det {:.2} tgt {m} sm {:.2} cand {} age {:.3}", (t - t0) * 1000.0, curve(*t), d, dbg[i].0, dbg[i].1, dbg[i].2);
                                    }
                                }
                            }
                        }
                    }
                    if retune == 0.0 && !keep {
                        let m = pitch_at(&out, ((t0 + 0.3) * fs) as usize + lat, fs);
                        if (m - b as f64).abs() > 0.1 {
                            off_out.push((k, b, ((m - b as f64) * 100.0).round() as i32));
                        }
                    }
                }
                let line = format!("seed {seed0} scale {scale} key {key} tolerance {tol_ms} ms vibrato {keep} speed {retune}: settle {:.0} ms, outside span {:?}, stairs {:?}, unsettled {:?}, output off {:?}",
                    worst_settle * 1000.0, bad_span, stairs, unsettled, off_out);
                println!("{line}");
                let limit = 0.09 + tol_ms as f64 / 1000.0 + if keep { 0.03 } else { 0.0 };
                if !bad_span.is_empty() || !stairs.is_empty() || !unsettled.is_empty() || !off_out.is_empty() || worst_settle > limit {
                    report.push(line);
                }
            }
        }
        assert!(report.is_empty(), "transition problems:\n{}", report.join("\n"));
    }

    /// The tuner must never pull the voice far from its nearest note for more than a moment:
    /// fast slides (a semitone every ~10 ms), big leaps and a detector octave slip used to hold the
    /// old note and pull the voice 2-4 st (12 st on an octave slip) for tens of ms - a gargled,
    /// broken sound with Retune 0.
    #[test]
    fn never_pulls_far_from_the_nearest_note() {
        let fs = 48000.0;
        let smooth = |x: f64| { let x = x.clamp(0.0, 1.0); x * x * (3.0 - 2.0 * x) };
        // 0.3 s notes joined by fast slides of 2..5 st in 25..50 ms, up and down
        let pts: [(f64, f64); 12] = [(54.0, 0.02), (50.6, 0.025), (55.0, 0.015), (51.3, 0.02), (56.1, 0.025), (53.0, 0.012), (57.4, 0.02), (52.2, 0.025), (55.6, 0.015), (51.0, 0.02), (54.8, 0.012), (50.9, 0.018)];
        let dur = 0.25;
        let mut seed = 99u32;
        let jit: Vec<f64> = (0..2000).map(|_| (xorshift(&mut seed) as f64 - 0.5) * 0.16).collect();
        let curve = |t: f64| {
            let k = ((t / dur) as usize).min(pts.len() - 1);
            let tau = t - k as f64 * dur;
            let b = pts[k].0;
            let a = if k == 0 { b } else { pts[k - 1].0 };
            // a slide that overshoots a little and settles, random jitter like a real voice
            let sl = smooth(tau / pts[k].1);
            let over = 0.25 * (b - a).signum() * (-(tau - pts[k].1).max(0.0) / 0.03).exp() * sl;
            a + (b - a) * sl + over + jit[((t * 200.0) as usize).min(1999)] + 0.15 * (2.0 * PI * 5.5 * t).sin()
        };
        let x = sung(fs, curve, dur * pts.len() as f64);
        for (scale, keep) in [(0usize, false), (0, true), (2, false), (2, true)] {
            let mut e = Engine::new(fs, 70.0);
            let mut p = Params::default();
            p.scale = scale;
            p.key = 5;
            p.keep_vibrato = keep;
            p.retune_ms = 0.0;
            p.transition_ms = 0.0;
            let lim = if scale == 0 { 1.6 } else { 2.1 };
            let (mut run, mut worst, mut maxc) = (0usize, 0usize, 0.0f32);
            for ch in x.chunks(64) {
                e.set_params(&p);
                for &s in ch {
                    e.process_frame([s, s], 1);
                }
                let c = e.meters.corr.abs();
                maxc = maxc.max(c);
                if e.meters.voiced && c > lim {
                    run += 1;
                    worst = worst.max(run);
                } else {
                    run = 0;
                }
            }
            let ms = worst as f64 * 64.0 / fs * 1000.0;
            println!("scale {scale} vibrato {keep}: longest pull > {lim} st: {ms:.0} ms (max {maxc:.2} st)");
            assert!(ms <= 20.0, "scale {scale} vibrato {keep}: pulled > {lim} st for {ms:.0} ms (max {maxc:.2} st)");
        }
    }

    /// Same check on real recordings (they are not shipped with the repo): set NYASHK_VOICE_DIR to a
    /// folder of raw mono f32 files at 48 kHz (`*.f32`), e.g. dry a cappella takes. Skipped otherwise.
    #[test]
    fn real_voice_never_pulls_far() {
        let Ok(dir) = std::env::var("NYASHK_VOICE_DIR") else { return };
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "f32")).collect();
        files.sort();
        let mut report = Vec::new();
        for f in &files {
            let raw = std::fs::read(f).unwrap();
            let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            for (scale, keep) in [(0usize, false), (0, true), (2, false), (2, true)] {
                let mut e = Engine::new(48000.0, 70.0);
                let mut p = Params::default();
                p.scale = scale;
                p.key = 5;
                p.keep_vibrato = keep;
                p.retune_ms = 0.0;
                p.transition_ms = 0.0;
                let lim = if scale == 0 { 1.6 } else { 2.1 };
                let (mut run, mut worst, mut maxc) = (0usize, 0usize, 0.0f32);
                for ch in x.chunks(64) {
                    e.set_params(&p);
                    for &s in ch {
                        e.process_frame([s, s], 1);
                    }
                    let c = e.meters.corr.abs();
                    if e.meters.voiced {
                        maxc = maxc.max(c);
                    }
                    if e.meters.voiced && c > lim {
                        run += 1;
                        worst = worst.max(run);
                    } else {
                        run = 0;
                    }
                }
                let ms = worst as f64 * 64.0 / 48.0;
                let line = format!("{} scale {scale} vibrato {keep}: longest pull > {lim} st {ms:.0} ms, max {maxc:.2} st", f.file_name().unwrap().to_string_lossy());
                println!("{line}");
                if ms > 25.0 || maxc > 6.0 {
                    report.push(line);
                }
            }
        }
        assert!(report.is_empty(), "{}", report.join("\n"));
    }

    /// With no correction (Amount 0) the voice must come out as it went in: grains laid back exactly
    /// where they came from. Placing them by the period estimate instead drifted them against the
    /// original (lags wandering by several ms, periods dropped / repeated) - a rough sound.
    #[test]
    fn no_correction_is_transparent() {
        let fs = 48000.0;
        let x = sung(fs, |t| 57.0 + 0.3 * (2.0 * PI * 5.0 * t).sin() + 2.0 * (t / 3.0).min(1.0), 3.0);
        let mut p = Params::default();
        p.scale = 0;
        p.amount = 0.0;
        let (y, lat) = run_engine(fs, &x, &p);
        let w = 1920usize;
        let mut snrs = Vec::new();
        let mut s0 = 24000usize;
        while s0 + w + 400 < x.len() - 2000 {
            let a = &x[s0..s0 + w];
            let ea: f64 = a.iter().map(|v| (*v as f64).powi(2)).sum();
            let mut best = -99.0f64;
            for lag in -400i64..=400 {
                let o = (s0 as i64 + lat as i64 + lag) as usize;
                let b = &y[o..o + w];
                let ab: f64 = a.iter().zip(b).map(|(u, v)| *u as f64 * *v as f64).sum();
                let bb: f64 = b.iter().map(|v| (*v as f64).powi(2)).sum();
                let g = ab / bb.max(1e-12);
                let e: f64 = a.iter().zip(b).map(|(u, v)| (*u as f64 - g * *v as f64).powi(2)).sum();
                best = best.max(10.0 * (ea / e.max(1e-12)).log10());
            }
            snrs.push(best);
            s0 += w;
        }
        snrs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = snrs[snrs.len() / 2];
        let low = snrs[snrs.len() / 10];
        println!("Amount 0: per-40 ms match with the input: median {med:.1} dB, p10 {low:.1} dB");
        assert!(med > 30.0 && low > 20.0, "not transparent: median {med:.1} dB, p10 {low:.1} dB");
    }

    /// Switching modules on and off while singing must not click.
    #[test]
    fn toggling_modules_does_not_click() {
        let mut seed = 77u32;
        let fs = 48000.0;
        let x = voice_signal(fs, 8.0, &mut seed);
        let run = |toggle: bool| -> f32 {
            let mut e = Engine::new(fs, 70.0);
            let mut p = Params::default();
            p.scale = 0;
            p.formant_semis = 4.0;
            p.formant_mode = FormantMode::Detailed;
            let mut maxj = 0.0f32;
            let mut prev = 0.0f32;
            for (bi, chunk) in x.chunks(240).enumerate() {
                if toggle {
                    let ph = bi / 100; // every 0.5 s
                    p.formant_on = ph % 2 == 0;
                    p.colour_on = ph % 3 != 0;
                    p.air_db = 6.0;
                    p.drive = 0.3;
                    p.voices[0].on = ph % 2 == 1;
                    p.voices[1].on = ph % 3 == 1;
                    p.voices[0].interval = 7.0;
                    p.voices_on = ph % 4 != 3;
                }
                e.set_params(&p);
                for &s in chunk {
                    let o = e.process_frame([s, s], 1)[0];
                    if bi > 60 {
                        maxj = maxj.max((o - prev).abs());
                    }
                    prev = o;
                }
            }
            maxj
        };
        let base = run(false);
        let tog = run(true);
        assert!(tog < 3.0 * base + 0.05, "toggle jump {tog} vs plain {base}");
    }
}

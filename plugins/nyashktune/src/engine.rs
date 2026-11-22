//! NyashkTune DSP core (no plugin-framework dependency).
//!
//! Pipeline (the classic real-time tuner: pitch-synchronous resampling)
//! --------
//! input -> high-pass -> decimate -> McLeod pitch method (NSDF, first clear peak) -> pitch frames
//!       -> note logic (Speed, Note Transition, Tolerance, vibrato) -> pitch ratio
//!       -> a read head runs through the delayed input at the ratio; when it drifts too far from
//!          its nominal delay it jumps by whole pitch periods (at the best-matching offset, with a
//!          cross-fade), so whole cycles are repeated or skipped and the waveform stays intact
//!       -> formant stage: short frames of the result, the spectral envelope moved back.
//! Because the read head lags the input, the pitch at the audio being played is known from
//! frames before and after it.

use crate::formant::{Fft, FormantMode, FormantShifter};
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
            retune_ms: 5.0,
            transition_ms: 40.0,
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
/// decimated samples between pitch frames (~4 ms)
const HOP: usize = 64;
const FRAME_RING: usize = 1024;
/// control rate of the pitch logic, samples
const CTL: usize = 32;

#[derive(Clone, Copy, Default)]
struct Frame {
    time: i64,   // input sample index of the centre of the analysed window
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
    pub centre: f32, // note centre the choice is made on (diagnostics)
}

pub struct Engine {
    p: Params,
    allowed_pc: u16,
    /// effective note states (1 legal, 2 illegal, 3 up, 4 down, 5 bypass) per pitch class
    states: [u8; 12],
    pub held: [bool; 128],
    pub meters: Meters,
    /// number of resampler jumps so far (diagnostics)
    pub jumps: u64,

    fs: f64,
    fs_dec: f64,
    decim: usize,
    /// delay of the read head (the pitch shifter) behind the input, nominal
    lat_rs: i64,
    /// longest period searched for (input samples)
    max_period: f64,
    /// the formant stage: frame length, hop
    fn_n: usize,
    fn_hop: usize,

    ring_mask: i64,
    ring: [Vec<f32>; MAX_CH],
    mono: Vec<f32>,
    w: i64, // number of input samples written

    // analysis (McLeod pitch method on a 16 kHz copy)
    hp: Biquad,
    hp_fc: f64,
    dec: Vec<f32>,
    dec_w: i64,
    dec_acc: f64,
    dec_cnt: usize,
    since_hop: usize,
    pfft: Fft,
    pre: Vec<f64>,
    pim: Vec<f64>,
    nsdf: Vec<f64>,
    prev_tau: f64,
    hold_run: u32,
    unvoiced_run: u32,
    frames: Vec<Frame>,
    nframes: u64,
    last_period: f64,

    // the pitch shifter: a read head moving at the pitch ratio; it jumps by whole periods
    // (cross-faded, at the best-matching offset) to stay near its nominal delay
    started: bool,
    pos: f64,
    rate: f64,
    rate_target: f64,
    xf_pos: f64,
    xf_left: usize,
    xf_len: usize,
    xf_pow: bool,
    ctl_cnt: usize,
    cur_voiced: bool,
    cur_period: f64,

    // formant stage: short-time frames of the shifted signal, envelope moved back
    fbuf: [Vec<f32>; MAX_CH],
    fw: usize,
    fola: [Vec<f32>; MAX_CH],
    fr_ring: Vec<f32>,
    fper_ring: Vec<f32>,
    fwin: Vec<f32>,
    fcnt: usize,
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
    c_prev: f64,  // centre at the previous step (for its slope)
    c_slope: f64, // how fast the note centre moves, semitones / s (smoothed)
    n_prev: f64,
    n_slope: f64, // the same for the lightly smoothed pitch
    centre: f64,      // note centre: vibrato taken out (midpoint of its last peak and trough)
    vib_sm: f64,      // lightly smoothed pitch the vibrato turning points are found on
    in_vib: bool,     // the voice is in a recognised vibrato (the centre is its midpoint)
    vib_w: f64,       // 0..1, eased: how much of (pitch - centre) is kept as vibrato
    glide: f64,       // where the output is heading: glides onto a new note with Note Transition
    glide_note: i32,  // the note the glide was last aimed at
    glide_from: f64,  // where the glide started
    glide_t: f64,     // time since it started
    ext: f64,         // running extreme since the last turning point
    ext_t: f64,
    rising: bool,
    tp_max: (f64, f64), // last peak (value, time)
    tp_min: (f64, f64), // last trough
    tp_max_prev: (f64, f64), // the peak / trough before those: a vibrato repeats itself
    tp_min_prev: (f64, f64),
    clock: f64,
}

#[inline]
fn rd(r: &[f32], mask: i64, k: i64) -> f32 {
    r[(k & mask) as usize]
}

/// Window length of the pitch analysis for a lowest frequency (decimated samples).
fn mpm_window(fs_dec: f64, min_hz: f64) -> usize {
    (((fs_dec / min_hz.max(40.0)) * 2.0).ceil() as usize + 4).clamp(256, 1000)
}

impl Engine {
    pub fn new(sample_rate: f64, lowest_hz: f32) -> Self {
        let fs = sample_rate;
        let mut ring_size = 1usize;
        while ring_size < (fs * 0.5) as usize + 16384 {
            ring_size <<= 1;
        }
        let decim = ((fs / 16000.0).round() as usize).max(1);
        let fs_dec = fs / decim as f64;
        let mut fn_n = 256usize;
        while (fn_n as f64) < 0.021 * fs {
            fn_n <<= 1;
        }
        let fwin: Vec<f32> = (0..fn_n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / fn_n as f64).cos()) as f32).collect();
        let mut e = Self {
            p: Params::default(),
            allowed_pc: 0xFFF,
            states: [1; 12],
            held: [false; 128],
            meters: Meters { voiced: false, hz: 0.0, target: 0.0, corr: 0.0, centre: 0.0 },
            jumps: 0,
            fs,
            fs_dec,
            decim,
            lat_rs: 1024,
            max_period: fs / 80.0,
            fn_n,
            fn_hop: fn_n / 4,
            ring_mask: ring_size as i64 - 1,
            ring: [vec![0.0; ring_size], vec![0.0; ring_size]],
            mono: vec![0.0; ring_size],
            w: 0,
            hp: Biquad::default(),
            hp_fc: 0.0,
            dec: vec![0.0; 4096],
            dec_w: 0,
            dec_acc: 0.0,
            dec_cnt: 0,
            since_hop: 0,
            pfft: Fft::new(2048),
            pre: vec![0.0; 2048],
            pim: vec![0.0; 2048],
            nsdf: vec![0.0; 1024],
            prev_tau: 0.0,
            hold_run: 0,
            unvoiced_run: 100,
            frames: vec![Frame::default(); FRAME_RING],
            nframes: 0,
            last_period: fs / 150.0,
            started: false,
            pos: 0.0,
            rate: 1.0,
            rate_target: 1.0,
            xf_pos: 0.0,
            xf_left: 0,
            xf_len: 1,
            xf_pow: false,
            ctl_cnt: 0,
            cur_voiced: false,
            cur_period: fs / 150.0,
            fbuf: [vec![0.0; fn_n], vec![0.0; fn_n]],
            fw: 0,
            fola: [vec![0.0; fn_n * 2], vec![0.0; fn_n * 2]],
            fr_ring: vec![1.0; fn_n],
            fper_ring: vec![(fs / 150.0) as f32; fn_n],
            fwin,
            fcnt: 0,
            shifter: FormantShifter::new(fn_n * 4),
            gbuf: [vec![0.0; fn_n], vec![0.0; fn_n]],
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
            c_prev: 60.0,
            c_slope: 0.0,
            n_prev: 60.0,
            n_slope: 0.0,
            centre: 60.0,
            vib_sm: 60.0,
            in_vib: false,
            vib_w: 0.0,
            glide: 0.0,
            glide_note: i32::MIN,
            glide_from: 0.0,
            glide_t: 0.0,
            ext: 60.0,
            ext_t: 0.0,
            rising: true,
            tp_max: (f64::NAN, -1.0),
            tp_min: (f64::NAN, -1.0),
            tp_max_prev: (f64::NAN, -1.0),
            tp_min_prev: (f64::NAN, -1.0),
            clock: 0.0,
        };
        e.set_lowest_hz(lowest_hz);
        e
    }

    /// Lowest pitch to track; decides the latency. Allocation-free.
    pub fn set_lowest_hz(&mut self, hz: f32) {
        let hz = hz.clamp(40.0, 400.0) as f64;
        self.max_period = self.fs / hz;
        // the read head swings up to a period either side of its nominal delay, and a jump plus
        // its cross-fade must never reach the newest input sample
        self.lat_rs = (1.45 * self.max_period).ceil() as i64 + 32;
        self.hp_fc = (hz * 0.6).max(25.0);
        self.reset();
    }

    /// Total latency: the pitch shifter plus the formant stage.
    pub fn latency_samples(&self) -> u32 {
        (self.lat_rs as usize + self.fn_n) as u32
    }

    pub fn reset(&mut self) {
        for c in 0..MAX_CH {
            self.ring[c].iter_mut().for_each(|v| *v = 0.0);
            self.fbuf[c].iter_mut().for_each(|v| *v = 0.0);
            self.fola[c].iter_mut().for_each(|v| *v = 0.0);
        }
        self.fr_ring.iter_mut().for_each(|v| *v = 1.0);
        self.fw = 0;
        self.fcnt = 0;
        self.mono.iter_mut().for_each(|v| *v = 0.0);
        self.dec.iter_mut().for_each(|v| *v = 0.0);
        self.w = 0;
        self.dec_w = 0;
        self.dec_acc = 0.0;
        self.dec_cnt = 0;
        self.since_hop = 0;
        self.hp = Biquad::highpass(self.fs, self.hp_fc);
        self.prev_tau = 0.0;
        self.unvoiced_run = 100;
        self.nframes = 0;
        self.last_period = self.fs / 150.0;
        self.started = false;
        self.pos = 0.0;
        self.rate = 1.0;
        self.rate_target = 1.0;
        self.xf_left = 0;
        self.ctl_cnt = 0;
        self.cur_voiced = false;
        self.cur_period = self.fallback();
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
        self.meters = Meters { voiced: false, hz: 0.0, target: 0.0, corr: 0.0, centre: 0.0 };
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
            self.since_hop += 1;
            if self.since_hop >= HOP {
                self.since_hop = 0;
                self.run_mpm();
            }
        }
        self.w += 1;

        // ---- the pitch shifter
        if !self.started && self.w > self.lat_rs + 8 {
            self.started = true;
            self.pos = (self.w - 1 - self.lat_rs) as f64;
            self.rate = 1.0;
            self.rate_target = 1.0;
            self.ctl_cnt = 0;
        }
        let mut wet = [0.0f32; MAX_CH];
        let mut ratio_now = 1.0f64;
        if self.started {
            if self.ctl_cnt == 0 {
                self.control();
            }
            self.ctl_cnt = (self.ctl_cnt + 1) % CTL;
            // the ratio glides per sample (no zipper steps)
            self.rate += (self.rate_target - self.rate) * (1.0 - (-1.0 / (0.0015 * self.fs)).exp());
            ratio_now = self.rate;
            for c in 0..nch {
                wet[c] = self.read_cubic(c, self.pos);
            }
            if self.xf_left > 0 {
                let t = 1.0 - self.xf_left as f64 / self.xf_len as f64;
                let (gi, go) = if self.xf_pow { ((0.5 * PI * t).sin(), (0.5 * PI * t).cos()) } else { (0.5 - 0.5 * (PI * t).cos(), 0.5 + 0.5 * (PI * t).cos()) };
                for c in 0..nch {
                    let old = self.read_cubic(c, self.xf_pos);
                    wet[c] = wet[c] * gi as f32 + old * go as f32;
                }
                self.xf_pos += self.rate;
                self.xf_left -= 1;
            }
            self.pos += self.rate;
            // never read past the newest sample (only on absurd settings)
            let newest = (self.w - 3) as f64;
            if self.pos > newest {
                self.pos = newest - self.max_period;
            }
        }

        // ---- formant stage (constant latency: always framed, processed only when wanted)
        let n = self.fn_n;
        let fi = self.fw % n;
        for c in 0..MAX_CH {
            self.fbuf[c][fi] = if c < nch { wet[c] } else { 0.0 };
        }
        self.fr_ring[fi] = ratio_now as f32;
        self.fper_ring[fi] = self.cur_period as f32;
        self.fw += 1;
        self.fcnt += 1;
        if self.fcnt >= self.fn_hop {
            self.fcnt = 0;
            self.formant_frame(nch);
        }
        let oi = self.fw % (2 * n);
        let mut outw = [0.0f32; MAX_CH];
        for c in 0..nch {
            outw[c] = self.fola[c][oi];
            self.fola[c][oi] = 0.0;
        }

        // ---- output
        let mut out = [0.0f32; MAX_CH];
        let drive = if self.p.colour_on { self.p.drive.clamp(0.0, 1.0) } else { 0.0 };
        let air_on = self.p.colour_on && self.p.air_db.abs() > 0.05;
        let lat = self.latency_samples() as i64;
        let o = self.w - 1;
        for c in 0..nch {
            let mut y = outw[c];
            if air_on {
                y = self.air[c].tick(y as f64) as f32;
            }
            if drive > 0.0 {
                let g = 1.0 + 3.0 * drive;
                y = (y * g).tanh() / g.sqrt();
            }
            let dry = rd(&self.ring[c], self.ring_mask, o - lat);
            let v = if self.p.bypass { dry } else { dry * (1.0 - mix) + y * mix };
            out[c] = v * out_gain;
        }
        out
    }

    /// One short-time frame of the shifted signal: the formants the resampling moved by the
    /// pitch ratio are moved back (Formant Corrected), then overlap-added.
    fn formant_frame(&mut self, nch: usize) {
        let n = self.fn_n;
        let start = self.fw; // fbuf holds the last n samples, oldest at fw % n
        let centre = (start + n / 2) % n;
        let r = self.fr_ring[centre] as f64;
        let per = self.fper_ring[centre] as f64;
        let s = 1.0 / r;
        let want = self.p.formant_on && self.p.formant_preserve && s.ln().abs() > 0.001;
        for c in 0..nch {
            for j in 0..n {
                self.gbuf[c][j] = self.fbuf[c][(start + j) % n] * self.fwin[j];
            }
        }
        if want {
            let (g0, g1) = self.gbuf.split_at_mut(1);
            let right = if nch == 2 { Some(&mut g1[0][..n]) } else { None };
            let ok = self.shifter.process2(&mut g0[0][..n], right, (per / r).max(16.0), self.fs, FormantMode::Smooth, s.clamp(0.4, 2.5), 0.0);
            let _ = ok;
        }
        // hop n/4 with a Hann window: the windows sum to 2
        let base = self.fw;
        for c in 0..nch {
            for j in 0..n {
                let idx = (base + j) % (2 * n);
                self.fola[c][idx] += 0.5 * self.gbuf[c][j];
            }
        }
    }

    // --------------------------------------------------------------------------------------
    // Pitch shifter control
    // --------------------------------------------------------------------------------------
    fn control(&mut self) {
        let (voiced, pq) = self.query(self.pos);
        let p = if voiced { pq } else { self.fallback() };
        let dt = CTL as f64 / self.fs;
        let r = self.compute_ratio(dt, voiced, p);
        self.rate_target = r.clamp(0.25, 4.0);
        self.cur_voiced = voiced;
        self.cur_period = p;
        if self.xf_left > 0 {
            return;
        }
        let d = self.w as f64 - self.pos;
        let l = self.lat_rs as f64;
        let rr = self.rate_target.max(self.rate);
        let rl = self.rate_target.min(self.rate);
        if voiced {
            let t = p;
            if d < l - 0.55 * t && rr > 1.0 {
                let k = ((l - d) / t).round().max(1.0);
                self.jump(-(k * t), t, true);
            } else if d > l + 0.55 * t && rl < 1.0 {
                let k = ((d - l) / t).round().max(1.0);
                self.jump(k * t, t, true);
            }
        } else {
            // breath / consonants: the ratio is 1, the head only has to come home after a note
            let lim = 0.6 * self.max_period;
            if (d - l).abs() > lim {
                let j = d - l;
                self.jump(j, self.fallback(), false);
            }
        }
    }

    /// Moves the read head by about `amount` samples (+ forwards, - back), at the offset whose
    /// waveform best continues the current one, with a cross-fade.
    fn jump(&mut self, amount: f64, period: f64, voiced: bool) {
        let win = (period.min(0.012 * self.fs)).max(32.0) as i64;
        let search = if voiced { (0.15 * period).max(4.0) as i64 } else { (0.5 * period) as i64 };
        let cand = self.pos + amount;
        let newest = self.w - 3;
        let m = self.ring_mask;
        // the current head's upcoming samples
        let p0 = self.pos.round() as i64;
        let mut e0 = 1e-12f64;
        let mut j = 0;
        while j < win {
            let v = rd(&self.mono, m, p0 + j) as f64;
            e0 += v * v;
            j += 2;
        }
        let mut best = 0i64;
        let mut best_s = f64::MIN;
        for dlt in -search..=search {
            let c0 = cand.round() as i64 + dlt;
            if c0 + win + 4 >= newest {
                continue;
            }
            let (mut dot, mut e1) = (0.0f64, 1e-12f64);
            let mut j = 0;
            while j < win {
                let a = rd(&self.mono, m, p0 + j) as f64;
                let b = rd(&self.mono, m, c0 + j) as f64;
                dot += a * b;
                e1 += b * b;
                j += 2;
            }
            // a slight preference for the nominal offset
            let sc = dot / (e0 * e1).sqrt() - 0.05 * (dlt.abs() as f64 / search.max(1) as f64);
            if sc > best_s {
                best_s = sc;
                best = dlt;
            }
        }
        if best_s == f64::MIN {
            return;
        }
        let new_pos = self.pos + amount + best as f64;
        // cross-fade: about a period (shorter when the next jump would come sooner)
        let r_dev = (self.rate - 1.0).abs().max(1e-3);
        let mut len = if voiced { period.clamp(0.004 * self.fs, 0.015 * self.fs).min(0.8 * period / r_dev) } else { 0.012 * self.fs };
        // neither head may reach the newest input during the fade
        let room = (self.w as f64 - new_pos.max(self.pos)) - 8.0;
        if self.rate > 1.0 {
            len = len.min(room / (self.rate - 1.0));
        }
        let len = len.max(16.0) as usize;
        self.xf_pos = self.pos;
        self.pos = new_pos;
        self.xf_len = len;
        self.xf_left = len;
        // correlated (a good match) -> equal gain; noise -> equal power
        self.xf_pow = best_s < 0.6;
        self.jumps += 1;
    }

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

    // --------------------------------------------------------------------------------------
    // Pitch frames: McLeod pitch method (normalised square difference, first strong peak)
    // --------------------------------------------------------------------------------------
    fn run_mpm(&mut self) {
        let min_tau = ((self.fs_dec / self.p.max_hz.clamp(1600.0, 2000.0) as f64) as usize).max(2);
        let max_tau = ((self.fs_dec / self.p.min_hz.max(40.0) as f64) as usize + 2).min(500);
        let wlen = mpm_window(self.fs_dec, self.p.min_hz as f64);
        if (self.dec_w as usize) < wlen {
            return;
        }
        let mut nfft = 64;
        while nfft < 2 * wlen {
            nfft <<= 1;
        }
        let nfft = nfft.min(2048);
        let mut e = 0.0f64;
        for i in 0..nfft {
            let v = if i < wlen { self.dec[((self.dec_w - wlen as i64 + i as i64) & 4095) as usize] as f64 } else { 0.0 };
            self.pre[i] = v;
            self.pim[i] = 0.0;
            e += v * v;
        }
        let rms_db = 10.0 * (e / wlen as f64 + 1e-12).log10();
        // autocorrelation by FFT
        self.pfft.forward(&mut self.pre[..nfft], &mut self.pim[..nfft]);
        for i in 0..nfft {
            self.pre[i] = self.pre[i] * self.pre[i] + self.pim[i] * self.pim[i];
            self.pim[i] = 0.0;
        }
        self.pfft.inverse(&mut self.pre[..nfft], &mut self.pim[..nfft]);
        // m(tau) = sum of x_j^2 + x_{j+tau}^2 over the overlap
        let x = |i: usize| self.dec[((self.dec_w - wlen as i64 + i as i64) & 4095) as usize] as f64;
        let top = (wlen - 1).min(self.nsdf.len() - 1);
        let mut mm = 2.0 * e;
        self.nsdf[0] = 1.0;
        for tau in 1..=top {
            mm -= x(tau - 1).powi(2) + x(wlen - tau).powi(2);
            self.nsdf[tau] = if mm > 1e-12 { 2.0 * self.pre[tau] / mm } else { 0.0 };
        }
        // key maxima: the highest point between each rise above zero and the next fall below
        let mut keys: [(usize, f64); 24] = [(0, 0.0); 24];
        let mut nk = 0;
        let mut tau = 1;
        // skip the main lobe around zero lag
        while tau < top && self.nsdf[tau] > 0.0 {
            tau += 1;
        }
        while tau < top && nk < keys.len() {
            while tau < top && self.nsdf[tau] <= 0.0 {
                tau += 1;
            }
            let mut bt = tau;
            while tau < top && self.nsdf[tau] > 0.0 {
                if self.nsdf[tau] > self.nsdf[bt] {
                    bt = tau;
                }
                tau += 1;
            }
            if bt >= min_tau && bt <= max_tau && bt > 0 && bt < top && self.nsdf[bt] > 0.0 {
                keys[nk] = (bt, self.nsdf[bt]);
                nk += 1;
            }
        }
        let keys = &keys[..nk];
        let nmax = keys.iter().map(|k| k.1).fold(0.0f64, f64::max);
        // the first peak that is nearly as clear as the clearest one: never a multiple of the period
        let thr = 0.86 * nmax;
        let mut pick = keys.iter().copied().find(|k| k.1 >= thr);
        // a strong 2nd harmonic (vowels "ee" / "oo", breathy voice) puts a peak at half the period
        // that can pass the threshold: a real period repeats at least as clearly at twice its lag
        // as at half of it, so a clearly better peak at twice the pick means the pick was half.
        for _ in 0..2 {
            if let Some(pk) = pick {
                let dbl = keys.iter().copied().filter(|k| ((k.0 as f64) / (2.0 * pk.0 as f64)).ln().abs() < 0.06).max_by(|a, b| a.1.total_cmp(&b.1));
                // ... and the half-period pick shows itself at 3x: that is 1.5 true periods (a dip)
                let near_max = |t: f64| -> Option<f64> {
                    let (a, b) = ((t * 0.96) as usize, (t * 1.04).ceil() as usize);
                    if b >= top {
                        return None;
                    }
                    Some((a..=b).map(|i| self.nsdf[i]).fold(-1.0, f64::max))
                };
                match dbl {
                    Some(d) if d.1 > pk.1 + 0.04 && near_max(3.0 * pk.0 as f64).map_or(d.1 > pk.1 + 0.2, |v3| d.1 > v3 + 0.12) => pick = Some(d),
                    _ => break,
                }
            }
        }
        // continuity: a peak right at the period we follow wins when it is about as clear (stops
        // flicker between neighbouring estimates; never between octaves: that peak must be the
        // first or later than an already clear one only if it is not a multiple)
        if let (Some(pk), true) = (pick, self.prev_tau > 0.0) {
            if let Some(near) = keys.iter().copied().filter(|k| ((k.0 as f64) / self.prev_tau).ln().abs() < 0.08).max_by(|a, b| a.1.total_cmp(&b.1)) {
                let is_multiple = near.0 > pk.0 && ((near.0 as f64 / pk.0 as f64) - (near.0 as f64 / pk.0 as f64).round()).abs() < 0.08;
                if !is_multiple && near.1 >= 0.93 * pk.1 {
                    pick = Some(near);
                }
            }
        }
        // a sudden drop of half an octave or more while the period we follow is still clearly
        // there is a glide / a consonant smearing the window, not a real leap: stay (a jump up
        // to a clear shorter period is always taken - that is how a wrong low reading ends)
        if let (Some(pk), true) = (pick, self.prev_tau > 0.0) {
            let ratio = pk.0 as f64 / self.prev_tau;
            if ratio > 1.45 {
                if let Some(near) = keys.iter().copied().filter(|k| ((k.0 as f64) / self.prev_tau).ln().abs() < 0.12).max_by(|a, b| a.1.total_cmp(&b.1)) {
                    if near.1 >= pk.1 - 0.02 {
                        pick = Some(near);
                    }
                }
            }
        }
        let gate_open = rms_db > self.p.gate_db as f64;
        let wl_in = (wlen * self.decim) as i64;
        let mut frame = Frame { time: self.w - wl_in / 2, period: self.last_period as f32, voiced: false };
        // two notes in one window (a leap, a slide, a consonant between notes) repeat together
        // only at a long common period: a sudden, not quite clean reading well below the note we
        // follow is held off for a few frames (a real low note is clean and stays)
        if let (Some(pk), true, true) = (pick, self.prev_tau > 0.0, gate_open) {
            if pk.0 as f64 > 1.45 * self.prev_tau && pk.1 < 0.93 && self.hold_run < 4 {
                self.hold_run += 1;
                let frame = Frame { time: self.w - (wlen * self.decim) as i64 / 2, period: (self.prev_tau * self.decim as f64) as f32, voiced: true };
                self.frames[(self.nframes % FRAME_RING as u64) as usize] = frame;
                self.nframes += 1;
                return;
            }
        }
        self.hold_run = 0;
        match pick {
            Some((t, v)) if v > 0.45 && gate_open => {
                let (a, b, c) = (self.nsdf[t - 1], self.nsdf[t], self.nsdf[t + 1]);
                let den = a - 2.0 * b + c;
                let sh = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
                let tf = t as f64 + sh;
                frame.period = (tf * self.decim as f64) as f32;
                frame.voiced = true;
                self.prev_tau = tf;
                self.unvoiced_run = 0;
                self.last_period += 0.3 * (frame.period as f64 - self.last_period);
            }
            _ => {
                self.unvoiced_run += 1;
                if self.unvoiced_run > 4 {
                    self.prev_tau = 0.0;
                }
            }
        }
        self.frames[(self.nframes % FRAME_RING as u64) as usize] = frame;
        self.nframes += 1;
    }

    /// Pitch of the audio around input time `t` (median of the frames centred near it).
    fn query(&self, t: f64) -> (bool, f64) {
        if self.nframes == 0 {
            return (false, self.fallback());
        }
        let hop = (HOP * self.decim) as f64;
        let mut periods = [0.0f32; 8];
        let (mut n, mut nv) = (0usize, 0usize);
        let newest = self.frames[((self.nframes - 1) % FRAME_RING as u64) as usize];
        let target = t.min(newest.time as f64);
        let mut i = self.nframes;
        let mut scanned = 0;
        while i > 0 && scanned < 64 && n < 5 {
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
                // counted only once the pitch has stopped sliding fast (> 25 st/s: a slide passes
                // other notes; a vibrato never moves that fast)
                if self.far_note != rb {
                    self.far_age = 0.0;
                }
                self.far_note = rb;
                if self.n_slope.abs() < 25.0 {
                    self.far_age += dt;
                }
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
        // While the voice glides from note to note (the centre moves faster than 8 st/s) no note is
        // chosen on the way: a slide 60 -> 58 used to settle on 59 first and pull the voice up to it
        // for a while. The far rule above still catches real jumps.
        let gliding = self.c_slope.abs() > 8.0;
        if best != cur && past_border && gliding {
            self.cand_note = best;
            self.cand_age = 0.0;
        } else if best != cur && past_border {
            if self.cand_note == best {
                self.cand_age += dt;
            } else {
                self.cand_note = best;
                self.cand_age = 0.0;
            }
            if (past && self.cand_age >= hold) || self.cand_age >= hold + hold.max(0.035) {
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
                self.tp_max_prev = self.tp_max;
                self.tp_max = (self.ext, self.ext_t);
                self.rising = false;
                self.ext = s;
                self.ext_t = t;
            }
        } else if s <= self.ext {
            self.ext = s;
            self.ext_t = t;
        } else if s > self.ext + TURN {
            self.tp_min_prev = self.tp_min;
            self.tp_min = (self.ext, self.ext_t);
            self.rising = true;
            self.ext = s;
            self.ext_t = t;
        }
        let (hi, th) = self.tp_max;
        let (lo, tl) = self.tp_min;
        let half = (th - tl).abs();
        // A vibrato repeats: the peak before the last one (or the trough before the last one) is
        // at about the same height, one period earlier. A single up-down - a step to the next
        // note with an overshoot, a scoop - is not a vibrato: taking the midpoint of the old note
        // and the new one used to hold the centre between them for up to 0.3 s and pick the
        // wrong note (with a key set: the scale note on the old side of an illegal one).
        let again = |cur: (f64, f64), prev: (f64, f64)| {
            prev.0.is_finite() && (cur.0 - prev.0).abs() < 0.45 && (0.07..=0.5).contains(&(cur.1 - prev.1))
        };
        let repeats = again(self.tp_max, self.tp_max_prev) || again(self.tp_min, self.tp_min_prev);
        // (pairs across a step cannot happen: a step forgets the turning points, see forget_turns)
        let _ = repeats;
        let vibrato = hi.is_finite()
            && lo.is_finite()
            && hi > lo
            && hi - lo < 2.0
            && (0.03..=0.25).contains(&half)
            // the next turn is due within about half a period: if it does not come, the voice has
            // settled (on this note or a new one) and the vibrato is over
            && t - th.max(tl) < (1.4 * half).max(0.06)
            // still inside the band of its last peak and trough: after a step to another note the
            // old peak / trough must not pull the centre back half-way
            && s > lo - 0.15
            && s < hi + 0.15;
        self.in_vib = vibrato;
        let target = if vibrato { 0.5 * (hi + lo) } else { s };
        self.centre += (target - self.centre) * (1.0 - (-dt / 0.025).exp());
    }

    fn forget_turns(&mut self) {
        self.tp_max = (f64::NAN, -1.0);
        self.tp_min = (f64::NAN, -1.0);
        self.tp_max_prev = (f64::NAN, -1.0);
        self.tp_min_prev = (f64::NAN, -1.0);
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
            self.vib_w = 0.0;
            self.in_vib = false;
            self.glide_note = i32::MIN;
            self.slow_midi = midi;
            self.note_sm = midi;
            self.centre = midi;
            self.c_prev = midi;
            self.c_slope = 0.0;
            self.n_prev = midi;
            self.n_slope = 0.0;
            self.vib_sm = midi;
            self.ext = midi;
            self.tp_max = (f64::NAN, -1.0);
            self.tp_min = (f64::NAN, -1.0);
            self.tp_max_prev = (f64::NAN, -1.0);
            self.tp_min_prev = (f64::NAN, -1.0);
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
            // a step: peaks and troughs from before it belong to the old note and must not be
            // paired with the new one into a fake vibrato midpoint
            self.forget_turns();
        } else {
            self.note_sm += (midi - self.note_sm) * (1.0 - (-dt / 0.010).exp());
        }
        if (self.note_sm - self.centre).abs() > 1.2 {
            self.centre = self.note_sm; // left the vibrato: follow at once
            self.forget_turns();
        }
        // how fast the note centre (vibrato taken out) is moving: a glide / scoop between notes
        if dt > 0.0 {
            let sl = (self.centre - self.c_prev) / dt;
            self.c_slope += (sl - self.c_slope) * (1.0 - (-dt / 0.012).exp());
        }
        self.c_prev = self.centre;
        if dt > 0.0 {
            let sl = ((self.note_sm - self.n_prev) / dt).clamp(-200.0, 200.0);
            self.n_slope += (sl - self.n_slope) * (1.0 - (-dt / 0.008).exp());
        }
        self.n_prev = self.note_sm;
        let keep = self.p.keep_vibrato;
        // the note is always chosen on the centre: vibrato never flips it to a neighbour, in either
        // mode (Vibrato On / Off only decides whether the vibrato itself is corrected away)
        let basis = self.centre;
        let mut err = 0.0;
        if self.pick_note(basis, midi, dt) {
            // Vibrato On: the centre goes onto the note and the singer's vibrato rides on top,
            // scaled by Depth (0 flat, 1 as sung, 2 twice as wide). Off: the pitch itself is corrected.
            // Only a real vibrato (a regular oscillation, recognised by track_centre) is kept. Any
            // other movement - a wobbly, drifting, unsteady voice - is corrected like with Vibrato
            // Off: keeping everything that was not the slow centre let an untrained voice through
            // almost uncorrected (with a key set it sounded like the tuner missed every note).
            // The switch between the two is eased over ~60 ms so a vibrato fades in and out.
            let want_w = if keep && self.in_vib { 1.0 } else { 0.0 };
            self.vib_w += (want_w - self.vib_w) * (1.0 - (-dt / 0.06).exp());
            let d = (self.p.vib_amount as f64).clamp(0.0, 2.0);
            // Note Transition shapes only the move from one note to the next: on a new note the
            // glide starts where the output is right now and eases onto the new note. Inside a
            // note the voice is always held on the glide with Speed. (Before, the whole
            // correction ran on the slow transition time for most of every note, so a wobbly
            // voice was hardly corrected - "the tuner misses every note".)
            if self.cur_note != self.glide_note {
                let now = midi + self.corr_sm;
                // (the octave the output is in does not matter: keep the glide near the note)
                self.glide = now - 12.0 * ((now - self.cur_note as f64) / 12.0).round();
                if self.glide_note == i32::MIN {
                    self.glide = self.cur_note as f64 + (midi - self.cur_note as f64).clamp(-1.0, 1.0);
                }
                self.glide_note = self.cur_note;
                self.glide_from = self.glide;
                self.glide_t = 0.0;
            }
            // the glide takes exactly Note Transition: smooth start, smooth landing, then it is on
            // the note (an exponential with that time constant needed 3x as long to get there)
            let tr = self.p.transition_ms as f64 / 1000.0;
            self.glide_t += dt;
            let u = if tr < 0.0005 { 1.0 } else { (self.glide_t / tr).min(1.0) };
            let ease = u * u * (3.0 - 2.0 * u);
            self.glide = self.glide_from + (self.cur_note as f64 - self.glide_from) * ease;
            err = self.glide - midi + self.vib_w * d * (midi - self.centre);
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
        // (the move between notes is already in the glide; this is the Speed of holding the voice)
        let tau = self.p.retune_ms as f64 / 1000.0;
        // While the voice has crossed into another note's half and the switch is only waiting out
        // the guard / Tolerance time, do not pull it any harder back to the note it is leaving
        // (that was the "note hangs and drags" feeling: up to half a semitone of pull for 200 ms).
        let pending = (self.cand_note != i32::MIN && self.cand_note != self.cur_note) || self.far_note != i32::MIN;
        if pending && err.abs() > self.corr_sm.abs() {
            // hold
        } else if tau < 0.0005 {
            self.corr_sm = err;
        } else {
            self.corr_sm += (err - self.corr_sm) * (1.0 - (-dt / tau).exp());
        }
        // the transition is over once the correction has arrived (within ~5 cents)
        // (and the pitch measurement has had ~80 ms to settle after the note change)
        // A wobbly real voice never sits still long enough for "within 5 cents", and the whole
        // note used to stay in the slow transition glide (the tuner seemed to miss every note):
        // the transition also ends after twice its own time, then Speed takes over.
        let tr_s = self.p.transition_ms as f64 / 1000.0;
        if self.transition && self.note_age > 0.08 && ((err - self.corr_sm).abs() < 0.05 || self.note_age > 0.08 + 2.0 * tr_s) {
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
        self.meters.centre = self.centre as f32;
        2f64.powf(total / 12.0)
    }
}

#[cfg(test)]
#[path = "analysis.rs"]
mod analysis;

#[cfg(test)]
mod fuzz_tests {
    use super::*;

    /// Octave safety of the detector: notes above the search range (the range only decides what
    /// is corrected) and note onsets where the 2nd harmonic is the strongest partial (formant on
    /// it, fundamental weak) must not be read an octave off - the grains would come out an
    /// octave off too ("jumps an octave for no reason").
    /// Low notes that start right after a breath / consonant: the first voiced grain is much
    /// longer than the unvoiced ones before it. It used to be cut off where it would start in the
    /// past - a click, and only for some start positions (a render had it "every other time").
    #[test]
    fn no_clicks_when_a_low_note_starts_after_noise() {
        let fs = 48000.0;
        let mut seed = 99u32;
        let mut x = Vec::new();
        let (mut ph, mut lp) = (0.0f64, 0.0f32);
        for (k, f0) in [47.0f64, 52.0, 44.0, 58.0, 49.0, 55.0].iter().enumerate() {
            // 120 ms of breathy noise, then 500 ms of a low voice (harmonics up to 3 kHz)
            for _ in 0..(0.12 * fs) as usize {
                lp += 0.3 * (xorshift(&mut seed) - 0.5 - lp);
                x.push(lp * 0.15);
            }
            let f0 = f0 * (1.0 + 0.002 * k as f64);
            let nh = (3000.0 / f0) as usize;
            let n = (0.5 * fs) as usize;
            for i in 0..n {
                ph += f0 / fs;
                let fade = ((i.min(n - i)) as f64 / 480.0).min(1.0);
                let mut y = 0.0;
                for h in 1..=nh {
                    let fh = h as f64 * f0;
                    let amp = 1.0 / (1.0 + ((fh - 600.0) / 300.0).powi(2)) + 0.2 / h as f64;
                    y += amp * (2.0 * PI * h as f64 * ph).sin();
                }
                x.push((y * 0.05 * fade) as f32);
            }
        }
        let p = Params { min_hz: 40.0, amount: 0.0, ..Params::default() };
        let count = |out: &[f32]| -> usize {
            // a click: a 2nd difference far above its 10 ms neighbourhood on a sounding signal
            let d2: Vec<f32> = out.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
            let (mut clicks, mut last) = (0, 0usize);
            for i in 480..d2.len().saturating_sub(480) {
                let loc = (d2[i - 240..i + 240].iter().map(|v| v * v).sum::<f32>() / 480.0).sqrt() + 1e-7;
                if d2[i].abs() > 9.0 * loc && d2[i].abs() > 0.004 && i > last + 240 {
                    clicks += 1;
                    last = i;
                }
            }
            clicks
        };
        assert_eq!(count(&x), 0, "the test signal itself clicks");
        let mut worst = 0;
        for off in [0usize, 33, 129, 480, 777, 1000, 2047, 3001] {
            let mut e = Engine::new(fs, p.min_hz);
            e.set_params(&p);
            let mut out = Vec::with_capacity(x.len() + off);
            for i in 0..x.len() + off {
                let v = if i < off { 0.0 } else { x[i - off] };
                out.push(e.process_frame([v, v], 1)[0]);
            }
            let clicks = count(&out);
            worst = worst.max(clicks);
        }
        assert!(worst == 0, "{worst} clicks in one render");
    }

    #[test]
    fn no_octave_errors_high_notes_and_weak_fundamental() {
        let fs = 48000.0;
        // a pulse train through a resonance at 2 x f0: the 2nd harmonic dominates
        let voice = |notes: &[f64], secs_each: f64, res_mult: f64| -> (Vec<f32>, Vec<f64>) {
            let n_each = (fs * secs_each) as usize;
            let (mut ph, mut y1, mut y2) = (0.0f64, 0.0f64, 0.0f64);
            let (mut out, mut truth) = (Vec::new(), Vec::new());
            for (k, &m) in notes.iter().enumerate() {
                let f0 = 440.0 * 2f64.powf((m - 69.0) / 12.0);
                for i in 0..n_each {
                    let g = ((i as f64 / (0.02 * fs)).min(1.0)) * (((n_each - i) as f64 / (0.03 * fs)).min(1.0));
                    let vib = 1.0 + 0.004 * (2.0 * PI * 5.5 * (k * n_each + i) as f64 / fs).sin();
                    ph = (ph + f0 * vib / fs) % 1.0;
                    let src = if ph < 0.08 { 1.0 } else { -0.08 / 0.92 };
                    let r = 0.97;
                    let th = 2.0 * PI * (f0 * res_mult).min(fs * 0.4) / fs;
                    let y = src * 0.1 + 2.0 * r * th.cos() * y1 - r * r * y2;
                    y2 = y1;
                    y1 = y;
                    out.push((y * g) as f32);
                    truth.push(if g > 0.99 { f0 } else { 0.0 });
                }
            }
            let pk = out.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            (out.iter().map(|v| v / pk * 0.5).collect(), truth)
        };
        for (name, notes, res, max_hz) in [
            ("high notes above the range", vec![84.0, 86.0, 88.0, 86.0, 84.0], 1.0, 1100.0f32),
            ("weak fundamental, female", vec![64.0, 67.0, 69.0, 72.0, 69.0], 2.0, 1100.0),
            ("weak fundamental, male", vec![45.0, 48.0, 50.0, 52.0, 48.0], 2.0, 1100.0),
        ] {
            let (x, truth) = voice(&notes, 0.5, res);
            let mut e = Engine::new(fs, 60.0);
            let mut p = Params::default();
            p.min_hz = 60.0;
            p.max_hz = max_hz;
            let (mut bad, mut total) = (0, 0);
            let lat = e.latency_samples() as usize;
            for (i, ch) in x.chunks(64).enumerate() {
                e.set_params(&p);
                for &s in ch {
                    e.process_frame([s, s], 1);
                }
                // the meter belongs to the audio coming out now (one latency ago)
                let t = (i * 64).saturating_sub(lat);
                if e.meters.voiced && truth.get(t).copied().unwrap_or(0.0) > 0.0 {
                    total += 1;
                    if (e.meters.hz as f64 / truth[t]).log2().abs() > 0.5 {
                        bad += 1;
                    }
                }
            }
            println!("{name}: {bad} of {total} blocks an octave off");
            assert!(total > 100 && bad == 0, "{name}: {bad} of {total} blocks an octave off");
        }
    }

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
        // (the switch waits until the jump has stopped sliding: settled 150 ms after it)
        let (hmax2, havg2) = dev(&h, lat, 1.65, 2.9, 60.0, 0.0);
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
                let limit = 0.09 + tol_ms as f64 / 1000.0 + retune as f64 / 1000.0 + if keep { 0.03 } else { 0.0 };
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


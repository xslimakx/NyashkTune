//! NyashkTune - realtime vocal pitch correction (VST3).

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub mod engine;
pub mod formant;
mod editor;

// ------------------------------------------------------------------------------------------
// Enums shown in the host / GUI
// ------------------------------------------------------------------------------------------

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Key {
    #[name = "C"]
    C,
    #[name = "C# / Db"]
    Cs,
    #[name = "D"]
    D,
    #[name = "D# / Eb"]
    Ds,
    #[name = "E"]
    E,
    #[name = "F"]
    F,
    #[name = "F# / Gb"]
    Fs,
    #[name = "G"]
    G,
    #[name = "G# / Ab"]
    Gs,
    #[name = "A"]
    A,
    #[name = "A# / Bb"]
    As,
    #[name = "B"]
    B,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum ScaleKind {
    #[name = "Chromatic"]
    Chromatic,
    #[name = "Major"]
    Major,
    #[name = "Natural Minor"]
    NaturalMinor,
    #[name = "Harmonic Minor"]
    HarmonicMinor,
    #[name = "Melodic Minor"]
    MelodicMinor,
    #[name = "Major Pentatonic"]
    MajorPent,
    #[name = "Minor Pentatonic"]
    MinorPent,
    #[name = "Blues"]
    Blues,
    #[name = "Dorian"]
    Dorian,
    #[name = "Phrygian"]
    Phrygian,
    #[name = "Lydian"]
    Lydian,
    #[name = "Mixolydian"]
    Mixolydian,
    #[name = "Locrian"]
    Locrian,
    #[name = "Whole Tone"]
    WholeTone,
    #[name = "Diminished"]
    Diminished,
    #[name = "Arabic (Hijaz)"]
    Hijaz,
    #[name = "Custom"]
    Custom,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum VoiceRange {
    #[name = "Generic"]
    Generic,
    #[name = "Bass"]
    Bass,
    #[name = "Baritone"]
    Baritone,
    #[name = "Tenor"]
    Tenor,
    #[name = "Alto"]
    Alto,
    #[name = "Mezzo-Soprano"]
    Mezzo,
    #[name = "Soprano"]
    Soprano,
}

impl VoiceRange {
    /// Correction range (MIDI notes) of each voice type.
    pub fn notes(self) -> (i32, i32) {
        match self {
            VoiceRange::Generic => (36, 84),  // C2 .. C6
            VoiceRange::Bass => (36, 64),     // C2 .. E4
            VoiceRange::Baritone => (40, 67), // E2 .. G4
            VoiceRange::Tenor => (45, 72),    // A2 .. C5
            VoiceRange::Alto => (50, 77),     // D3 .. F5
            VoiceRange::Mezzo => (53, 81),    // F3 .. A5
            VoiceRange::Soprano => (57, 86),  // A3 .. D6
        }
    }
}

/// What a key of the keyboard does (clicking the strip above it cycles the states).
#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum NoteState {
    /// as the Root / Scale say (legal in the scale, illegal outside)
    #[name = "Scale"]
    Scale,
    #[name = "Legal"]
    Legal,
    /// corrected to the nearest legal note
    #[name = "Illegal"]
    Illegal,
    /// corrected up to the next legal note
    #[name = "Illegal - Pitch Up"]
    Up,
    /// corrected down to the next legal note
    #[name = "Illegal - Pitch Down"]
    Down,
    /// legal, but left as it is sung
    #[name = "Bypass"]
    Bypass,
}

// ------------------------------------------------------------------------------------------
// Parameters
// ------------------------------------------------------------------------------------------

#[derive(Params)]
pub struct NoteParam {
    #[id = "st"]
    pub state: EnumParam<NoteState>,
}

/// 0.1 .. 800 ms, close to logarithmic
fn ms_range() -> FloatRange {
    FloatRange::Skewed { min: 0.1, max: 800.0, factor: FloatRange::skew_factor(-2.6) }
}

fn ms_text() -> Arc<dyn Fn(f32) -> String + Send + Sync> {
    Arc::new(|v| if v < 9.95 { format!("{:.1}", v) } else { format!("{:.0}", v) })
}

#[derive(Params)]
pub struct NyashkTuneParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// GUI naming: show flats (Db, Eb ...) instead of sharps; not automatable
    #[persist = "flats"]
    pub flats: AtomicU32,

    #[id = "bypass"]
    pub bypass: BoolParam,

    // correction
    #[id = "retune"]
    pub speed: FloatParam,
    #[id = "transition"]
    pub transition: FloatParam,
    #[id = "link"]
    pub link: BoolParam,
    #[id = "tolc"]
    pub tol_cents: FloatParam,
    #[id = "tolt"]
    pub tol_time: FloatParam,
    #[id = "vibkeep"]
    pub vibrato: BoolParam,
    #[id = "vibamt"]
    pub vib_depth: FloatParam,
    #[id = "corron"]
    pub correction: BoolParam,
    #[id = "amount"]
    pub amount: FloatParam,
    #[id = "fcorr"]
    pub formant: BoolParam,

    // global
    #[id = "key"]
    pub key: EnumParam<Key>,
    #[id = "scale"]
    pub scale: EnumParam<ScaleKind>,
    #[nested(array, group = "Notes")]
    pub notes: [NoteParam; 12],
    #[id = "refa"]
    pub ref_a: FloatParam,
    #[id = "range"]
    pub range: EnumParam<VoiceRange>,
    #[id = "rlo"]
    pub range_lo: IntParam,
    #[id = "rhi"]
    pub range_hi: IntParam,
    #[id = "midimode"]
    pub midi_mode: BoolParam,
}

fn lin(min: f32, max: f32) -> FloatRange {
    FloatRange::Linear { min, max }
}

fn midi_note_text() -> Arc<dyn Fn(i32) -> String + Send + Sync> {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    Arc::new(|m| format!("{}{}", N[m.rem_euclid(12) as usize], m.div_euclid(12) - 1))
}

fn midi_note_parse() -> Arc<dyn Fn(&str) -> Option<i32> + Send + Sync> {
    Arc::new(|s| {
        let s = s.trim();
        if let Ok(n) = s.parse::<i32>() {
            return Some(n);
        }
        let up = s.to_uppercase().replace('♯', "#").replace('♭', "B");
        let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let flats = ["C", "DB", "D", "EB", "E", "F", "GB", "G", "AB", "A", "BB", "B"];
        for len in [2usize, 1] {
            if up.len() > len {
                let (n, o) = up.split_at(len);
                let pc = names.iter().position(|x| *x == n).or_else(|| flats.iter().position(|x| *x == n));
                if let (Some(pc), Ok(oct)) = (pc, o.parse::<i32>()) {
                    return Some(pc as i32 + 12 * (oct + 1));
                }
            }
        }
        None
    })
}

impl Default for NoteParam {
    fn default() -> Self {
        Self { state: EnumParam::new("State", NoteState::Scale) }
    }
}

impl Default for NyashkTuneParams {
    fn default() -> Self {
        const NOTE_NAMES: [&str; 12] = ["C", "C#/Db", "D", "D#/Eb", "E", "F", "F#/Gb", "G", "G#/Ab", "A", "A#/Bb", "B"];
        let notes: [NoteParam; 12] = std::array::from_fn(|i| NoteParam { state: EnumParam::new(format!("Note {}", NOTE_NAMES[i]), NoteState::Scale) });
        let (lo, hi) = VoiceRange::Generic.notes();
        let off_or = |unit: &'static str| -> Arc<dyn Fn(f32) -> String + Send + Sync> {
            Arc::new(move |v| if v < 0.5 { "Off".to_string() } else { format!("{:.0}{unit}", v) })
        };
        let off_parse = || -> Arc<dyn Fn(&str) -> Option<f32> + Send + Sync> {
            Arc::new(|s| {
                let t = s.trim().to_lowercase();
                if t == "off" {
                    return Some(0.0);
                }
                t.trim_end_matches("ms").trim_end_matches('c').trim().parse::<f32>().ok()
            })
        };
        Self {
            editor_state: EguiState::from_size(940, 600),
            flats: AtomicU32::new(0),

            bypass: BoolParam::new("Bypass", false).make_bypass(),

            speed: FloatParam::new("Speed", 15.0, ms_range()).with_unit(" ms").with_step_size(0.1).with_value_to_string(ms_text()),
            transition: FloatParam::new("Note Transition", 120.0, ms_range()).with_unit(" ms").with_step_size(0.1).with_value_to_string(ms_text()),
            link: BoolParam::new("Link", false),
            tol_cents: FloatParam::new("Tolerance Cents", 0.0, lin(0.0, 40.0))
                .with_step_size(1.0)
                .with_value_to_string(off_or(" c"))
                .with_string_to_value(off_parse()),
            tol_time: FloatParam::new("Tolerance Time", 0.0, lin(0.0, 300.0))
                .with_step_size(1.0)
                .with_value_to_string(off_or(" ms"))
                .with_string_to_value(off_parse()),
            vibrato: BoolParam::new("Vibrato", true),
            vib_depth: FloatParam::new("Vibrato Depth", 100.0, lin(0.0, 200.0)).with_unit(" %").with_step_size(1.0),
            correction: BoolParam::new("Correction", true),
            amount: FloatParam::new("Correction Amount", 100.0, lin(1.0, 100.0)).with_unit(" %").with_step_size(1.0),
            formant: BoolParam::new("Formant Correction", true),

            key: EnumParam::new("Root", Key::C),
            scale: EnumParam::new("Scale", ScaleKind::Chromatic),
            notes,
            ref_a: FloatParam::new("Reference Frequency", 440.0, lin(415.3, 466.16)).with_unit(" Hz").with_step_size(0.01),
            range: EnumParam::new("Voice Range", VoiceRange::Generic),
            range_lo: IntParam::new("Range Low", lo, IntRange::Linear { min: 24, max: 96 }).with_value_to_string(midi_note_text()).with_string_to_value(midi_note_parse()),
            range_hi: IntParam::new("Range High", hi, IntRange::Linear { min: 24, max: 96 }).with_value_to_string(midi_note_text()).with_string_to_value(midi_note_parse()),
            midi_mode: BoolParam::new("MIDI Target", false),
        }
    }
}

impl NyashkTuneParams {
    /// legal pitch classes of Root + Scale ("Custom" = user scale: starts with all notes illegal)
    pub fn scale_mask(&self) -> u16 {
        engine::allowed_mask(self.scale.value() as usize, self.key.value() as usize, 0)
    }

    /// Effective state of pitch class `pc` (Scale resolved).
    pub fn note_state(&self, pc: usize) -> NoteState {
        match self.notes[pc].state.value() {
            NoteState::Scale => {
                if (self.scale_mask() >> pc) & 1 == 1 {
                    NoteState::Legal
                } else {
                    NoteState::Illegal
                }
            }
            s => s,
        }
    }

    /// true when a key was changed by hand (the scale name then shows an asterisk)
    pub fn scale_modified(&self) -> bool {
        self.notes.iter().any(|n| n.state.value() != NoteState::Scale)
    }

    /// lowest fundamental searched for: a little under the range
    pub fn lowest_hz(&self) -> f32 {
        let lo = self.range_lo.value().min(self.range_hi.value()) as f32 - 3.0;
        (440.0 * 2f32.powf((lo - 69.0) / 12.0)).clamp(50.0, 400.0)
    }

    pub fn engine_params(&self) -> engine::Params {
        let mut note_override = [0u8; 12];
        for (pc, n) in self.notes.iter().enumerate() {
            note_override[pc] = match n.state.value() {
                NoteState::Scale => 0,
                NoteState::Legal => 1,
                NoteState::Illegal => 2,
                NoteState::Up => 3,
                NoteState::Down => 4,
                NoteState::Bypass => 5,
            };
        }
        let (lo, hi) = (self.range_lo.value().min(self.range_hi.value()), self.range_lo.value().max(self.range_hi.value()));
        engine::Params {
            bypass: self.bypass.value(),
            key: self.key.value() as usize,
            scale: self.scale.value() as usize,
            custom_mask: 0,
            ref_a: self.ref_a.value(),
            retune_ms: self.speed.value(),
            transition_ms: if self.link.value() { self.speed.value() } else { self.transition.value() },
            amount: if self.correction.value() { self.amount.value() / 100.0 } else { 0.0 },
            note_override,
            tol_cents: self.tol_cents.value(),
            tol_ms: self.tol_time.value(),
            keep_vibrato: self.vibrato.value(),
            vib_amount: self.vib_depth.value() / 100.0,
            range_lo: lo as f32 - 0.5,
            range_hi: hi as f32 + 0.5,
            formant_preserve: self.formant.value(),
            formant_follow: 1.0,
            midi_mode: self.midi_mode.value(),
            min_hz: self.lowest_hz(),
            // search at least up to ~C6: singing above the range is detected (and passed as it is)
            // instead of being taken an octave down
            max_hz: (440.0 * 2f32.powf((hi as f32 + 5.0 - 69.0) / 12.0)).max(1100.0),
            voices_on: false,
            colour_on: false,
            vib_on: false,
            ..engine::Params::default()
        }
    }
}

// ------------------------------------------------------------------------------------------
// Meters shared with the GUI
// ------------------------------------------------------------------------------------------

pub struct Meters {
    pub voiced: AtomicU32,
    pub hz: AtomicF32,
    pub target: AtomicF32,
    pub corr: AtomicF32,
    /// block peaks (linear), the GUI lets them fall
    pub in_pk: AtomicF32,
    pub out_pk: AtomicF32,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            voiced: AtomicU32::new(0),
            hz: AtomicF32::new(0.0),
            target: AtomicF32::new(0.0),
            corr: AtomicF32::new(0.0),
            in_pk: AtomicF32::new(0.0),
            out_pk: AtomicF32::new(0.0),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Plugin
// ------------------------------------------------------------------------------------------

pub struct NyashkTune {
    params: Arc<NyashkTuneParams>,
    meters: Arc<Meters>,
    engine: Option<engine::Engine>,
    cur_min_hz: f32,
}

impl Default for NyashkTune {
    fn default() -> Self {
        Self {
            params: Arc::new(NyashkTuneParams::default()),
            meters: Arc::new(Meters::default()),
            engine: None,
            cur_min_hz: 80.0,
        }
    }
}

impl Plugin for NyashkTune {
    const NAME: &'static str = "NyashkTune";
    const VENDOR: &'static str = "Nyashk";
    const URL: &'static str = "https://github.com/xslimakx/NyashkTune";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        static HOOK: std::sync::Once = std::sync::Once::new();
        HOOK.call_once(|| {
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                baseview::nylog(&format!("PANIC: {info}\n{}", std::backtrace::Backtrace::force_capture()));
                prev(info);
            }));
        });
        baseview::nylog("editor() requested by host");
        editor::create(self.params.clone(), self.meters.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.cur_min_hz = self.params.lowest_hz();
        let eng = engine::Engine::new(config.sample_rate as f64, self.cur_min_hz);
        context.set_latency_samples(eng.latency_samples());
        self.engine = Some(eng);
        true
    }

    fn reset(&mut self) {
        if let Some(e) = self.engine.as_mut() {
            e.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        demo_voice(buffer);
        let Some(eng) = self.engine.as_mut() else {
            return ProcessStatus::Normal;
        };

        let ep = self.params.engine_params();
        if (ep.min_hz - self.cur_min_hz).abs() > 0.5 {
            self.cur_min_hz = ep.min_hz;
            eng.set_lowest_hz(ep.min_hz);
            context.set_latency_samples(eng.latency_samples());
        }
        eng.set_params(&ep);

        let (mut ipk, mut opk) = (0.0f32, 0.0f32);
        let mut next_event = context.next_event();
        for (sample_id, mut channels) in buffer.iter_samples().enumerate() {
            while let Some(event) = next_event {
                if event.timing() > sample_id as u32 {
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } => eng.held[(note & 127) as usize] = velocity > 0.0,
                    NoteEvent::NoteOff { note, .. } => eng.held[(note & 127) as usize] = false,
                    _ => {}
                }
                next_event = context.next_event();
            }

            let nch = channels.len().min(engine::MAX_CH);
            let mut inp = [0.0f32; engine::MAX_CH];
            for (i, s) in channels.iter_mut().enumerate().take(nch) {
                inp[i] = *s;
                ipk = ipk.max(s.abs());
            }
            let out = eng.process_frame(inp, nch);
            for (i, s) in channels.iter_mut().enumerate().take(nch) {
                *s = out[i];
                opk = opk.max(s.abs());
            }
        }

        // publish meters for the GUI
        let m = &eng.meters;
        self.meters.voiced.store(m.voiced as u32, Ordering::Relaxed);
        self.meters.hz.store(m.hz, Ordering::Relaxed);
        self.meters.target.store(m.target, Ordering::Relaxed);
        self.meters.corr.store(m.corr, Ordering::Relaxed);
        self.meters.in_pk.store(self.meters.in_pk.load(Ordering::Relaxed).max(ipk), Ordering::Relaxed);
        self.meters.out_pk.store(self.meters.out_pk.load(Ordering::Relaxed).max(opk), Ordering::Relaxed);

        ProcessStatus::Normal
    }
}

impl Vst3Plugin for NyashkTune {
    const VST3_CLASS_ID: [u8; 16] = *b"NyashkTuneVST3!!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nih_export_vst3!(NyashkTune);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_values_parse() {
        let p = NyashkTuneParams::default();
        let n = p.speed.string_to_normalized_value("100").unwrap();
        assert!((p.speed.preview_plain(n) - 100.0).abs() < 0.5);
        let n = p.speed.string_to_normalized_value("100 ms").unwrap();
        assert!((p.speed.preview_plain(n) - 100.0).abs() < 0.5);
        let n = p.tol_time.string_to_normalized_value("Off").unwrap();
        assert_eq!(p.tol_time.preview_plain(n), 0.0);
        let n = p.tol_cents.string_to_normalized_value("20 c").unwrap();
        assert!((p.tol_cents.preview_plain(n) - 20.0).abs() < 0.5);
        let n = p.ref_a.string_to_normalized_value("432").unwrap();
        assert!((p.ref_a.preview_plain(n) - 432.0).abs() < 0.05);
        let n = p.range_lo.string_to_normalized_value("C2").unwrap();
        assert_eq!(p.range_lo.preview_plain(n), 36);
        let n = p.range_hi.string_to_normalized_value("F#4").unwrap();
        assert_eq!(p.range_hi.preview_plain(n), 66);
    }

    #[test]
    fn defaults_are_waves_like() {
        let p = NyashkTuneParams::default();
        assert!((p.speed.value() - 15.0).abs() < 0.01);
        assert!((p.transition.value() - 120.0).abs() < 0.01);
        assert!(p.vibrato.value() && (p.vib_depth.value() - 100.0).abs() < 0.01);
        let e = p.engine_params();
        assert!(!e.voices_on && !e.colour_on && !e.vib_on);
        assert_eq!(e.amount, 1.0);
    }
}

/// Developer helper for screenshots: with `NYASHK_DEMO=1` the standalone build sings a short melody
/// (vowel with vibrato, slightly out of tune) instead of using the input.
fn demo_voice(buffer: &mut Buffer) {
    use std::sync::Mutex;
    static ST: Mutex<Option<(bool, u64, f64, f64, f64)>> = Mutex::new(None);
    let mut g = ST.lock().unwrap();
    let st = g.get_or_insert_with(|| (std::env::var("NYASHK_DEMO").is_ok(), 0, 0.0, 0.0, 0.0));
    if !st.0 {
        return;
    }
    let fs = 48000.0f64;
    let melody = [57.0f64, 60.0, 62.0, 60.0, 64.0, 62.0, 60.0, 57.0];
    for mut frame in buffer.iter_samples() {
        let t = st.1 as f64 / fs;
        st.1 += 1;
        let k = (t / 0.6) as usize;
        let gap = (t / 0.6).fract() > 0.88;
        let m = melody[k % melody.len()] + 0.3 + 0.35 * (2.0 * std::f64::consts::PI * 5.5 * t).sin();
        let f0 = 440.0 * 2f64.powf((m - 69.0) / 12.0);
        st.2 = (st.2 + f0 / fs) % 1.0;
        let src = if st.2 < 0.05 { 1.0 } else { -0.05 };
        let (r, th) = (0.985, 2.0 * std::f64::consts::PI * 750.0 / fs);
        let y = src * 0.1 + 2.0 * r * th.cos() * st.3 - r * r * st.4;
        st.4 = st.3;
        st.3 = y;
        let v = if gap { 0.0 } else { (y * 0.15) as f32 };
        for s in frame.iter_mut() {
            *s = v;
        }
    }
}

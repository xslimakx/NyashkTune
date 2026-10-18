//! NyashkTune - realtime vocal pitch correction (VST3).

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub mod engine;
mod editor;

// ------------------------------------------------------------------------------------------
// Enums shown in the host / GUI
// ------------------------------------------------------------------------------------------

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum Key {
    #[name = "C"]
    C,
    #[name = "C#"]
    Cs,
    #[name = "D"]
    D,
    #[name = "D#"]
    Ds,
    #[name = "E"]
    E,
    #[name = "F"]
    F,
    #[name = "F#"]
    Fs,
    #[name = "G"]
    G,
    #[name = "G#"]
    Gs,
    #[name = "A"]
    A,
    #[name = "A#"]
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
    #[name = "Mezzo-Sop."]
    Mezzo,
    #[name = "Soprano"]
    Soprano,
}

// ------------------------------------------------------------------------------------------
// Parameters
// ------------------------------------------------------------------------------------------

#[derive(Params)]
pub struct NoteParam {
    #[id = "on"]
    pub on: BoolParam,
}

#[derive(Params)]
pub struct NyashkTuneParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    #[persist = "preset"]
    pub preset: AtomicU32,

    #[id = "bypass"]
    pub bypass: BoolParam,
    #[id = "key"]
    pub key: EnumParam<Key>,
    #[id = "scale"]
    pub scale: EnumParam<ScaleKind>,
    #[nested(array, group = "Custom Notes")]
    pub notes: [NoteParam; 12],
    #[id = "refa"]
    pub ref_a: FloatParam,

    #[id = "retune"]
    pub retune: FloatParam,
    #[id = "transition"]
    pub transition: FloatParam,
    #[id = "link"]
    pub link: BoolParam,
    #[id = "amount"]
    pub amount: FloatParam,
    #[id = "flex"]
    pub flex: FloatParam,
    #[id = "humanize"]
    pub humanize: FloatParam,
    #[id = "natvib"]
    pub nat_vib: FloatParam,

    #[id = "transpose"]
    pub transpose: FloatParam,
    #[id = "fine"]
    pub fine: FloatParam,
    #[id = "formant"]
    pub formant: FloatParam,
    #[id = "keepformants"]
    pub keep_formants: BoolParam,

    #[id = "viboon"]
    pub vib_on: BoolParam,
    #[id = "vibdepth"]
    pub vib_depth: FloatParam,
    #[id = "vibrate"]
    pub vib_rate: FloatParam,
    #[id = "vibdelay"]
    pub vib_delay: FloatParam,

    #[id = "midimode"]
    pub midi_mode: BoolParam,
    #[id = "range"]
    pub range: EnumParam<VoiceRange>,
    #[id = "gate"]
    pub gate: FloatParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "ingain"]
    pub in_gain: FloatParam,
    #[id = "outgain"]
    pub out_gain: FloatParam,
}

fn lin(min: f32, max: f32) -> FloatRange {
    FloatRange::Linear { min, max }
}

impl Default for NoteParam {
    fn default() -> Self {
        Self { on: BoolParam::new("On", true) }
    }
}

impl Default for NyashkTuneParams {
    fn default() -> Self {
        const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        let notes: [NoteParam; 12] = std::array::from_fn(|i| NoteParam {
            on: BoolParam::new(format!("Custom Note {}", NOTE_NAMES[i]), true),
        });
        Self {
            editor_state: EguiState::from_size(900, 640),
            preset: AtomicU32::new(0),

            bypass: BoolParam::new("Bypass", false).make_bypass(),
            key: EnumParam::new("Key", Key::C),
            scale: EnumParam::new("Scale", ScaleKind::Major),
            notes,
            ref_a: FloatParam::new("Reference A4", 440.0, lin(415.0, 466.0))
                .with_unit(" Hz")
                .with_step_size(0.1),

            retune: FloatParam::new(
                "Retune Speed",
                50.0,
                FloatRange::Skewed { min: 0.0, max: 400.0, factor: FloatRange::skew_factor(-1.6) },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
            transition: FloatParam::new(
                "Note Transition",
                50.0,
                FloatRange::Skewed { min: 0.0, max: 400.0, factor: FloatRange::skew_factor(-1.6) },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
            link: BoolParam::new("Link Speed/Transition", false),
            amount: FloatParam::new("Amount", 100.0, lin(0.0, 100.0)).with_unit(" %").with_step_size(0.1),
            flex: FloatParam::new("Flex-Tune", 0.0, lin(0.0, 100.0)).with_unit(" c").with_step_size(0.1),
            humanize: FloatParam::new("Humanize", 0.0, lin(0.0, 100.0)).with_unit(" %").with_step_size(0.1),
            nat_vib: FloatParam::new("Natural Vibrato", 0.0, lin(0.0, 100.0)).with_unit(" %").with_step_size(0.1),

            transpose: FloatParam::new("Transpose", 0.0, lin(-12.0, 12.0)).with_unit(" st").with_step_size(1.0),
            fine: FloatParam::new("Detune", 0.0, lin(-100.0, 100.0)).with_unit(" c").with_step_size(0.1),
            formant: FloatParam::new("Formant Shift", 0.0, lin(-12.0, 12.0)).with_unit(" st").with_step_size(0.1),
            keep_formants: BoolParam::new("Keep Formants", true),

            vib_on: BoolParam::new("Vibrato On", false),
            vib_depth: FloatParam::new("Vibrato Depth", 30.0, lin(0.0, 100.0)).with_unit(" c").with_step_size(0.1),
            vib_rate: FloatParam::new("Vibrato Rate", 5.5, lin(2.0, 9.0)).with_unit(" Hz").with_step_size(0.01),
            vib_delay: FloatParam::new("Vibrato Delay", 250.0, lin(0.0, 1000.0)).with_unit(" ms").with_step_size(1.0),

            midi_mode: BoolParam::new("MIDI Target", false),
            range: EnumParam::new("Voice Range", VoiceRange::Generic),
            gate: FloatParam::new("Input Gate", -60.0, lin(-90.0, -20.0)).with_unit(" dB").with_step_size(0.1),
            mix: FloatParam::new("Mix", 100.0, lin(0.0, 100.0)).with_unit(" %").with_step_size(0.1),
            in_gain: FloatParam::new("Input Gain", 0.0, lin(-24.0, 24.0)).with_unit(" dB").with_step_size(0.1),
            out_gain: FloatParam::new("Output Gain", 0.0, lin(-24.0, 24.0)).with_unit(" dB").with_step_size(0.1),
        }
    }
}

impl NyashkTuneParams {
    pub fn custom_mask(&self) -> u16 {
        let mut m = 0u16;
        for (i, n) in self.notes.iter().enumerate() {
            if n.on.value() {
                m |= 1 << i;
            }
        }
        m
    }

    /// (lowest, highest) fundamental searched for, per voice type.
    pub fn range_limits(&self) -> (f32, f32) {
        match self.range.value() {
            VoiceRange::Generic => (70.0, 1000.0),
            VoiceRange::Bass => (60.0, 340.0),      // ~B1 .. F4
            VoiceRange::Baritone => (75.0, 420.0),  // ~D2 .. G#4
            VoiceRange::Tenor => (95.0, 560.0),     // ~G2 .. C#5
            VoiceRange::Alto => (120.0, 760.0),     // ~B2 .. F#5
            VoiceRange::Mezzo => (140.0, 920.0),    // ~C#3 .. A#5
            VoiceRange::Soprano => (180.0, 1200.0), // ~F#3 .. D6
        }
    }

    pub fn range_hz(&self) -> f32 {
        self.range_limits().0
    }

    pub fn allowed_mask(&self) -> u16 {
        engine::allowed_mask(self.scale.value() as usize, self.key.value() as usize, self.custom_mask())
    }

    pub fn engine_params(&self) -> engine::Params {
        engine::Params {
            bypass: self.bypass.value(),
            key: self.key.value() as usize,
            scale: self.scale.value() as usize,
            custom_mask: self.custom_mask(),
            ref_a: self.ref_a.value(),
            retune_ms: self.retune.value(),
            transition_ms: self.transition.value(),
            amount: self.amount.value() / 100.0,
            flex_cents: self.flex.value(),
            humanize: self.humanize.value() / 100.0,
            natural_vib: self.nat_vib.value() / 100.0,
            transpose: self.transpose.value(),
            fine_cents: self.fine.value(),
            formant_semis: self.formant.value(),
            formant_preserve: self.keep_formants.value(),
            vib_on: self.vib_on.value(),
            vib_depth_cents: self.vib_depth.value(),
            vib_rate_hz: self.vib_rate.value(),
            vib_delay_ms: self.vib_delay.value(),
            midi_mode: self.midi_mode.value(),
            min_hz: self.range_hz(),
            max_hz: self.range_limits().1,
            gate_db: self.gate.value(),
            mix: self.mix.value() / 100.0,
            in_gain_db: self.in_gain.value(),
            out_gain_db: self.out_gain.value(),
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
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            voiced: AtomicU32::new(0),
            hz: AtomicF32::new(0.0),
            target: AtomicF32::new(0.0),
            corr: AtomicF32::new(0.0),
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
    const VENDOR: &'static str = "NyashkTune";
    const URL: &'static str = "https://github.com/xslimakx/nyashk-plugins";
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
        self.cur_min_hz = self.params.range_hz();
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
            }
            let out = eng.process_frame(inp, nch);
            for (i, s) in channels.iter_mut().enumerate().take(nch) {
                *s = out[i];
            }
        }

        // publish meters for the GUI
        let m = &eng.meters;
        self.meters.voiced.store(m.voiced as u32, Ordering::Relaxed);
        self.meters.hz.store(m.hz, Ordering::Relaxed);
        self.meters.target.store(m.target, Ordering::Relaxed);
        self.meters.corr.store(m.corr, Ordering::Relaxed);

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
        let n = p.retune.string_to_normalized_value("100").unwrap();
        assert!((p.retune.preview_plain(n) - 100.0).abs() < 0.5);
        let n = p.retune.string_to_normalized_value("100 ms").unwrap();
        assert!((p.retune.preview_plain(n) - 100.0).abs() < 0.5);
        let n = p.gate.string_to_normalized_value("-40").unwrap();
        assert!((p.gate.preview_plain(n) + 40.0).abs() < 0.5);
        let n = p.transpose.string_to_normalized_value("-5").unwrap();
        assert!((p.transpose.preview_plain(n) + 5.0).abs() < 0.1);
        let n = p.ref_a.string_to_normalized_value("432").unwrap();
        assert!((p.ref_a.preview_plain(n) - 432.0).abs() < 0.5);
    }
}

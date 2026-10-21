//! NyashkTune GUI: black / white / grey, hard edges, mono type, numbered sections.

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, epaint::PathStroke, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextFormat, Ui, UiBuilder, Vec2},
};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{engine, FormantAlgo, Key, Meters, NyashkTuneParams, ScaleKind, VibShape, VoiceRange};

// palette
const BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
const PANEL: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
const PANEL2: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
const LINE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
const WHITE: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
const DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
const MID: Color32 = Color32::from_rgb(0x7a, 0x7a, 0x7a);

const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const HIST: usize = 220;
const BASE_W: f32 = 900.0;
const BASE_H: f32 = 670.0;

pub const PRESETS: [&str; 28] = [
    "Init (Major, 50 ms)",
    "Natural Correction",
    "Transparent Pop",
    "Smooth R&B",
    "Hard Tune (T-Pain)",
    "Hard Tune Major",
    "Minor Pentatonic Rap",
    "Robot Voice",
    "Gender Up",
    "Gender Down",
    "Add Vibrato",
    "MIDI Target",
    "Anime Vocal",
    "Anime Vocal - Soft",
    "Anime Vocal - Hyper",
    "Anime Idol (Duet Layers)",
    "Chipmunk",
    "Giant / Demon",
    "Robot Choir",
    "Octave Doubler",
    "Thirds Harmony",
    "Gospel Choir",
    "Stereo Double (ADT)",
    "Alien Throat",
    "Wobbly Vibrato",
    "Telephone Singer",
    "Whisper Child",
    "Formant Lab (LPC +3)",
];

struct Gui {
    det: VecDeque<f32>,
    cor: VecDeque<f32>,
    last_push: Instant,
    centre: f32,
    last_det: f32,
    last_target: f32,
    voiced: bool,
    tab: usize,
}

fn midi_name(m: i32) -> String {
    format!("{}{}", NOTE_NAMES[m.rem_euclid(12) as usize], m.div_euclid(12) - 1)
}

fn is_black(pc: usize) -> bool {
    matches!(pc, 1 | 3 | 6 | 8 | 10)
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

// ------------------------------------------------------------------------------------------
// Parameter helpers
// ------------------------------------------------------------------------------------------

fn set_norm<P: Param>(setter: &ParamSetter, p: &P, v: f32) {
    setter.begin_set_parameter(p);
    setter.set_parameter_normalized(p, v.clamp(0.0, 1.0));
    setter.end_set_parameter(p);
}
fn set_f(setter: &ParamSetter, p: &FloatParam, v: f32) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn set_b(setter: &ParamSetter, p: &BoolParam, v: bool) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn set_e<E: Enum + PartialEq>(setter: &ParamSetter, p: &EnumParam<E>, v: E) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}
fn reset<P: Param>(setter: &ParamSetter, p: &P) {
    set_norm(setter, p, p.default_normalized_value());
}

#[allow(clippy::too_many_arguments)]
fn voice(setter: &ParamSetter, p: &NyashkTuneParams, i: usize, interval: f32, level: f32, pan: f32, detune: f32, formant: f32, delay: f32, snap: bool) {
    let v = &p.voices[i];
    set_b(setter, &v.on, true);
    set_f(setter, &v.interval, interval);
    set_f(setter, &v.level, level);
    set_f(setter, &v.pan, pan);
    set_f(setter, &v.detune, detune);
    set_f(setter, &v.formant, formant);
    set_f(setter, &v.delay, delay);
    set_b(setter, &v.snap, snap);
}

fn apply_preset(setter: &ParamSetter, p: &NyashkTuneParams, idx: usize) {
    // everything back to default except bypass and voice range
    reset(setter, &p.key);
    reset(setter, &p.scale);
    for n in &p.notes {
        reset(setter, &n.on);
    }
    p.note_bypass.store(0, Ordering::Relaxed);
    reset(setter, &p.ref_a);
    reset(setter, &p.retune);
    reset(setter, &p.transition);
    reset(setter, &p.amount);
    reset(setter, &p.flex);
    reset(setter, &p.humanize);
    reset(setter, &p.nat_vib);
    reset(setter, &p.transpose);
    reset(setter, &p.fine);
    reset(setter, &p.formant);
    reset(setter, &p.keep_formants);
    reset(setter, &p.fmode);
    reset(setter, &p.ftilt);
    reset(setter, &p.ffollow);
    reset(setter, &p.tracking);
    reset(setter, &p.vib_shape);
    reset(setter, &p.air);
    reset(setter, &p.drive);
    for v in &p.voices {
        reset(setter, &v.on);
        reset(setter, &v.interval);
        reset(setter, &v.snap);
        reset(setter, &v.level);
        reset(setter, &v.pan);
        reset(setter, &v.detune);
        reset(setter, &v.formant);
        reset(setter, &v.delay);
    }
    reset(setter, &p.vib_on);
    reset(setter, &p.vib_depth);
    reset(setter, &p.vib_rate);
    reset(setter, &p.vib_delay);
    reset(setter, &p.midi_mode);
    reset(setter, &p.gate);
    reset(setter, &p.mix);
    reset(setter, &p.in_gain);
    reset(setter, &p.out_gain);

    let chroma = |setter: &ParamSetter| set_e(setter, &p.scale, ScaleKind::Chromatic);
    match idx {
        1 => {
            chroma(setter);
            set_f(setter, &p.retune, 90.0);
            set_f(setter, &p.transition, 130.0);
            set_f(setter, &p.amount, 80.0);
            set_f(setter, &p.nat_vib, 100.0);
            set_f(setter, &p.humanize, 40.0);
            set_f(setter, &p.flex, 15.0);
        }
        2 => {
            set_f(setter, &p.retune, 35.0);
            set_f(setter, &p.transition, 35.0);
            set_f(setter, &p.nat_vib, 60.0);
            set_f(setter, &p.humanize, 20.0);
        }
        3 => {
            set_f(setter, &p.retune, 120.0);
            set_f(setter, &p.transition, 160.0);
            set_f(setter, &p.amount, 90.0);
            set_f(setter, &p.nat_vib, 100.0);
            set_f(setter, &p.humanize, 60.0);
        }
        4 => {
            chroma(setter);
            set_f(setter, &p.retune, 0.0);
            set_f(setter, &p.transition, 0.0);
        }
        5 => { set_f(setter, &p.retune, 0.0); set_f(setter, &p.transition, 0.0); }
        6 => {
            set_e(setter, &p.scale, ScaleKind::MinorPent);
            set_e(setter, &p.key, Key::A);
            set_f(setter, &p.retune, 8.0);
            set_f(setter, &p.transition, 8.0);
        }
        7 => {
            chroma(setter);
            set_f(setter, &p.retune, 0.0);
            set_f(setter, &p.transition, 0.0);
            set_f(setter, &p.formant, -2.0);
        }
        8 => {
            chroma(setter);
            set_f(setter, &p.retune, 60.0);
            set_f(setter, &p.transition, 60.0);
            set_f(setter, &p.formant, 4.0);
            set_f(setter, &p.transpose, 3.0);
        }
        9 => {
            chroma(setter);
            set_f(setter, &p.retune, 60.0);
            set_f(setter, &p.transition, 60.0);
            set_f(setter, &p.formant, -4.0);
            set_f(setter, &p.transpose, -3.0);
        }
        10 => {
            chroma(setter);
            set_f(setter, &p.retune, 150.0);
            set_f(setter, &p.transition, 200.0);
            set_f(setter, &p.nat_vib, 100.0);
            set_b(setter, &p.vib_on, true);
            set_f(setter, &p.vib_depth, 35.0);
        }
        11 => {
            set_b(setter, &p.midi_mode, true);
            set_f(setter, &p.retune, 15.0);
            set_f(setter, &p.transition, 15.0);
        }
        // ---- anime vocal family: bright, small-vocal-tract formants, fast hard-ish tune, airy top,
        // a shimmer an octave up and a slightly detuned double. Set KEY/SCALE (or DETECT KEY) for the song.
        12 => {
            chroma(setter);
            set_f(setter, &p.retune, 6.0);
            set_f(setter, &p.transition, 22.0);
            set_f(setter, &p.humanize, 0.0);
            set_f(setter, &p.transpose, 2.0);
            set_e(setter, &p.fmode, FormantAlgo::Detailed);
            set_f(setter, &p.formant, 5.0);
            set_f(setter, &p.ftilt, 40.0);
            set_f(setter, &p.air, 5.0);
            set_f(setter, &p.drive, 12.0);
            set_f(setter, &p.tracking, 35.0);
            voice(setter, p, 0, 12.0, -14.0, -35.0, 4.0, 2.0, 6.0, true);
            voice(setter, p, 1, 0.0, -11.0, 45.0, 9.0, 0.5, 14.0, false);
        }
        13 => {
            chroma(setter);
            set_f(setter, &p.retune, 25.0);
            set_f(setter, &p.transition, 45.0);
            set_f(setter, &p.nat_vib, 40.0);
            set_f(setter, &p.transpose, 1.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            set_f(setter, &p.formant, 4.0);
            set_f(setter, &p.ftilt, 25.0);
            set_f(setter, &p.air, 3.5);
            voice(setter, p, 1, 0.0, -13.0, 40.0, 7.0, 0.0, 12.0, false);
        }
        14 => {
            chroma(setter);
            set_f(setter, &p.retune, 0.0);
            set_f(setter, &p.transition, 12.0);
            set_f(setter, &p.transpose, 5.0);
            set_e(setter, &p.fmode, FormantAlgo::Lpc);
            set_f(setter, &p.formant, 7.0);
            set_f(setter, &p.ftilt, 55.0);
            set_f(setter, &p.air, 8.0);
            set_f(setter, &p.drive, 25.0);
            set_f(setter, &p.tracking, 45.0);
            voice(setter, p, 0, 12.0, -9.0, -50.0, 5.0, 3.0, 5.0, true);
            voice(setter, p, 1, 7.0, -14.0, 50.0, -5.0, 2.0, 11.0, true);
        }
        15 => {
            chroma(setter);
            set_f(setter, &p.retune, 10.0);
            set_f(setter, &p.transition, 30.0);
            set_f(setter, &p.transpose, 2.0);
            set_e(setter, &p.fmode, FormantAlgo::Detailed);
            set_f(setter, &p.formant, 4.5);
            set_f(setter, &p.ftilt, 30.0);
            set_f(setter, &p.air, 4.0);
            set_f(setter, &p.drive, 8.0);
            voice(setter, p, 0, 4.0, -12.0, -60.0, 3.0, 3.0, 9.0, true);
            voice(setter, p, 1, 7.0, -13.0, 60.0, -3.0, 3.5, 17.0, true);
        }
        16 => {
            chroma(setter);
            set_f(setter, &p.retune, 20.0);
            set_f(setter, &p.transpose, 7.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            set_f(setter, &p.formant, 6.0);
            set_f(setter, &p.air, 3.0);
        }
        17 => {
            chroma(setter);
            set_f(setter, &p.retune, 30.0);
            set_f(setter, &p.transpose, -7.0);
            set_e(setter, &p.fmode, FormantAlgo::Lpc);
            set_f(setter, &p.formant, -5.0);
            set_f(setter, &p.ftilt, -30.0);
            set_f(setter, &p.air, -4.0);
            set_f(setter, &p.drive, 20.0);
            voice(setter, p, 0, -12.0, -8.0, 0.0, 6.0, -3.0, 0.0, false);
        }
        18 => {
            // choir of robots: hard-tuned lead plus a fixed major triad
            chroma(setter);
            set_f(setter, &p.retune, 0.0);
            set_f(setter, &p.transition, 0.0);
            set_e(setter, &p.fmode, FormantAlgo::Lpc);
            set_f(setter, &p.formant, -1.0);
            voice(setter, p, 0, 4.0, -7.0, -60.0, 0.0, 0.0, 0.0, false);
            voice(setter, p, 1, 7.0, -7.0, 60.0, 0.0, 0.0, 0.0, false);
        }
        19 => {
            set_f(setter, &p.retune, 30.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            voice(setter, p, 0, 12.0, -10.0, -30.0, 4.0, 1.0, 0.0, false);
            voice(setter, p, 1, -12.0, -9.0, 30.0, -4.0, -1.0, 0.0, false);
        }
        20 => {
            set_f(setter, &p.retune, 35.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            set_f(setter, &p.humanize, 25.0);
            voice(setter, p, 0, 4.0, -8.0, -55.0, 3.0, 0.8, 9.0, true);
            voice(setter, p, 1, 7.0, -9.0, 55.0, -3.0, -0.8, 15.0, true);
        }
        21 => {
            set_f(setter, &p.retune, 55.0);
            set_f(setter, &p.humanize, 40.0);
            set_f(setter, &p.nat_vib, 100.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            set_f(setter, &p.air, 2.0);
            voice(setter, p, 0, 4.0, -7.0, -65.0, 6.0, 1.0, 14.0, true);
            voice(setter, p, 1, -5.0, -8.0, 65.0, -6.0, -1.5, 24.0, true);
        }
        22 => {
            set_f(setter, &p.retune, 40.0);
            set_f(setter, &p.nat_vib, 80.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            voice(setter, p, 0, 0.0, -9.0, -80.0, 8.0, 0.3, 12.0, false);
            voice(setter, p, 1, 0.0, -9.0, 80.0, -8.0, -0.3, 22.0, false);
        }
        23 => {
            chroma(setter);
            set_f(setter, &p.retune, 15.0);
            set_e(setter, &p.fmode, FormantAlgo::Bend);
            set_f(setter, &p.formant, -4.0);
            set_f(setter, &p.ftilt, 70.0);
            set_e(setter, &p.vib_shape, VibShape::Wander);
            set_b(setter, &p.vib_on, true);
            set_f(setter, &p.vib_depth, 45.0);
            set_f(setter, &p.vib_rate, 6.5);
            set_f(setter, &p.vib_delay, 0.0);
            set_f(setter, &p.air, 3.0);
        }
        24 => {
            chroma(setter);
            set_f(setter, &p.retune, 120.0);
            set_f(setter, &p.transition, 180.0);
            set_f(setter, &p.nat_vib, 100.0);
            set_e(setter, &p.vib_shape, VibShape::Wander);
            set_b(setter, &p.vib_on, true);
            set_f(setter, &p.vib_depth, 60.0);
            set_f(setter, &p.vib_rate, 4.2);
            set_f(setter, &p.vib_delay, 0.0);
        }
        25 => {
            chroma(setter);
            set_f(setter, &p.retune, 20.0);
            set_e(setter, &p.fmode, FormantAlgo::Lpc);
            set_f(setter, &p.formant, 1.0);
            set_f(setter, &p.ftilt, -100.0);
            set_f(setter, &p.air, -6.0);
            set_f(setter, &p.drive, 55.0);
        }
        26 => {
            chroma(setter);
            set_f(setter, &p.retune, 35.0);
            set_f(setter, &p.transpose, 9.0);
            set_e(setter, &p.fmode, FormantAlgo::Smooth);
            set_f(setter, &p.formant, 7.0);
            set_f(setter, &p.ftilt, 20.0);
            set_f(setter, &p.air, 6.0);
            set_f(setter, &p.amount, 85.0);
        }
        27 => {
            set_f(setter, &p.retune, 40.0);
            set_e(setter, &p.fmode, FormantAlgo::Lpc);
            set_f(setter, &p.formant, 3.0);
        }
        _ => {}
    }
    p.preset.store(idx as u32, Ordering::Relaxed);
}

fn toggle_note(setter: &ParamSetter, p: &NyashkTuneParams, pc: usize) {
    let mut m = p.allowed_mask();
    m ^= 1 << pc;
    for (i, n) in p.notes.iter().enumerate() {
        set_b(setter, &n.on, (m >> i) & 1 == 1);
    }
    set_e(setter, &p.scale, ScaleKind::Custom);
}

// ------------------------------------------------------------------------------------------
// Widgets
// ------------------------------------------------------------------------------------------

/// What a knob did this frame. `total` is the unclamped displacement since the drag began, so a
/// linked partner can be driven from its own start value (no stale reads, no drift at the ends).
#[derive(Clone, Copy, Default)]
struct KnobEv {
    delta: f32,
    started: bool,
    total: Option<f32>,
}

fn knob<P: Param>(ui: &mut Ui, setter: &ParamSetter, p: &P, label: &str, cell: Rect, tooltip: &str, bipolar: bool) -> KnobEv {
    knob_id(ui, setter, p, label, label, cell, tooltip, bipolar)
}

#[allow(clippy::too_many_arguments)]
fn knob_id<P: Param>(ui: &mut Ui, setter: &ParamSetter, p: &P, label: &str, id: &str, cell: Rect, tooltip: &str, bipolar: bool) -> KnobEv {
    let r = 30.0;
    let c = Pos2::new(cell.center().x, cell.top() + r + 4.0);
    let hit = Rect::from_center_size(c, Vec2::splat(2.0 * r + 8.0));
    let resp = ui.allocate_rect(hit, Sense::click_and_drag());
    let resp = if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) };

    let norm = p.unmodulated_normalized_value();
    let mut delta = 0.0f32;
    let mut ev = KnobEv::default();
    let raw_id = egui::Id::new(("knob_raw", id));
    if resp.drag_started() {
        setter.begin_set_parameter(p);
        ui.data_mut(|d| d.insert_temp(raw_id, (norm, norm)));
        ev.started = true;
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = -resp.drag_delta().y / if fine { 1200.0 } else { 220.0 };
        let (raw0, raw) = ui.data(|dd| dd.get_temp::<(f32, f32)>(raw_id)).unwrap_or((norm, norm));
        let raw = raw + d;
        ui.data_mut(|dd| dd.insert_temp(raw_id, (raw0, raw)));
        let new = raw.clamp(0.0, 1.0);
        delta = new - norm;
        setter.set_parameter_normalized(p, new);
        ev.total = Some(raw - raw0);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(p);
    }
    if resp.double_clicked() {
        delta = p.default_normalized_value() - norm;
        set_norm(setter, p, p.default_normalized_value());
        ev.started = false;
    }
    if resp.hovered() {
        let sc = ui.input(|i| i.smooth_scroll_delta.y);
        if sc != 0.0 {
            let new = (norm + sc / 1500.0).clamp(0.0, 1.0);
            delta = new - norm;
            setter.begin_set_parameter(p);
            setter.set_parameter_normalized(p, new);
            setter.end_set_parameter(p);
        }
    }

    let norm = p.unmodulated_normalized_value();
    let painter = ui.painter();
    let a0 = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let ring = r - 2.5;
    let pt = |rad: f32, t: f32| -> Pos2 {
        let a = a0 + t * sweep;
        Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin())
    };
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        let n = (((to - from).abs() / sweep) * 120.0).ceil().max(2.0) as usize;
        (0..=n).map(|i| pt(ring, from + (to - from) * i as f32 / n as f32)).collect()
    };
    // track + value arc with round caps
    painter.add(egui::Shape::line(arc(0.0, 1.0), PathStroke::new(4.0_f32, LINE)));
    painter.circle_filled(pt(ring, 0.0), 2.0, LINE);
    painter.circle_filled(pt(ring, 1.0), 2.0, LINE);
    let from = if bipolar { 0.5 } else { 0.0 };
    if (norm - from).abs() > 0.002 {
        painter.add(egui::Shape::line(arc(from, norm), PathStroke::new(4.0_f32, WHITE)));
        painter.circle_filled(pt(ring, from), 2.0, WHITE);
        painter.circle_filled(pt(ring, norm), 2.0, WHITE);
    }
    // round knob body
    let body = r * 0.72;
    painter.circle_filled(c + Vec2::new(0.0, 1.5), body + 1.0, Color32::from_rgb(5, 5, 5));
    painter.circle_filled(c, body, Color32::from_rgb(34, 34, 34));
    painter.circle_filled(c, body * 0.84, Color32::from_rgb(24, 24, 24));
    painter.circle_stroke(c, body, Stroke::new(1.0_f32, Color32::from_rgb(70, 70, 70)));
    // pointer
    painter.line_segment([pt(body * 0.30, norm), pt(body * 0.80, norm)], Stroke::new(3.0_f32, WHITE));
    painter.circle_filled(pt(body * 0.80, norm), 1.5, WHITE);
    painter.circle_filled(pt(body * 0.30, norm), 1.5, WHITE);

    // value + label (click the value box to type a number)
    let val = p.normalized_value_to_string(norm, true);
    let vr = Rect::from_center_size(Pos2::new(cell.center().x, c.y + r + 14.0), Vec2::new(cell.width().min(84.0), 18.0));
    let edit_id = egui::Id::new(("knob_edit", id));
    let editing: Option<String> = ui.data(|d| d.get_temp::<String>(edit_id));
    painter_label(ui, cell, vr, label);
    if let Some(mut buf) = editing {
        let te = egui::TextEdit::singleline(&mut buf)
            .font(mono(11.0))
            .horizontal_align(egui::Align::Center)
            .desired_width(vr.width())
            .margin(egui::Margin::symmetric(2, 1));
        let r_te = ui.put(vr, te);
        if !r_te.has_focus() && !r_te.lost_focus() {
            r_te.request_focus();
        }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if esc {
            ui.data_mut(|d| d.remove::<String>(edit_id));
        } else if enter || r_te.lost_focus() {
            let txt = buf.trim().replace(',', ".");
            if let Some(nv) = p.string_to_normalized_value(&txt) {
                let nv = nv.clamp(0.0, 1.0);
                delta = nv - norm;
                set_norm(setter, p, nv);
            }
            ui.data_mut(|d| d.remove::<String>(edit_id));
        } else {
            ui.data_mut(|d| d.insert_temp(edit_id, buf));
        }
    } else {
        let vresp = ui.allocate_rect(vr, Sense::click()).on_hover_text("Click to type a value");
        let painter = ui.painter();
        painter.rect_stroke(vr, CornerRadius::same(4), Stroke::new(1.0_f32, if vresp.hovered() { DIM } else { LINE }), StrokeKind::Inside);
        painter.text(vr.center(), Align2::CENTER_CENTER, &val, mono(11.0), WHITE);
        if vresp.clicked() {
            let plain = val.trim().split(' ').next().unwrap_or("").to_string();
            ui.data_mut(|d| d.insert_temp(edit_id, plain));
        }
    }
    ev.delta = delta;
    ev
}

fn painter_label(ui: &Ui, cell: Rect, vr: Rect, label: &str) {
    ui.painter().text(Pos2::new(cell.center().x, vr.bottom() + 9.0), Align2::CENTER_CENTER, label, mono(10.0), DIM);
}

fn pill(ui: &mut Ui, rect: Rect, text: &str, on: bool) -> bool {
    let resp = ui.allocate_rect(rect, Sense::click());
    let painter = ui.painter();
    let hov = resp.hovered();
    painter.rect_filled(rect, CornerRadius::same(6), if on { WHITE } else if hov { PANEL2 } else { Color32::TRANSPARENT });
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0_f32, if on { WHITE } else if hov { DIM } else { LINE }), StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, mono(11.0), if on { Color32::BLACK } else { WHITE });
    resp.clicked()
}

fn enum_combo<E: Enum + PartialEq + 'static>(ui: &mut Ui, setter: &ParamSetter, p: &EnumParam<E>, id: &str, rect: Rect) {
    let names = E::variants();
    let idx = p.value().to_index();
    ui.allocate_new_ui(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .width(rect.width() - 22.0)
            .selected_text(egui::RichText::new(names[idx]).font(mono(12.0)))
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(i == idx, egui::RichText::new(*n).font(mono(12.0))).clicked() {
                        set_e(setter, p, E::from_index(i));
                    }
                }
            });
    });
}

fn section_title(ui: &Ui, pos: Pos2, num: &str, title: &str) {
    let mut job = LayoutJob::default();
    job.append("\u{2116}", 0.0, TextFormat { font_id: FontId::proportional(11.0), color: DIM, ..Default::default() });
    job.append(&format!(" {num}  {title}"), 0.0, TextFormat { font_id: mono(10.5), color: DIM, ..Default::default() });
    let galley = ui.fonts(|f| f.layout_job(job));
    ui.painter().galley(pos, galley, DIM);
}

// ------------------------------------------------------------------------------------------
// Editor
// ------------------------------------------------------------------------------------------

pub fn create(params: Arc<NyashkTuneParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let state = Gui {
        det: VecDeque::from(vec![f32::NAN; HIST]),
        cor: VecDeque::from(vec![f32::NAN; HIST]),
        last_push: Instant::now(),
        centre: 60.0,
        last_det: 0.0,
        last_target: 0.0,
        voiced: false,
        tab: 0,
    };
    let editor_state = params.editor_state.clone();
    let size_state = editor_state.clone();

    create_egui_editor(
        editor_state,
        state,
        |ctx, _| {
            let mut v = egui::Visuals::dark();
            v.panel_fill = BG;
            v.window_fill = PANEL;
            v.extreme_bg_color = PANEL;
            v.override_text_color = Some(WHITE);
            v.window_stroke = Stroke::new(1.0_f32, LINE);
            v.selection.bg_fill = Color32::from_rgb(64, 64, 64);
            v.selection.stroke = Stroke::new(1.0_f32, WHITE);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = CornerRadius::same(6);
                w.bg_stroke = Stroke::new(1.0_f32, LINE);
            }
            v.widgets.inactive.weak_bg_fill = PANEL;
            v.widgets.inactive.bg_fill = PANEL;
            v.widgets.hovered.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_fill = PANEL2;
            v.widgets.active.weak_bg_fill = PANEL2;
            v.widgets.open.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, DIM);
            v.popup_shadow = egui::Shadow::NONE;
            v.window_shadow = egui::Shadow::NONE;
            ctx.set_visuals(v);
        },
        move |ctx, setter, st| {
            ctx.request_repaint_after(Duration::from_millis(33));
            let p = &*params;

            // ---- history of detected / corrected pitch
            if st.last_push.elapsed() >= Duration::from_millis(33) {
                st.last_push = Instant::now();
                st.voiced = meters.voiced.load(Ordering::Relaxed) != 0;
                let (mut d, mut c) = (f32::NAN, f32::NAN);
                if st.voiced {
                    let hz = meters.hz.load(Ordering::Relaxed);
                    if hz.is_finite() && hz > 20.0 {
                        d = 69.0 + 12.0 * (hz / p.ref_a.value()).log2();
                        c = d + meters.corr.load(Ordering::Relaxed);
                        st.last_det = d;
                        st.last_target = meters.target.load(Ordering::Relaxed);
                        st.centre += 0.08 * (d - st.centre);
                    }
                }
                if !st.voiced {
                    let (lo, hi) = p.range_limits();
                    let mid = 69.0 + 12.0 * ((lo * hi).sqrt().min(500.0) / p.ref_a.value()).log2();
                    st.centre += 0.03 * (mid - st.centre);
                }
                if !st.centre.is_finite() {
                    st.centre = 60.0;
                }
                st.det.pop_front();
                st.det.push_back(d);
                st.cor.pop_front();
                st.cor.push_back(c);
            }

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| {
                let full = ui.max_rect();
                let o = full.min;
                let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(o.x + x, o.y + y), Vec2::new(w, h));
                let painter = ui.painter().clone();

                // ---- header
                let time = ui.input(|i| i.time);
                painter.text(Pos2::new(o.x + 18.0, o.y + 34.0), Align2::LEFT_CENTER, "Nyashk", mono(19.0), WHITE);
                painter.text(Pos2::new(o.x + 18.0 + 6.0 * 11.4, o.y + 34.0), Align2::LEFT_CENTER, "Tune", mono(19.0), DIM);
                if ((time * 2.0) as i64) % 2 == 0 {
                    painter.text(Pos2::new(o.x + 18.0 + 10.0 * 11.4, o.y + 34.0), Align2::LEFT_CENTER, "_", mono(19.0), WHITE);
                }
                painter.hline(o.x + 14.0..=o.x + 886.0, o.y + 58.0, Stroke::new(1.0_f32, LINE));

                // preset
                let cur_preset = (p.preset.load(Ordering::Relaxed) as usize).min(PRESETS.len() - 1);
                ui.allocate_new_ui(UiBuilder::new().max_rect(rect(158.0, 17.0, 164.0, 28.0)), |ui| {
                    egui::ComboBox::from_id_salt("preset")
                        .width(142.0)
                        .selected_text(egui::RichText::new(PRESETS[cur_preset]).font(mono(12.0)))
                        .show_ui(ui, |ui| {
                            for (i, n) in PRESETS.iter().enumerate() {
                                if ui.selectable_label(i == cur_preset, egui::RichText::new(*n).font(mono(12.0))).clicked() {
                                    apply_preset(setter, p, i);
                                }
                            }
                        });
                });
                painter.text(Pos2::new(o.x + 336.0, o.y + 31.0), Align2::LEFT_CENTER, "KEY", mono(10.0), DIM);
                enum_combo(ui, setter, &p.key, "key", rect(366.0, 17.0, 66.0, 28.0));
                painter.text(Pos2::new(o.x + 440.0, o.y + 31.0), Align2::LEFT_CENTER, "SCALE", mono(10.0), DIM);
                enum_combo(ui, setter, &p.scale, "scale", rect(484.0, 17.0, 136.0, 28.0));
                if pill(ui, rect(634.0, 18.0, 68.0, 26.0), "BYPASS", p.bypass.value()) {
                    set_b(setter, &p.bypass, !p.bypass.value());
                }
                if pill(ui, rect(710.0, 18.0, 60.0, 26.0), "MIDI", p.midi_mode.value()) {
                    set_b(setter, &p.midi_mode, !p.midi_mode.value());
                }
                enum_combo(ui, setter, &p.range, "range", rect(778.0, 17.0, 108.0, 28.0));

                // ---- pitch display
                let disp = rect(14.0, 70.0, 872.0, 184.0);
                painter.rect_filled(disp, CornerRadius::same(6), PANEL);
                painter.rect_stroke(disp, CornerRadius::same(6), Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
                {
                    let span = 16.0f32;
                    let lo = st.centre - span * 0.5;
                    let y_of = |m: f32| disp.bottom() - 8.0 - (m - lo) / span * (disp.height() - 16.0);
                    let mask = p.allowed_mask();
                    let midi_mode = p.midi_mode.value();
                    let clip = painter.with_clip_rect(disp.shrink(1.0));
                    for m in (lo.floor() as i32 - 1)..=(lo + span).ceil() as i32 + 1 {
                        let pc = m.rem_euclid(12) as usize;
                        let allowed = !midi_mode && (mask >> pc) & 1 == 1;
                        let y = y_of(m as f32);
                        clip.hline(disp.left() + 48.0..=disp.right() - 8.0, y, Stroke::new(1.0_f32, if allowed { WHITE.gamma_multiply(0.28) } else { WHITE.gamma_multiply(0.05) }));
                        clip.text(Pos2::new(disp.left() + 8.0, y), Align2::LEFT_CENTER, midi_name(m), mono(10.5), if allowed { WHITE } else { DIM.gamma_multiply(0.7) });
                    }
                    let line = |h: &VecDeque<f32>, col: Color32, w: f32| {
                        let x0 = disp.left() + 52.0;
                        let wd = disp.width() - 62.0;
                        let mut pts: Vec<Pos2> = Vec::new();
                        for (i, v) in h.iter().enumerate() {
                            if v.is_nan() {
                                if pts.len() > 1 {
                                    clip.add(egui::Shape::line(std::mem::take(&mut pts), PathStroke::new(w, col)));
                                }
                                pts.clear();
                                continue;
                            }
                            pts.push(Pos2::new(x0 + wd * i as f32 / (HIST - 1) as f32, y_of(*v)));
                        }
                        if pts.len() > 1 {
                            clip.add(egui::Shape::line(pts, PathStroke::new(w, col)));
                        }
                    };
                    line(&st.det, MID, 1.4);
                    line(&st.cor, WHITE, 2.2);

                    let mut info = String::from("--");
                    if st.voiced {
                        let cents = ((st.last_det - st.last_det.round()) * 100.0).round() as i32;
                        info = format!("{} {:+} c", midi_name(st.last_det.round() as i32), cents);
                        if midi_mode {
                            info.push_str("  (MIDI)");
                        } else if st.last_target > 0.0 {
                            info.push_str(&format!("  >  {}", midi_name(st.last_target as i32)));
                        }
                    }
                    clip.text(Pos2::new(disp.right() - 12.0, disp.top() + 18.0), Align2::RIGHT_CENTER, info, mono(17.0), WHITE);
                    clip.text(Pos2::new(disp.right() - 12.0, disp.top() + 38.0), Align2::RIGHT_CENTER, "output", mono(10.0), WHITE);
                    clip.text(Pos2::new(disp.right() - 64.0, disp.top() + 38.0), Align2::RIGHT_CENTER, "input", mono(10.0), MID);
                }

                // ---- note strip (click: note on/off, right-click: BYPASS = sung as it is)
                let mask = p.allowed_mask();
                let midi_mode = p.midi_mode.value();
                let byp = p.note_bypass.load(Ordering::Relaxed);
                let cw = 872.0 / 12.0;
                for i in 0..12 {
                    let r = rect(14.0 + cw * i as f32 + 2.0, 262.0, cw - 4.0, 34.0);
                    let resp = ui.allocate_rect(r, Sense::click()).on_hover_text("Click: note on/off in the scale.  Right-click: BYPASS - this note is left uncorrected.");
                    let on = (mask >> i) & 1 == 1;
                    let is_byp = (byp >> i) & 1 == 1;
                    let a = if midi_mode { 0.35 } else { 1.0 };
                    let fill = if on { WHITE } else if is_black(i) { BG } else { PANEL };
                    painter.rect_filled(r, CornerRadius::same(6), fill.gamma_multiply(a));
                    painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0_f32, if on { WHITE } else if resp.hovered() { DIM } else { LINE }), StrokeKind::Inside);
                    painter.text(r.center() - Vec2::new(0.0, if is_byp { 4.0 } else { 0.0 }), Align2::CENTER_CENTER, NOTE_NAMES[i], mono(12.0), if on { Color32::BLACK } else { DIM });
                    if is_byp {
                        painter.text(r.center() + Vec2::new(0.0, 10.0), Align2::CENTER_CENTER, "BYP", mono(8.0), if on { Color32::BLACK } else { WHITE });
                    }
                    if resp.clicked() {
                        toggle_note(setter, p, i);
                    }
                    if resp.secondary_clicked() {
                        p.note_bypass.store(byp ^ (1 << i), Ordering::Relaxed);
                    }
                }

                // ---- tabs
                for (i, t) in ["TUNE", "FORMANT", "VOICES"].iter().enumerate() {
                    if pill(ui, rect(14.0 + i as f32 * 118.0, 304.0, 110.0, 26.0), t, st.tab == i) {
                        st.tab = i;
                    }
                }
                {
                    let nv = p.voices.iter().filter(|v| v.on.value()).count();
                    let txt = format!("formant: {}   voices: {}", p.fmode.value().to_string_name(), nv);
                    painter.text(Pos2::new(o.x + 886.0, o.y + 317.0), Align2::RIGHT_CENTER, txt, mono(10.0), DIM);
                }

                let row1 = 338.0;
                let row2 = 496.0;
                let rh = 150.0;
                let group = |ui: &mut Ui, x: f32, y: f32, w: f32, num: &str, title: &str| -> Rect {
                    let r = rect(x, y, w, rh);
                    ui.painter().rect_filled(r, CornerRadius::same(6), PANEL);
                    ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
                    section_title(ui, Pos2::new(r.left() + 12.0, r.top() + 8.0), num, title);
                    r
                };
                let cells = |r: Rect, n: usize| -> Vec<Rect> {
                    let inner = Rect::from_min_max(Pos2::new(r.left() + 8.0, r.top() + 28.0), Pos2::new(r.right() - 8.0, r.bottom() - 4.0));
                    let w = inner.width() / n as f32;
                    (0..n).map(|i| Rect::from_min_size(Pos2::new(inner.left() + w * i as f32, inner.top()), Vec2::new(w, inner.height()))).collect()
                };

                if st.tab == 0 {
                    let g1 = group(ui, 14.0, row1, 584.0, "01", "TUNING");
                    let c = cells(g1, 6);
                    let d_retune = knob(ui, setter, &p.retune, "Retune Speed", c[0], "How fast the pitch settles onto the note while you hold it. 0 = instant robotic hard-tune.", false);
                    let d_trans = knob(ui, setter, &p.transition, "Note Transition", c[1], "How fast the pitch glides when the target note changes. Higher = smooth slides between notes.", false);
                    // LINK: both knobs move together and keep the distance between them.
                    // The partner is driven from its value at drag start, so nothing drifts or goes stale.
                    if p.link.value() {
                        let id = egui::Id::new("link_partner0");
                        if d_retune.started { let v = p.transition.unmodulated_normalized_value(); ui.data_mut(|d| d.insert_temp(id, v)); }
                        if d_trans.started { let v = p.retune.unmodulated_normalized_value(); ui.data_mut(|d| d.insert_temp(id, v)); }
                        if let Some(t) = d_retune.total {
                            let p0 = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(p.transition.unmodulated_normalized_value());
                            set_norm(setter, &p.transition, p0 + t);
                        } else if let Some(t) = d_trans.total {
                            let p0 = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(p.retune.unmodulated_normalized_value());
                            set_norm(setter, &p.retune, p0 + t);
                        } else if d_retune.delta != 0.0 {
                            set_norm(setter, &p.transition, p.transition.unmodulated_normalized_value() + d_retune.delta);
                        } else if d_trans.delta != 0.0 {
                            set_norm(setter, &p.retune, p.retune.unmodulated_normalized_value() + d_trans.delta);
                        }
                    }
                    let link_rect = Rect::from_min_size(Pos2::new(g1.left() + 128.0, g1.top() + 6.0), Vec2::new(52.0, 18.0));
                    if pill(ui, link_rect, "LINK", p.link.value()) {
                        set_b(setter, &p.link, !p.link.value());
                    }
                    knob(ui, setter, &p.amount, "Amount", c[2], "How much of the pitch error is corrected.", false);
                    knob(ui, setter, &p.flex, "Flex-Tune", c[3], "Dead zone around each note where nothing is corrected.", false);
                    knob(ui, setter, &p.humanize, "Humanize", c[4], "Slows correction on sustained notes that are already nearly in tune.", false);
                    knob(ui, setter, &p.nat_vib, "Nat. Vibrato", c[5], "Keeps the singer's own vibrato, only the centre pitch is corrected.", false);

                    let g2 = group(ui, 608.0, row1, 278.0, "02", "TRACKING");
                    let c = cells(g2, 3);
                    knob(ui, setter, &p.tracking, "Tracking", c[0], "How reluctantly the tuner jumps to another note. Low = follows quick note changes, high = stays on a note until you clearly leave it.", false);
                    knob(ui, setter, &p.gate, "Gate", c[1], "Below this level the signal passes through untuned.", false);
                    knob(ui, setter, &p.ref_a, "Ref A4", c[2], "", false);

                    let g3 = group(ui, 14.0, row2, 324.0, "03", "VIBRATO");
                    let c = cells(g3, 3);
                    knob(ui, setter, &p.vib_depth, "Depth", c[0], "", false);
                    knob(ui, setter, &p.vib_rate, "Rate", c[1], "", false);
                    knob(ui, setter, &p.vib_delay, "Delay", c[2], "Time after a note starts before the vibrato fades in.", false);
                    enum_combo(ui, setter, &p.vib_shape, "vibshape", Rect::from_min_size(Pos2::new(g3.left() + 112.0, g3.top() + 6.0), Vec2::new(108.0, 20.0)));
                    if pill(ui, Rect::from_min_size(Pos2::new(g3.right() - 54.0, g3.top() + 6.0), Vec2::new(44.0, 18.0)), "ON", p.vib_on.value()) {
                        set_b(setter, &p.vib_on, !p.vib_on.value());
                    }

                    // ---- scale learn: what pitch classes were sung, key detection
                    let g4 = group(ui, 348.0, row2, 270.0, "04", "SCALE LEARN");
                    let hist: [f32; 12] = std::array::from_fn(|i| meters.hist[i].load(Ordering::Relaxed) as f32);
                    let total: f32 = hist.iter().sum();
                    let mx = hist.iter().cloned().fold(0.0f32, f32::max).max(1.0);
                    let bars = Rect::from_min_size(Pos2::new(g4.left() + 12.0, g4.top() + 30.0), Vec2::new(g4.width() - 24.0, 48.0));
                    let bw = bars.width() / 12.0;
                    for i in 0..12 {
                        let h = hist[i] / mx * (bars.height() - 13.0);
                        let x0 = bars.left() + bw * i as f32;
                        let br = Rect::from_min_max(Pos2::new(x0 + 2.0, bars.bottom() - 13.0 - h), Pos2::new(x0 + bw - 2.0, bars.bottom() - 13.0));
                        if h > 0.5 {
                            painter.rect_filled(br, CornerRadius::same(2), if hist[i] >= mx * 0.12 { WHITE } else { MID });
                        }
                        painter.hline(x0 + 2.0..=x0 + bw - 2.0, bars.bottom() - 13.0, Stroke::new(1.0_f32, LINE));
                        painter.text(Pos2::new(x0 + bw * 0.5, bars.bottom() - 5.0), Align2::CENTER_CENTER, NOTE_NAMES[i], mono(8.0), if is_black(i) { DIM } else { WHITE });
                    }
                    let det = if total > 3000.0 { engine::detect_key(&hist) } else { None };
                    let txt = match det {
                        Some((k, sc, c)) => format!("sung in: {} {}  ({:.0}%)", NOTE_NAMES[k], if sc == 1 { "major" } else { "minor" }, (c * 100.0).max(0.0)),
                        None => "sing a phrase to learn...".to_string(),
                    };
                    painter.text(Pos2::new(g4.left() + 12.0, g4.top() + 92.0), Align2::LEFT_CENTER, txt, mono(10.5), if det.is_some() { WHITE } else { DIM });
                    let py = g4.top() + 108.0;
                    if pill(ui, Rect::from_min_size(Pos2::new(g4.left() + 12.0, py), Vec2::new(78.0, 26.0)), "DETECT", false) {
                        if let Some((k, sc, _)) = det {
                            set_e(setter, &p.key, Key::from_index(k));
                            set_e(setter, &p.scale, if sc == 1 { ScaleKind::Major } else { ScaleKind::NaturalMinor });
                        }
                    }
                    if pill(ui, Rect::from_min_size(Pos2::new(g4.left() + 96.0, py), Vec2::new(78.0, 26.0)), "LEARN", false) && total > 3000.0 {
                        let m = engine::learn_mask(&hist, 0.12);
                        for (i, n) in p.notes.iter().enumerate() {
                            set_b(setter, &n.on, (m >> i) & 1 == 1);
                        }
                        set_e(setter, &p.scale, ScaleKind::Custom);
                    }
                    if pill(ui, Rect::from_min_size(Pos2::new(g4.left() + 180.0, py), Vec2::new(78.0, 26.0)), "CLEAR", false) {
                        meters.hist_reset.store(true, Ordering::Relaxed);
                    }

                    let g5 = group(ui, 628.0, row2, 258.0, "05", "OUTPUT");
                    let c = cells(g5, 3);
                    knob(ui, setter, &p.mix, "Mix", c[0], "", false);
                    knob(ui, setter, &p.in_gain, "Input", c[1], "", true);
                    knob(ui, setter, &p.out_gain, "Output", c[2], "", true);
                } else if st.tab == 1 {
                    let g1 = group(ui, 14.0, row1, 190.0, "01", "PITCH");
                    let c = cells(g1, 2);
                    knob(ui, setter, &p.transpose, "Transpose", c[0], "", true);
                    knob(ui, setter, &p.fine, "Detune", c[1], "", true);

                    let g2 = group(ui, 214.0, row1, 672.0, "02", "FORMANT");
                    enum_combo(ui, setter, &p.fmode, "fmode", Rect::from_min_size(Pos2::new(g2.left() + 112.0, g2.top() + 6.0), Vec2::new(150.0, 20.0)));
                    if pill(ui, Rect::from_min_size(Pos2::new(g2.right() - 106.0, g2.top() + 6.0), Vec2::new(98.0, 18.0)), "KEEP FORMANTS", p.keep_formants.value()) {
                        set_b(setter, &p.keep_formants, !p.keep_formants.value());
                    }
                    let c = cells(g2, 5);
                    knob(ui, setter, &p.formant, "Shift", c[0], "Moves the vocal-tract resonances (gender / size) independently of pitch.", true);
                    knob(ui, setter, &p.ftilt, "Tilt", c[1], "Spectral algorithms: + moves the high formants more than the low ones (brighter, smaller), - the opposite.", true);
                    knob(ui, setter, &p.ffollow, "Follow", c[2], "With KEEP FORMANTS off: how much the formants follow the pitch correction (100 % = like a varispeed).", false);
                    knob(ui, setter, &p.air, "Air", c[3], "High shelf (7 kHz) on the processed voice. Brightness / breath.", false);
                    knob(ui, setter, &p.drive, "Drive", c[4], "Soft saturation of the processed voice.", false);

                    // warp curve: where a formant of the source ends up
                    let g3 = group(ui, 14.0, row2, 430.0, "03", "FORMANT MAP");
                    let plot = Rect::from_min_max(Pos2::new(g3.left() + 44.0, g3.top() + 30.0), Pos2::new(g3.right() - 16.0, g3.bottom() - 22.0));
                    painter.rect_filled(plot, CornerRadius::same(3), BG);
                    painter.rect_stroke(plot, CornerRadius::same(3), Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
                    let mode = p.fmode.value();
                    let sft = 2f32.powf(p.formant.value() / 12.0);
                    let tilt = if mode == FormantAlgo::Classic { 0.0 } else { p.ftilt.value() / 100.0 };
                    let mapf = |f: f32| -> f32 {
                        let x = (f / 8000.0).min(1.0);
                        let st = (sft.ln() * (1.0 + tilt * (2.0 * x - 1.0))).exp();
                        if mode == FormantAlgo::Bend {
                            400.0 * (f.max(1.0) / 400.0).powf(st)
                        } else {
                            f * st
                        }
                    };
                    let to_pt = |fin: f32, fout: f32| Pos2::new(plot.left() + plot.width() * fin / 8000.0, plot.bottom() - plot.height() * (fout / 8000.0).clamp(0.0, 1.0));
                    for k in 1..4 {
                        let f = k as f32 * 2000.0;
                        painter.vline(plot.left() + plot.width() * f / 8000.0, plot.top()..=plot.bottom(), Stroke::new(1.0_f32, LINE.gamma_multiply(0.7)));
                        painter.hline(plot.left()..=plot.right(), plot.bottom() - plot.height() * f / 8000.0, Stroke::new(1.0_f32, LINE.gamma_multiply(0.7)));
                        painter.text(Pos2::new(plot.left() + plot.width() * f / 8000.0, plot.bottom() + 9.0), Align2::CENTER_CENTER, format!("{}k", k * 2), mono(9.0), DIM);
                        painter.text(Pos2::new(plot.left() - 6.0, plot.bottom() - plot.height() * f / 8000.0), Align2::RIGHT_CENTER, format!("{}k", k * 2), mono(9.0), DIM);
                    }
                    painter.line_segment([to_pt(0.0, 0.0), to_pt(8000.0, 8000.0)], Stroke::new(1.0_f32, DIM));
                    let pts: Vec<Pos2> = (0..=64).map(|i| { let f = 8000.0 * i as f32 / 64.0; to_pt(f, mapf(f)) }).collect();
                    painter.with_clip_rect(plot).add(egui::Shape::line(pts, PathStroke::new(2.2_f32, WHITE)));
                    painter.text(Pos2::new(plot.left() + 8.0, plot.top() + 10.0), Align2::LEFT_CENTER, format!("1 kHz -> {:.2} kHz", mapf(1000.0) / 1000.0), mono(10.0), WHITE);
                    painter.text(Pos2::new(g3.left() + 12.0, g3.top() + 20.0 + 0.0), Align2::LEFT_CENTER, "", mono(9.0), DIM);
                    painter.text(Pos2::new(plot.right() - 6.0, plot.bottom() - 8.0), Align2::RIGHT_CENTER, "source formant", mono(8.5), DIM);

                    let g4 = group(ui, 454.0, row2, 432.0, "04", "ALGORITHM");
                    let (head, body): (&str, &[&str]) = match mode {
                        FormantAlgo::Classic => ("Classic", &["Every grain is resampled. Cheap and solid,", "the old-school PSOLA sound. Shift moves all", "formants evenly; Tilt does not apply."]),
                        FormantAlgo::Smooth => ("Smooth", &["Spectral. A broad envelope is moved, the fine", "structure stays. Most natural, hardest to break,", "good for subtle gender / size changes."]),
                        FormantAlgo::Detailed => ("Detailed", &["Spectral. Sharper envelope keeps vowel detail", "and presence. Clear, 'pro' sounding shifts;", "can get a little glassy at extreme settings."]),
                        FormantAlgo::Lpc => ("Vocal (LPC)", &["Spectral. All-pole vocal-tract model: peaky,", "singer-like resonances. Characterful and a bit", "nasal when pushed - great for stylised voices."]),
                        FormantAlgo::Bend => ("Bend", &["Spectral. Warps the formant SPACING around", "400 Hz instead of sliding everything: stretches", "or squeezes the tract. Odd, alien, expressive."]),
                    };
                    painter.text(Pos2::new(g4.left() + 14.0, g4.top() + 46.0), Align2::LEFT_CENTER, head, mono(14.0), WHITE);
                    for (i, l) in body.iter().enumerate() {
                        painter.text(Pos2::new(g4.left() + 14.0, g4.top() + 72.0 + 17.0 * i as f32), Align2::LEFT_CENTER, *l, mono(10.5), MID);
                    }
                    painter.text(Pos2::new(g4.left() + 14.0, g4.bottom() - 18.0), Align2::LEFT_CENTER, "Tip: KEEP FORMANTS on = formants stay where you set them.", mono(9.5), DIM);
                } else {
                    for i in 0..2 {
                        let y = if i == 0 { row1 } else { row2 };
                        let g = group(ui, 14.0, y, 872.0, if i == 0 { "01" } else { "02" }, if i == 0 { "VOICE 1" } else { "VOICE 2" });
                        let v = &p.voices[i];
                        if pill(ui, Rect::from_min_size(Pos2::new(g.left() + 112.0, g.top() + 6.0), Vec2::new(44.0, 18.0)), "ON", v.on.value()) {
                            set_b(setter, &v.on, !v.on.value());
                        }
                        if pill(ui, Rect::from_min_size(Pos2::new(g.left() + 162.0, g.top() + 6.0), Vec2::new(98.0, 18.0)), "SCALE-AWARE", v.snap.value()) {
                            set_b(setter, &v.snap, !v.snap.value());
                        }
                        painter.text(Pos2::new(g.right() - 8.0 - 4.0 * 52.0 - 8.0, g.top() + 15.0), Align2::RIGHT_CENTER, "interval", mono(9.5), DIM);
                        for (k, (txt, iv)) in [("-8va", -12.0f32), ("3rd", 4.0), ("5th", 7.0), ("+8va", 12.0)].iter().enumerate() {
                            if pill(ui, Rect::from_min_size(Pos2::new(g.right() - 8.0 - 4.0 * 52.0 + 52.0 * k as f32, g.top() + 6.0), Vec2::new(48.0, 18.0)), txt, (v.interval.value() - iv).abs() < 0.01) {
                                set_f(setter, &v.interval, *iv);
                            }
                        }
                        let c = cells(g, 6);
                        let n = i + 1;
                        knob_id(ui, setter, &v.interval, "Interval", &format!("v{n}_int"), c[0], "Semitones above (+) or below (-) the corrected lead. SCALE-AWARE keeps it inside the scale (a 3rd becomes major or minor as the scale needs).", true);
                        knob_id(ui, setter, &v.level, "Level", &format!("v{n}_lvl"), c[1], "", false);
                        knob_id(ui, setter, &v.pan, "Pan", &format!("v{n}_pan"), c[2], "", true);
                        knob_id(ui, setter, &v.detune, "Detune", &format!("v{n}_det"), c[3], "Fine pitch offset: a few cents gives a chorus / double-tracked feel.", true);
                        knob_id(ui, setter, &v.formant, "Formant", &format!("v{n}_fmt"), c[4], "Extra formant shift for this voice only.", true);
                        knob_id(ui, setter, &v.delay, "Delay", &format!("v{n}_dly"), c[5], "Small delay (10-30 ms) makes it a separate 'person'.", false);
                    }
                }

                // resize grip in the bottom-right corner (keeps the 900:640 proportions)
                let grip = Rect::from_min_size(full.max - Vec2::splat(22.0), Vec2::splat(22.0));
                let gr = ui.interact(grip, egui::Id::new("nt_resize_grip"), Sense::drag()).on_hover_text("Drag to resize");
                if gr.dragged() {
                    if let Some(pos) = gr.interact_pointer_pos() {
                        let ppp = ctx.pixels_per_point();
                        let w = ((pos.x - full.min.x + 11.0) * ppp).clamp(540.0, 2400.0);
                        let h = w * BASE_H / BASE_W;
                        size_state.set_requested_size((w.round() as u32, h.round() as u32));
                    }
                }
                let gc = if gr.hovered() || gr.dragged() { WHITE } else { DIM };
                for k in [6.0f32, 11.0, 16.0] {
                    painter.line_segment([Pos2::new(full.max.x - 3.0, full.max.y - k), Pos2::new(full.max.x - k, full.max.y - 3.0)], Stroke::new(1.0_f32, gc));
                }

                let _ = engine::MAX_CH;
                let _ = VoiceRange::Generic;
            });
        },
    )
}

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

use crate::{engine, Key, Meters, NyashkTuneParams, ScaleKind, VoiceRange};

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
const BASE_H: f32 = 640.0;

pub const PRESETS: [&str; 12] = [
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
];

struct Gui {
    det: VecDeque<f32>,
    cor: VecDeque<f32>,
    last_push: Instant,
    centre: f32,
    last_det: f32,
    last_target: f32,
    voiced: bool,
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

fn apply_preset(setter: &ParamSetter, p: &NyashkTuneParams, idx: usize) {
    // everything back to default except bypass and voice range
    reset(setter, &p.key);
    reset(setter, &p.scale);
    for n in &p.notes {
        reset(setter, &n.on);
    }
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
    reset(setter, &p.vib_on);
    reset(setter, &p.vib_depth);
    reset(setter, &p.vib_rate);
    reset(setter, &p.vib_delay);
    reset(setter, &p.midi_mode);
    reset(setter, &p.gate);
    reset(setter, &p.mix);

    match idx {
        1 => {
            set_e(setter, &p.scale, ScaleKind::Chromatic);
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
            set_e(setter, &p.scale, ScaleKind::Chromatic);
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
            set_e(setter, &p.scale, ScaleKind::Chromatic);
            set_f(setter, &p.retune, 0.0);
            set_f(setter, &p.transition, 0.0);
            set_f(setter, &p.formant, -2.0);
        }
        8 => {
            set_e(setter, &p.scale, ScaleKind::Chromatic);
            set_f(setter, &p.retune, 60.0);
            set_f(setter, &p.transition, 60.0);
            set_f(setter, &p.formant, 4.0);
            set_f(setter, &p.transpose, 3.0);
        }
        9 => {
            set_e(setter, &p.scale, ScaleKind::Chromatic);
            set_f(setter, &p.retune, 60.0);
            set_f(setter, &p.transition, 60.0);
            set_f(setter, &p.formant, -4.0);
            set_f(setter, &p.transpose, -3.0);
        }
        10 => {
            set_e(setter, &p.scale, ScaleKind::Chromatic);
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
    let r = 30.0;
    let c = Pos2::new(cell.center().x, cell.top() + r + 4.0);
    let hit = Rect::from_center_size(c, Vec2::splat(2.0 * r + 8.0));
    let resp = ui.allocate_rect(hit, Sense::click_and_drag());
    let resp = if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) };

    let norm = p.unmodulated_normalized_value();
    let mut delta = 0.0f32;
    let mut ev = KnobEv::default();
    let raw_id = egui::Id::new(("knob_raw", label));
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
    painter.add(egui::Shape::line(arc(0.0, 1.0), PathStroke::new(4.0, LINE)));
    painter.circle_filled(pt(ring, 0.0), 2.0, LINE);
    painter.circle_filled(pt(ring, 1.0), 2.0, LINE);
    let from = if bipolar { 0.5 } else { 0.0 };
    if (norm - from).abs() > 0.002 {
        painter.add(egui::Shape::line(arc(from, norm), PathStroke::new(4.0, WHITE)));
        painter.circle_filled(pt(ring, from), 2.0, WHITE);
        painter.circle_filled(pt(ring, norm), 2.0, WHITE);
    }
    // round knob body
    let body = r * 0.72;
    painter.circle_filled(c + Vec2::new(0.0, 1.5), body + 1.0, Color32::from_rgb(5, 5, 5));
    painter.circle_filled(c, body, Color32::from_rgb(34, 34, 34));
    painter.circle_filled(c, body * 0.84, Color32::from_rgb(24, 24, 24));
    painter.circle_stroke(c, body, Stroke::new(1.0, Color32::from_rgb(70, 70, 70)));
    // pointer
    painter.line_segment([pt(body * 0.30, norm), pt(body * 0.80, norm)], Stroke::new(3.0, WHITE));
    painter.circle_filled(pt(body * 0.80, norm), 1.5, WHITE);
    painter.circle_filled(pt(body * 0.30, norm), 1.5, WHITE);

    // value + label (click the value box to type a number)
    let val = p.normalized_value_to_string(norm, true);
    let vr = Rect::from_center_size(Pos2::new(cell.center().x, c.y + r + 14.0), Vec2::new(cell.width().min(84.0), 18.0));
    let edit_id = egui::Id::new(("knob_edit", label));
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
        painter.rect_stroke(vr, CornerRadius::same(4), Stroke::new(1.0, if vresp.hovered() { DIM } else { LINE }), StrokeKind::Inside);
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
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, if on { WHITE } else if hov { DIM } else { LINE }), StrokeKind::Inside);
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
            v.window_stroke = Stroke::new(1.0, LINE);
            v.selection.bg_fill = Color32::from_rgb(64, 64, 64);
            v.selection.stroke = Stroke::new(1.0, WHITE);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
                w.corner_radius = CornerRadius::same(6);
                w.bg_stroke = Stroke::new(1.0, LINE);
            }
            v.widgets.inactive.weak_bg_fill = PANEL;
            v.widgets.inactive.bg_fill = PANEL;
            v.widgets.hovered.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_fill = PANEL2;
            v.widgets.active.weak_bg_fill = PANEL2;
            v.widgets.open.weak_bg_fill = PANEL2;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, DIM);
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
                painter.hline(o.x + 14.0..=o.x + 886.0, o.y + 58.0, Stroke::new(1.0, LINE));

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
                painter.rect_stroke(disp, CornerRadius::same(6), Stroke::new(1.0, LINE), StrokeKind::Inside);
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
                        clip.hline(disp.left() + 48.0..=disp.right() - 8.0, y, Stroke::new(1.0, if allowed { WHITE.gamma_multiply(0.28) } else { WHITE.gamma_multiply(0.05) }));
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

                // ---- note strip
                let mask = p.allowed_mask();
                let midi_mode = p.midi_mode.value();
                let cw = 872.0 / 12.0;
                for i in 0..12 {
                    let r = rect(14.0 + cw * i as f32 + 2.0, 262.0, cw - 4.0, 34.0);
                    let resp = ui.allocate_rect(r, Sense::click());
                    let on = (mask >> i) & 1 == 1;
                    let a = if midi_mode { 0.35 } else { 1.0 };
                    let fill = if on { WHITE } else if is_black(i) { BG } else { PANEL };
                    painter.rect_filled(r, CornerRadius::same(6), fill.gamma_multiply(a));
                    painter.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, if on { WHITE } else if resp.hovered() { DIM } else { LINE }), StrokeKind::Inside);
                    painter.text(r.center(), Align2::CENTER_CENTER, NOTE_NAMES[i], mono(12.0), if on { Color32::BLACK } else { DIM });
                    if resp.clicked() {
                        toggle_note(setter, p, i);
                    }
                }

                // ---- knob groups
                let row1 = 306.0;
                let row2 = 464.0;
                let rh = 150.0;
                let group = |ui: &mut Ui, x: f32, y: f32, w: f32, num: &str, title: &str| -> Rect {
                    let r = rect(x, y, w, rh);
                    ui.painter().rect_filled(r, CornerRadius::same(6), PANEL);
                    ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, LINE), StrokeKind::Inside);
                    section_title(ui, Pos2::new(r.left() + 12.0, r.top() + 8.0), num, title);
                    r
                };
                let cells = |r: Rect, n: usize| -> Vec<Rect> {
                    let inner = Rect::from_min_max(Pos2::new(r.left() + 8.0, r.top() + 28.0), Pos2::new(r.right() - 8.0, r.bottom() - 4.0));
                    let w = inner.width() / n as f32;
                    (0..n).map(|i| Rect::from_min_size(Pos2::new(inner.left() + w * i as f32, inner.top()), Vec2::new(w, inner.height()))).collect()
                };

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

                let g2 = group(ui, 608.0, row1, 278.0, "02", "PITCH & FORMANT");
                let c = cells(g2, 3);
                knob(ui, setter, &p.transpose, "Transpose", c[0], "", true);
                knob(ui, setter, &p.fine, "Detune", c[1], "", true);
                knob(ui, setter, &p.formant, "Formant", c[2], "Shifts the vocal-tract character (gender / size) independently of pitch.", true);
                if pill(ui, Rect::from_min_size(Pos2::new(g2.right() - 106.0, g2.top() + 6.0), Vec2::new(98.0, 18.0)), "KEEP FORMANTS", p.keep_formants.value()) {
                    set_b(setter, &p.keep_formants, !p.keep_formants.value());
                }

                let g3 = group(ui, 14.0, row2, 324.0, "03", "VIBRATO");
                let c = cells(g3, 3);
                knob(ui, setter, &p.vib_depth, "Depth", c[0], "", false);
                knob(ui, setter, &p.vib_rate, "Rate", c[1], "", false);
                knob(ui, setter, &p.vib_delay, "Delay", c[2], "Time after a note starts before the vibrato fades in.", false);
                if pill(ui, Rect::from_min_size(Pos2::new(g3.right() - 54.0, g3.top() + 6.0), Vec2::new(44.0, 18.0)), "ON", p.vib_on.value()) {
                    set_b(setter, &p.vib_on, !p.vib_on.value());
                }

                let g4 = group(ui, 348.0, row2, 538.0, "04", "INPUT / OUTPUT");
                let c = cells(g4, 5);
                knob(ui, setter, &p.gate, "Gate", c[0], "Below this level the signal passes through untuned.", false);
                knob(ui, setter, &p.ref_a, "Ref A4", c[1], "", false);
                knob(ui, setter, &p.mix, "Mix", c[2], "", false);
                knob(ui, setter, &p.in_gain, "Input", c[3], "", true);
                knob(ui, setter, &p.out_gain, "Output", c[4], "", true);

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
                    painter.line_segment([Pos2::new(full.max.x - 3.0, full.max.y - k), Pos2::new(full.max.x - k, full.max.y - 3.0)], Stroke::new(1.0, gc));
                }

                let _ = engine::MAX_CH;
                let _ = VoiceRange::Generic;
            });
        },
    )
}

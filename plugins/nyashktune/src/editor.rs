//! NyashkTune GUI, laid out like a hardware-simple real-time tuner: correction timing on the left,
//! the correction meter in the middle, tolerance on the right, vibrato / correction / formant /
//! reference below, the keyboard with the note states and the correction range, and root / scale /
//! voice at the bottom.

use nih_plug::prelude::*;
use nih_plug_egui::{
    create_egui_editor,
    egui::{self, epaint::PathStroke, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2},
};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::{Key, Meters, NoteState, NyashkTuneParams, ScaleKind, VoiceRange};

// palette
const BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
const PANEL: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
const PANEL2: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
const LINE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
const WHITE: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
const DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
const ILLEGAL: Color32 = Color32::from_rgb(0xd8, 0x62, 0x7a);
const BYPASS: Color32 = Color32::from_rgb(0x5f, 0xa3, 0xe4);
const IN_COL: Color32 = Color32::from_rgb(0xe2, 0xb0, 0x58);

const BASE_W: f32 = 940.0;
const BASE_H: f32 = 600.0;
const KB_LO: i32 = 24; // C1
const KB_HI: i32 = 96; // C7

const NOTE_SHARP: [&str; 12] = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
const NOTE_FLAT: [&str; 12] = ["C", "D♭", "D", "E♭", "E", "F", "G♭", "G", "A♭", "A", "B♭", "B"];

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn is_black(pc: usize) -> bool {
    matches!(pc, 1 | 3 | 6 | 8 | 10)
}

struct Gui {
    in_lv: f32,
    out_lv: f32,
    corr: f32,
    hold_note: f32,
    last_voiced: f64,
}

fn set_norm<P: Param>(setter: &ParamSetter, p: &P, v: f32) {
    setter.begin_set_parameter(p);
    setter.set_parameter_normalized(p, v.clamp(0.0, 1.0));
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
fn set_i(setter: &ParamSetter, p: &IntParam, v: i32) {
    setter.begin_set_parameter(p);
    setter.set_parameter(p, v);
    setter.end_set_parameter(p);
}

/// Root / Scale chosen by hand: the keyboard follows the scale again (hand edits are dropped).
fn reset_scale(setter: &ParamSetter, p: &NyashkTuneParams) {
    for n in p.notes.iter() {
        if n.state.value() != NoteState::Scale {
            set_e(setter, &n.state, NoteState::Scale);
        }
    }
}

/// ♯ and ♭ are not in egui's built-in fonts: DejaVu Sans as a fall-back after the primary font.
fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("dejavu".into(), Arc::new(FontData::from_static(include_bytes!("fonts/DejaVuSans.ttf"))));
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        if let Some(list) = fonts.families.get_mut(&fam) {
            let at = list.len().min(1);
            list.insert(at, "dejavu".into());
        }
    }
    ctx.set_fonts(fonts);
}

fn pill(ui: &mut Ui, rect: Rect, text: &str, on: bool, tip: &str) -> bool {
    let mut resp = ui.allocate_rect(rect, Sense::click());
    if !tip.is_empty() {
        resp = resp.on_hover_text(tip);
    }
    let painter = ui.painter();
    let hov = resp.hovered();
    painter.rect_filled(rect, CornerRadius::same(6), if on { WHITE } else if hov { PANEL2 } else { Color32::TRANSPARENT });
    painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, if on { WHITE } else if hov { DIM } else { LINE }), StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, mono(11.0), if on { Color32::BLACK } else { WHITE });
    resp.clicked()
}

fn panel(ui: &Ui, rect: Rect, title: &str) {
    ui.painter().rect_filled(rect, CornerRadius::same(6), PANEL);
    ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, LINE), StrokeKind::Inside);
    if !title.is_empty() {
        ui.painter().text(Pos2::new(rect.left() + 12.0, rect.top() + 14.0), Align2::LEFT_CENTER, title, mono(10.0), DIM);
    }
}

fn enum_combo<E: Enum + PartialEq + 'static>(ui: &mut Ui, id: &str, rect: Rect, cur: E, shown: &str) -> Option<E> {
    let names = E::variants();
    let idx = cur.to_index();
    let mut pick = None;
    ui.allocate_new_ui(UiBuilder::new().max_rect(rect), |ui| {
        egui::ComboBox::from_id_salt(id)
            .width(rect.width() - 22.0)
            .selected_text(egui::RichText::new(shown).font(mono(12.0)))
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    if ui.selectable_label(i == idx, egui::RichText::new(*n).font(mono(12.0))).clicked() {
                        pick = Some(E::from_index(i));
                    }
                }
            });
    });
    pick
}

/// Knob: drag (shift = fine), wheel, double-click = default, click the value to type.
/// Returns the normalised change made this frame.
#[allow(clippy::too_many_arguments)]
fn knob<P: Param>(ui: &mut Ui, setter: &ParamSetter, p: &P, label: &str, id: &str, c: Pos2, r: f32, tooltip: &str, bipolar: bool) -> f32 {
    let hit = Rect::from_center_size(c, Vec2::splat(2.0 * r + 8.0));
    let resp = ui.allocate_rect(hit, Sense::click_and_drag());
    let resp = if tooltip.is_empty() { resp } else { resp.on_hover_text(tooltip) };
    let norm = p.unmodulated_normalized_value();
    let mut delta = 0.0f32;
    let raw_id = egui::Id::new(("knob_raw", id));
    if resp.drag_started() {
        setter.begin_set_parameter(p);
        ui.data_mut(|d| d.insert_temp(raw_id, norm));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let d = -resp.drag_delta().y / if fine { 1200.0 } else { 220.0 };
        let raw = ui.data(|dd| dd.get_temp::<f32>(raw_id)).unwrap_or(norm) + d;
        ui.data_mut(|dd| dd.insert_temp(raw_id, raw));
        let new = raw.clamp(0.0, 1.0);
        delta = new - norm;
        setter.set_parameter_normalized(p, new);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(p);
    }
    if resp.double_clicked() {
        delta = p.default_normalized_value() - norm;
        set_norm(setter, p, p.default_normalized_value());
    }
    if resp.hovered() {
        let sc = ui.input(|i| i.smooth_scroll_delta.y);
        if sc != 0.0 {
            let new = (norm + sc / 1500.0).clamp(0.0, 1.0);
            delta = new - norm;
            set_norm(setter, p, new);
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
    let w = if r > 34.0 { 5.0 } else { 4.0 };
    painter.add(egui::Shape::line(arc(0.0, 1.0), PathStroke::new(w, LINE)));
    let from = if bipolar { 0.5 } else { 0.0 };
    if (norm - from).abs() > 0.002 {
        painter.add(egui::Shape::line(arc(from, norm), PathStroke::new(w, WHITE)));
    }
    let body = r * 0.72;
    painter.circle_filled(c + Vec2::new(0.0, 1.5), body + 1.0, Color32::from_rgb(5, 5, 5));
    painter.circle_filled(c, body, Color32::from_rgb(34, 34, 34));
    painter.circle_filled(c, body * 0.84, Color32::from_rgb(24, 24, 24));
    painter.circle_stroke(c, body, Stroke::new(1.0, Color32::from_rgb(70, 70, 70)));
    painter.line_segment([pt(body * 0.30, norm), pt(body * 0.80, norm)], Stroke::new(3.0, WHITE));

    // value box (click to type) + label
    let val = p.normalized_value_to_string(norm, true);
    let vr = Rect::from_center_size(Pos2::new(c.x, c.y + r + 14.0), Vec2::new(84.0, 18.0));
    let edit_id = egui::Id::new(("knob_edit", id));
    let editing: Option<String> = ui.data(|d| d.get_temp::<String>(edit_id));
    ui.painter().text(Pos2::new(c.x, vr.bottom() + 10.0), Align2::CENTER_CENTER, label, mono(10.0), DIM);
    if let Some(mut buf) = editing {
        let te = egui::TextEdit::singleline(&mut buf).font(mono(11.0)).horizontal_align(egui::Align::Center).desired_width(vr.width()).margin(egui::Margin::symmetric(2, 1));
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
        let gw = ui.fonts(|f| f.layout_no_wrap(val.clone(), mono(11.0), WHITE).size().x);
        let size = if gw > vr.width() - 6.0 { (11.0 * (vr.width() - 6.0) / gw).max(7.5) } else { 11.0 };
        painter.text(vr.center(), Align2::CENTER_CENTER, &val, mono(size), WHITE);
        if vresp.clicked() {
            let first = val.trim().split(' ').next().unwrap_or("").to_string();
            let same = p.string_to_normalized_value(&first).map(|m| p.normalized_value_to_string(m, true)).as_deref() == Some(val.as_str());
            let plain = if same { first } else { val.trim().to_string() };
            ui.data_mut(|d| d.insert_temp(edit_id, plain));
        }
    }
    delta
}

/// Vertical peak meter, -28 .. 0 dBFS.
fn meter(ui: &Ui, rect: Rect, lin: f32, label: &str) {
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(7, 7, 7));
    let db = 20.0 * lin.max(1e-6).log10();
    let t = ((db + 28.0) / 28.0).clamp(0.0, 1.0);
    let h = rect.height() * t;
    let col = if db > -0.5 { ILLEGAL } else { Color32::from_gray(200) };
    p.rect_filled(Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - h), rect.max), CornerRadius::same(2), col);
    p.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, LINE), StrokeKind::Inside);
    p.text(Pos2::new(rect.center().x, rect.bottom() + 9.0), Align2::CENTER_CENTER, label, mono(9.0), DIM);
}

pub fn create(params: Arc<NyashkTuneParams>, meters: Arc<Meters>) -> Option<Box<dyn Editor>> {
    let editor_state = params.editor_state.clone();
    let size_state = editor_state.clone();
    create_egui_editor(
        editor_state,
        Gui { in_lv: 0.0, out_lv: 0.0, corr: 0.0, hold_note: 0.0, last_voiced: -10.0 },
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
            install_fonts(ctx);
        },
        move |ctx, setter, st| {
            ctx.request_repaint_after(Duration::from_millis(33));
            let p = &*params;
            let flats = p.flats.load(Ordering::Relaxed) != 0;
            let nn = |pc: usize| if flats { NOTE_FLAT[pc % 12] } else { NOTE_SHARP[pc % 12] };
            let note_name = |m: i32| format!("{}{}", nn(m.rem_euclid(12) as usize), m.div_euclid(12) - 1);
            let now = ctx.input(|i| i.time);

            // ---- meters
            let fall = |cur: f32, new: f32| if new > cur { new } else { cur * 0.86 };
            st.in_lv = fall(st.in_lv, meters.in_pk.swap(0.0, Ordering::Relaxed));
            st.out_lv = fall(st.out_lv, meters.out_pk.swap(0.0, Ordering::Relaxed));
            let voiced = meters.voiced.load(Ordering::Relaxed) != 0;
            let hz = meters.hz.load(Ordering::Relaxed);
            let target = meters.target.load(Ordering::Relaxed);
            let corr = meters.corr.load(Ordering::Relaxed);
            if voiced && hz > 20.0 {
                st.corr += (corr - st.corr) * 0.5;
                st.hold_note = target;
                st.last_voiced = now;
            } else {
                st.corr *= 0.8;
            }
            let showing = now - st.last_voiced < 0.4;
            let in_midi = if voiced && hz > 20.0 { Some(69.0 + 12.0 * (hz / p.ref_a.value()).log2()) } else { None };

            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ctx, |ui| {
                let full = ui.max_rect();
                let o = full.min;
                let rect = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(o.x + x, o.y + y), Vec2::new(w, h));
                let at = |x: f32, y: f32| Pos2::new(o.x + x, o.y + y);
                let painter = ui.painter().clone();

                // ---- header
                painter.text(at(18.0, 32.0), Align2::LEFT_CENTER, "Nyashk", mono(19.0), WHITE);
                painter.text(at(18.0 + 6.0 * 11.4, 32.0), Align2::LEFT_CENTER, "Tune", mono(19.0), DIM);
                painter.text(at(28.0 + 10.0 * 11.4 + 6.0, 34.0), Align2::LEFT_CENTER, "REAL-TIME PITCH CORRECTION", mono(10.5), DIM);
                painter.hline(o.x + 14.0..=o.x + BASE_W - 14.0, o.y + 56.0, Stroke::new(1.0, LINE));
                let byp = p.bypass.value();
                if pill(ui, rect(BASE_W - 14.0 - 84.0, 20.0, 84.0, 24.0), "BYPASS", byp, "Bypass the plugin.") {
                    set_b(setter, &p.bypass, !byp);
                }
                let mut fl = flats;
                let acc = rect(BASE_W - 14.0 - 84.0 - 10.0 - 72.0, 20.0, 34.0, 24.0);
                if pill(ui, acc, "♯", !fl, "Show sharps.") {
                    fl = false;
                }
                if pill(ui, acc.translate(Vec2::new(38.0, 0.0)), "♭", fl, "Show flats.") {
                    fl = true;
                }
                p.flats.store(fl as u32, Ordering::Relaxed);

                // ================= correction timing (left)
                let a = rect(14.0, 68.0, 250.0, 236.0);
                panel(ui, a, "CORRECTION TIMING");
                let link = p.link.value();
                let d_speed = knob(ui, setter, &p.speed, "SPEED", "speed", Pos2::new(a.left() + 66.0, a.top() + 92.0), 42.0, "How fast a sustained note is pulled onto the target and held there. Low = tight / robotic, high = loose / natural.", false);
                let d_tr = knob(ui, setter, &p.transition, "NOTE TRANSITION", "transition", Pos2::new(a.right() - 66.0, a.top() + 92.0), 42.0, "How fast the correction moves when the target note changes (from one note to the next).", false);
                if link {
                    // linked: both move together
                    if d_speed != 0.0 {
                        set_norm(setter, &p.transition, p.transition.unmodulated_normalized_value() + d_speed);
                    } else if d_tr != 0.0 {
                        set_norm(setter, &p.speed, p.speed.unmodulated_normalized_value() + d_tr);
                    }
                }
                if pill(ui, Rect::from_center_size(Pos2::new(a.center().x, a.bottom() - 28.0), Vec2::new(70.0, 22.0)), "LINK", link, "Move Speed and Note Transition together.") {
                    set_b(setter, &p.link, !link);
                }

                // ================= correction meter (centre)
                let m = rect(276.0, 68.0, 388.0, 236.0);
                panel(ui, m, "");
                let c = Pos2::new(m.center().x, m.top() + 150.0);
                let rad = 118.0f32;
                let ang = |cents: f32| -> f32 { std::f32::consts::PI * (1.0 + (cents.clamp(-100.0, 100.0) + 100.0) / 200.0) };
                let pt = |cents: f32, r: f32| c + Vec2::new(ang(cents).cos() * r, ang(cents).sin() * r);
                let arc: Vec<Pos2> = (0..=80).map(|i| pt(-100.0 + 200.0 * i as f32 / 80.0, rad)).collect();
                painter.add(egui::Shape::line(arc, PathStroke::new(6.0, Color32::from_rgb(24, 24, 24))));
                for t in [-100.0f32, -50.0, 0.0, 50.0, 100.0] {
                    painter.line_segment([pt(t, rad - 10.0), pt(t, rad + 4.0)], Stroke::new(1.0, if t == 0.0 { WHITE } else { DIM }));
                    painter.text(pt(t, rad + 16.0), Align2::CENTER_CENTER, if t == 0.0 { "0".to_string() } else { format!("{:+.0}", t) }, mono(9.0), DIM);
                }
                let cc = st.corr * 100.0;
                if showing && cc.abs() > 0.5 {
                    let n = ((cc.abs() / 200.0) * 80.0).ceil().max(2.0) as usize;
                    let bar: Vec<Pos2> = (0..=n).map(|i| pt(cc * i as f32 / n as f32, rad)).collect();
                    painter.add(egui::Shape::line(bar, PathStroke::new(6.0, WHITE)));
                }
                painter.text(Pos2::new(m.left() + 12.0, m.top() + 14.0), Align2::LEFT_CENTER, "CORRECTION (cents)", mono(10.0), DIM);
                let tgt = if showing && st.hold_note > 0.0 { note_name(st.hold_note as i32) } else { "--".to_string() };
                painter.text(c - Vec2::new(0.0, 42.0), Align2::CENTER_CENTER, tgt, mono(40.0), if showing { WHITE } else { DIM });
                painter.text(c - Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, "TARGET NOTE", mono(9.5), DIM);
                let input = match in_midi {
                    Some(v) => {
                        let n = v.round() as i32;
                        format!("input {}  {:+.0} c   ·   correction {:+.0} c", note_name(n), (v - n as f32) * 100.0, st.corr * 100.0)
                    }
                    None => "input --".to_string(),
                };
                painter.text(Pos2::new(c.x, m.bottom() - 16.0), Align2::CENTER_CENTER, input, mono(10.5), if in_midi.is_some() { IN_COL } else { DIM });

                // ================= tolerance (right)
                let t = rect(676.0, 68.0, 250.0, 236.0);
                panel(ui, t, "TOLERANCE");
                knob(ui, setter, &p.tol_cents, "CENTS", "tolc", Pos2::new(t.left() + 66.0, t.top() + 92.0), 34.0, "Extra margin beyond the 50 c border between two notes before the target changes. Off = straight to the nearest note.", false);
                knob(ui, setter, &p.tol_time, "TIME", "tolt", Pos2::new(t.right() - 66.0, t.top() + 92.0), 34.0, "How long the voice must stay past the border before the target changes. Keeps short slides and scoops from switching the note.", false);
                painter.text(Pos2::new(t.center().x, t.bottom() - 28.0), Align2::CENTER_CENTER, "only for changes from one note to another", mono(9.0), DIM);

                // ================= second row
                let row_y = 316.0;
                // vibrato
                let v = rect(14.0, row_y, 250.0, 122.0);
                panel(ui, v, "VIBRATO");
                let von = p.vibrato.value();
                if pill(ui, rect(30.0, row_y + 40.0, 60.0, 24.0), if von { "ON" } else { "OFF" }, von, "On: the singer's own vibrato is kept (scaled by Depth) and only the note centre is corrected. Off: the vibrato is corrected away like any other pitch change.") {
                    set_b(setter, &p.vibrato, !von);
                }
                knob(ui, setter, &p.vib_depth, "DEPTH", "vibd", Pos2::new(v.right() - 70.0, row_y + 44.0), 26.0, "100 % = vibrato as sung, less = flatter, more = wider (up to 200 %).", false);
                // correction
                let cr = rect(276.0, row_y, 190.0, 122.0);
                panel(ui, cr, "CORRECTION");
                let con = p.correction.value();
                if pill(ui, rect(292.0, row_y + 40.0, 60.0, 24.0), if con { "ON" } else { "OFF" }, con, "Switch the pitch correction on / off.") {
                    set_b(setter, &p.correction, !con);
                }
                knob(ui, setter, &p.amount, "AMOUNT", "amount", Pos2::new(cr.right() - 52.0, row_y + 44.0), 26.0, "How much of the correction is applied (100 % = all the way onto the note).", false);
                // formant
                let fm = rect(478.0, row_y, 186.0, 122.0);
                panel(ui, fm, "FORMANT");
                let fc = p.formant.value();
                if pill(ui, rect(494.0, row_y + 34.0, 154.0, 24.0), "CORRECTED", fc, "Keep the voice's natural timbre while the pitch is shifted (recommended for vocals).") {
                    set_b(setter, &p.formant, true);
                }
                if pill(ui, rect(494.0, row_y + 66.0, 154.0, 24.0), "NON-CORRECTED", !fc, "The timbre follows the pitch shift (pitch-shifter character, e.g. for hard effects).") {
                    set_b(setter, &p.formant, false);
                }
                // reference + meters
                let rf = rect(676.0, row_y, 250.0, 122.0);
                panel(ui, rf, "REFERENCE");
                knob(ui, setter, &p.ref_a, "A4", "refa", Pos2::new(rf.left() + 60.0, row_y + 44.0), 26.0, "Tuning of A4 (415.3 - 466.2 Hz = +-100 cents). Match a sped-up / slowed-down beat.", true);
                let cents = 1200.0 * (p.ref_a.value() / 440.0).log2();
                painter.text(Pos2::new(rf.left() + 136.0, row_y + 50.0), Align2::LEFT_CENTER, format!("{:+.0} c", cents), mono(12.0), WHITE);
                painter.text(Pos2::new(rf.left() + 136.0, row_y + 68.0), Align2::LEFT_CENTER, "vs 440 Hz", mono(9.0), DIM);
                meter(ui, rect(rf.right() - 40.0, row_y + 26.0, 9.0, 76.0), st.in_lv, "IN");
                meter(ui, rect(rf.right() - 22.0, row_y + 26.0, 9.0, 76.0), st.out_lv, "OUT");

                // ================= keyboard
                let kb = rect(14.0, 450.0, BASE_W - 28.0, 92.0);
                panel(ui, kb, "");
                let strip_h = 22.0;
                let keys = Rect::from_min_max(Pos2::new(kb.left() + 10.0, kb.top() + 8.0 + strip_h + 4.0), Pos2::new(kb.right() - 10.0, kb.bottom() - 8.0));
                let n_white = (KB_LO..=KB_HI).filter(|m| !is_black(m.rem_euclid(12) as usize)).count() as f32;
                let ww = keys.width() / n_white;
                let lo = p.range_lo.value().min(p.range_hi.value());
                let hi = p.range_lo.value().max(p.range_hi.value());
                // x of a key
                let mut white_i = 0.0f32;
                let mut key_rects: Vec<(i32, Rect, bool)> = Vec::new();
                for mnote in KB_LO..=KB_HI {
                    let pc = mnote.rem_euclid(12) as usize;
                    if is_black(pc) {
                        let x = keys.left() + white_i * ww - ww * 0.32;
                        key_rects.push((mnote, Rect::from_min_size(Pos2::new(x, keys.top()), Vec2::new(ww * 0.64, keys.height() * 0.6)), true));
                    } else {
                        key_rects.push((mnote, Rect::from_min_size(Pos2::new(keys.left() + white_i * ww, keys.top()), Vec2::new(ww - 1.0, keys.height())), false));
                        white_i += 1.0;
                    }
                }
                let in_note = in_midi.map(|v| v.round() as i32);
                let tgt_note = if showing && st.hold_note > 0.0 { Some(st.hold_note as i32) } else { None };
                for pass in [false, true] {
                    for (mnote, r, black) in &key_rects {
                        if *black != pass {
                            continue;
                        }
                        let inside = *mnote >= lo && *mnote <= hi;
                        let pc = mnote.rem_euclid(12) as usize;
                        let st_n = p.note_state(pc);
                        let mut fill = if *black { Color32::from_gray(if inside { 26 } else { 14 }) } else { Color32::from_gray(if inside { 205 } else { 70 }) };
                        if Some(*mnote) == tgt_note {
                            fill = WHITE;
                        }
                        if matches!(st_n, NoteState::Illegal | NoteState::Up | NoteState::Down) && inside {
                            fill = if *black { Color32::from_rgb(40, 20, 26) } else { Color32::from_rgb(150, 120, 128) };
                        }
                        painter.rect_filled(*r, CornerRadius::same(2), fill);
                        painter.rect_stroke(*r, CornerRadius::same(2), Stroke::new(1.0, Color32::from_gray(10)), StrokeKind::Inside);
                        if Some(*mnote) == in_note {
                            painter.circle_filled(Pos2::new(r.center().x, r.bottom() - 8.0), 3.5, IN_COL);
                        }
                        if Some(*mnote) == tgt_note {
                            painter.rect_stroke(r.shrink(1.0), CornerRadius::same(2), Stroke::new(2.0, if *black { WHITE } else { Color32::BLACK }), StrokeKind::Inside);
                        }
                        if pc == 0 && !*black && *mnote > KB_LO && *mnote < KB_HI {
                            painter.text(Pos2::new(r.center().x, r.bottom() - 9.0), Align2::CENTER_CENTER, format!("C{}", mnote / 12 - 1), mono(8.5), Color32::from_gray(60));
                        }
                    }
                }
                // range handles (drag)
                for (which, mnote) in [(0, lo), (1, hi)] {
                    let kr = key_rects.iter().find(|k| k.0 == mnote).map(|k| k.1).unwrap_or(keys);
                    let x = if which == 0 { kr.left() - 1.0 } else { kr.right() + 1.0 };
                    let hr = Rect::from_center_size(Pos2::new(x, keys.center().y), Vec2::new(12.0, keys.height() + 6.0));
                    let resp = ui.interact(hr, egui::Id::new(("range", which)), Sense::drag()).on_hover_text("Drag: correction range (notes outside it are left as sung).");
                    let col = if resp.hovered() || resp.dragged() { WHITE } else { IN_COL };
                    painter.line_segment([Pos2::new(x, keys.top() - 3.0), Pos2::new(x, keys.bottom() + 3.0)], Stroke::new(2.0, col));
                    if resp.dragged() {
                        if let Some(pp) = resp.interact_pointer_pos() {
                            // nearest key edge
                            let mut best = (mnote, f32::MAX);
                            for (k, r, black) in &key_rects {
                                if *black {
                                    continue;
                                }
                                let edge = if which == 0 { r.left() } else { r.right() };
                                if (edge - pp.x).abs() < best.1 {
                                    best = (*k, (edge - pp.x).abs());
                                }
                            }
                            let pr = if which == 0 { &p.range_lo } else { &p.range_hi };
                            let v2 = if which == 0 { best.0.min(hi - 1) } else { best.0.max(lo + 1) };
                            if v2 != pr.value() {
                                set_i(setter, pr, v2);
                            }
                        }
                    }
                }
                // note-state strip (one cell per key, all octaves of a note together)
                let strip = Rect::from_min_max(Pos2::new(keys.left(), kb.top() + 8.0), Pos2::new(keys.right(), kb.top() + 8.0 + strip_h));
                for (mnote, r, black) in &key_rects {
                    let pc = mnote.rem_euclid(12) as usize;
                    let cell = Rect::from_min_max(Pos2::new(r.left() + 1.0, strip.top() + if *black { 0.0 } else { strip_h * 0.5 }), Pos2::new(r.right() - 1.0, if *black { strip.top() + strip_h * 0.5 - 1.0 } else { strip.bottom() }));
                    let st_n = p.note_state(pc);
                    let (col, sym) = match st_n {
                        NoteState::Legal | NoteState::Scale => (Color32::from_gray(70), ""),
                        NoteState::Illegal => (ILLEGAL, "–"),
                        NoteState::Up => (ILLEGAL, "↑"),
                        NoteState::Down => (ILLEGAL, "↓"),
                        NoteState::Bypass => (BYPASS, "×"),
                    };
                    let resp = ui.interact(cell, egui::Id::new(("nstate", *mnote)), Sense::click()).on_hover_text(format!(
                        "{}: {} - click: Legal → Illegal → Pitch Up → Pitch Down → Bypass (all octaves)",
                        nn(pc),
                        match st_n {
                            NoteState::Legal | NoteState::Scale => "Legal",
                            NoteState::Illegal => "Illegal (to the nearest legal note)",
                            NoteState::Up => "Illegal - pitch up",
                            NoteState::Down => "Illegal - pitch down",
                            NoteState::Bypass => "Bypass (left as sung)",
                        }
                    ));
                    painter.rect_filled(cell, CornerRadius::same(2), if resp.hovered() { col.gamma_multiply(1.4) } else { col });
                    if !sym.is_empty() && cell.width() > 6.0 {
                        painter.text(cell.center(), Align2::CENTER_CENTER, sym, mono(9.0), Color32::BLACK);
                    }
                    if resp.clicked() {
                        let next = match st_n {
                            NoteState::Legal | NoteState::Scale => NoteState::Illegal,
                            NoteState::Illegal => NoteState::Up,
                            NoteState::Up => NoteState::Down,
                            NoteState::Down => NoteState::Bypass,
                            NoteState::Bypass => NoteState::Legal,
                        };
                        // what the scale gives by itself is stored as "Scale" (no asterisk)
                        let scale_gives = if (p.scale_mask() >> pc) & 1 == 1 { NoteState::Legal } else { NoteState::Illegal };
                        set_e(setter, &p.notes[pc].state, if next == scale_gives { NoteState::Scale } else { next });
                    }
                }

                // ================= bottom bar: root / scale / voice / midi
                let by = 556.0;
                painter.text(at(18.0, by + 12.0), Align2::LEFT_CENTER, "ROOT", mono(10.0), DIM);
                let key_idx = p.key.value() as usize;
                let naturals = [(0usize, "C"), (2, "D"), (4, "E"), (5, "F"), (7, "G"), (9, "A"), (11, "B")];
                for (i, (pc, name)) in naturals.iter().enumerate() {
                    let on = key_idx == *pc || (is_black(key_idx) && ((key_idx + 11) % 12 == *pc && !flats || (key_idx + 1) % 12 == *pc && flats));
                    if pill(ui, rect(58.0 + 30.0 * i as f32, by, 27.0, 24.0), name, on, "Root note") {
                        set_e(setter, &p.key, Key::from_index(*pc));
                        reset_scale(setter, p);
                    }
                }
                let accx = 58.0 + 30.0 * 7.0 + 6.0;
                if pill(ui, rect(accx, by, 27.0, 24.0), "♭", is_black(key_idx) && flats, "Lower the root a semitone.") {
                    set_e(setter, &p.key, Key::from_index((key_idx + 11) % 12));
                    p.flats.store(1, Ordering::Relaxed);
                    reset_scale(setter, p);
                }
                if pill(ui, rect(accx + 30.0, by, 27.0, 24.0), "♯", is_black(key_idx) && !flats, "Raise the root a semitone.") {
                    set_e(setter, &p.key, Key::from_index((key_idx + 1) % 12));
                    p.flats.store(0, Ordering::Relaxed);
                    reset_scale(setter, p);
                }
                painter.text(at(accx + 70.0, by + 12.0), Align2::LEFT_CENTER, "SCALE", mono(10.0), DIM);
                let sc = p.scale.value();
                let shown = format!("{}{}", ScaleKind::variants()[sc.to_index()], if p.scale_modified() { " *" } else { "" });
                if let Some(ns) = enum_combo(ui, "scale", rect(accx + 112.0, by, 190.0, 24.0), sc, &shown) {
                    set_e(setter, &p.scale, ns);
                    reset_scale(setter, p);
                }
                if pill(ui, rect(accx + 310.0, by, 64.0, 24.0), "RESET", false, "Reset Scale: drop the hand edits on the keyboard.") {
                    reset_scale(setter, p);
                }
                painter.text(at(accx + 388.0, by + 12.0), Align2::LEFT_CENTER, "VOICE", mono(10.0), DIM);
                let vr = p.range.value();
                if let Some(nv) = enum_combo(ui, "voice", rect(accx + 430.0, by, 150.0, 24.0), vr, VoiceRange::variants()[vr.to_index()]) {
                    set_e(setter, &p.range, nv);
                    let (l, h) = nv.notes();
                    set_i(setter, &p.range_lo, l);
                    set_i(setter, &p.range_hi, h);
                }
                let midi = p.midi_mode.value();
                if pill(ui, rect(BASE_W - 14.0 - 64.0, by, 64.0, 24.0), "MIDI", midi, "MIDI target: the notes held on the plugin's MIDI input are the legal notes.") {
                    set_b(setter, &p.midi_mode, !midi);
                }

                // resize grip
                let grip = Rect::from_min_size(full.max - Vec2::splat(18.0), Vec2::splat(18.0));
                let r = ui.interact(grip, egui::Id::new("tune_grip"), Sense::drag()).on_hover_text("Drag to resize");
                if r.dragged() {
                    if let Some(pos) = r.interact_pointer_pos() {
                        let ppp = if cfg!(target_os = "macos") { size_state.size().0 as f32 / BASE_W } else { ctx.pixels_per_point() };
                        let w = ((pos.x - full.min.x + 9.0) * ppp).clamp(700.0, 2800.0);
                        size_state.set_requested_size((w.round() as u32, (w * BASE_H / BASE_W).round() as u32));
                    }
                }
                let gc = if r.hovered() || r.dragged() { WHITE } else { DIM };
                for k in [5.0f32, 9.0, 13.0] {
                    painter.line_segment([Pos2::new(full.max.x - 3.0, full.max.y - k), Pos2::new(full.max.x - k, full.max.y - 3.0)], Stroke::new(1.0, gc));
                }
            });
        },
    )
}

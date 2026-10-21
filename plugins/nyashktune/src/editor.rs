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

use crate::browse::{self, BAct, BItem, BState};
use crate::{engine, FormantAlgo, Key, Meters, NyashkTuneParams, ScaleKind, VibShape, VoiceRange};

// palette
pub(crate) const BG: Color32 = Color32::from_rgb(0x0a, 0x0a, 0x0a);
pub(crate) const PANEL: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
pub(crate) const PANEL2: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
pub(crate) const LINE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2e);
pub(crate) const WHITE: Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
pub(crate) const DIM: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
pub(crate) const MID: Color32 = Color32::from_rgb(0x7a, 0x7a, 0x7a);

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

const PRESET_CATS: [&str; 28] = [
    "Natural", "Natural", "Natural", "Natural", "Hard Tune", "Hard Tune", "Hard Tune", "Robot / FX", "Gender", "Gender", "Vibrato", "MIDI",
    "Anime", "Anime", "Anime", "Anime", "Gender", "Gender", "Choir / Harmony", "Choir / Harmony", "Choir / Harmony", "Choir / Harmony", "Choir / Harmony", "Robot / FX", "Vibrato", "Robot / FX", "Gender", "Robot / FX",
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
    user: Vec<(String, String)>, // (name, category)
    user_sel: Option<String>,
    save_name: String,
    save_cat: String,
    browser: bool,
    bs: BState,
    meta: std::collections::HashMap<String, (bool, u8)>, // favourite, rating
    last_scan: f64,
    msg: String,
}

fn midi_name(m: i32) -> String {
    format!("{}{}", NOTE_NAMES[m.rem_euclid(12) as usize], m.div_euclid(12) - 1)
}

fn is_black(pc: usize) -> bool {
    matches!(pc, 1 | 3 | 6 | 8 | 10)
}

pub(crate) fn mono(size: f32) -> FontId {
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
    reset(setter, &p.formant_on);
    reset(setter, &p.colour_on);
    reset(setter, &p.voices_on);
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

// ------------------------------------------------------------------------------------------
// User presets: plain text files "id=value" in the user's config folder
// ------------------------------------------------------------------------------------------

fn preset_dir() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("NyashkTune").join("presets"))
}

fn clean_preset_name(n: &str) -> String {
    n.trim().chars().filter(|c| !c.is_control() && !"/\\:*?\"<>|".contains(*c)).collect::<String>().trim().to_string()
}

fn user_path(name: &str, cat: &str) -> Option<std::path::PathBuf> {
    let d = preset_dir()?;
    let sub = d.join(cat).join(format!("{name}.nytune"));
    if cat != "User" && sub.is_file() {
        return Some(sub);
    }
    Some(d.join(format!("{name}.nytune")))
}

/// (name, category). Files inside a category folder belong to that category;
/// loose files in the root use their `@cat=` line (or "User").
fn list_user_presets() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let Some(root) = preset_dir() else { return v };
    let mut scan = |dir: &std::path::Path, folder_cat: Option<&str>| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) == Some("nytune") {
                    if let Some(n) = path.file_stem().and_then(|x| x.to_str()) {
                        let cat = match folder_cat {
                            Some(c) => c.to_string(),
                            None => std::fs::read_to_string(&path)
                                .ok()
                                .and_then(|t| t.lines().find_map(|l| l.strip_prefix("@cat=").map(|c| c.trim().to_string())))
                                .filter(|c| !c.is_empty())
                                .unwrap_or_else(|| "User".to_string()),
                        };
                        v.push((n.to_string(), cat));
                    }
                }
            }
        }
    };
    scan(&root, None);
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                if let Some(c) = e.file_name().to_str() {
                    scan(&e.path(), Some(c));
                }
            }
        }
    }
    v.sort_by_key(|a| (a.1.to_lowercase(), a.0.to_lowercase()));
    v
}

fn meta_path() -> Option<std::path::PathBuf> {
    preset_dir().and_then(|d| d.parent().map(|p| p.join("favorites.txt")))
}

fn load_meta() -> std::collections::HashMap<String, (bool, u8)> {
    let mut m = std::collections::HashMap::new();
    if let Some(t) = meta_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in t.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() == 3 {
                m.insert(f[0].to_string(), (f[1] == "1", f[2].parse().unwrap_or(0)));
            }
        }
    }
    m
}

fn save_meta(m: &std::collections::HashMap<String, (bool, u8)>) {
    if let Some(p) = meta_path() {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
        let mut out = String::new();
        for (k, (f, r)) in m {
            if *f || *r > 0 {
                out.push_str(&format!("{k}\t{}\t{r}\n", *f as u8));
            }
        }
        let _ = std::fs::write(p, out);
    }
}

fn save_user_preset(p: &NyashkTuneParams, name: &str, cat: &str) -> std::io::Result<String> {
    let name = clean_preset_name(name);
    if name.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty name"));
    }
    let cat = clean_preset_name(cat);
    let cat = if cat.is_empty() { "User".to_string() } else { cat };
    let root = preset_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no config folder"))?;
    let dir = if cat == "User" { root.clone() } else { root.join(&cat) };
    std::fs::create_dir_all(&dir)?;
    let mut out = String::from("NyashkTune preset v1\n");
    out.push_str(&format!("@cat={cat}\n"));
    for (id, ptr, _) in p.param_map() {
        if id == "bypass" {
            continue;
        }
        // SAFETY: the pointer comes from our own live parameter struct
        let v = unsafe { ptr.unmodulated_normalized_value() };
        out.push_str(&format!("{id}={v:.7}\n"));
    }
    out.push_str(&format!("@note_bypass={}\n", p.note_bypass.load(Ordering::Relaxed)));
    std::fs::write(dir.join(format!("{name}.nytune")), out)?;
    Ok(name)
}

fn load_user_preset(setter: &ParamSetter, p: &NyashkTuneParams, name: &str, cat: &str) -> bool {
    let Some(path) = user_path(name, cat) else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let mut map = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.to_string(), v.trim().to_string());
        }
    }
    if let Some(v) = map.get("@note_bypass").and_then(|v| v.parse::<u32>().ok()) {
        p.note_bypass.store(v, Ordering::Relaxed);
    }
    for (id, ptr, _) in p.param_map() {
        if id == "bypass" {
            continue;
        }
        if let Some(v) = map.get(&id).and_then(|v| v.parse::<f32>().ok()) {
            // SAFETY: the pointer comes from our own live parameter struct
            unsafe {
                setter.raw_context.raw_begin_set_parameter(ptr);
                setter.raw_context.raw_set_parameter_normalized(ptr, v.clamp(0.0, 1.0));
                setter.raw_context.raw_end_set_parameter(ptr);
            }
        }
    }
    true
}

/// Delete the file; a category folder left empty goes away too (never the root).
fn delete_user_preset(name: &str, cat: &str) {
    if let (Some(root), Some(path)) = (preset_dir(), user_path(name, cat)) {
        let _ = std::fs::remove_file(&path);
        if let Some(dir) = path.parent() {
            if dir != root && dir.starts_with(&root) {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
}

fn open_preset_folder(cat: &str) {
    let Some(root) = preset_dir() else { return };
    let d = if cat != "User" && root.join(cat).is_dir() { root.join(cat) } else { root };
    let _ = std::fs::create_dir_all(&d);
    let cmd = if cfg!(target_os = "windows") { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(d).spawn();
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("dejavu".into(), std::sync::Arc::new(FontData::from_static(include_bytes!("fonts/DejaVuSans.ttf"))));
    let mut extra: Vec<String> = vec!["dejavu".into()];
    let sys = [
        ("C:\\Windows\\Fonts\\seguisym.ttf", "sys_sym"),
        ("C:\\Windows\\Fonts\\msyh.ttc", "sys_cjk1"),
        ("C:\\Windows\\Fonts\\YuGothM.ttc", "sys_cjk2"),
        ("C:\\Windows\\Fonts\\malgun.ttf", "sys_cjk3"),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", "sys_cjk4"),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", "sys_cjk5"),
        ("/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", "sys_cjk6"),
        ("/usr/share/fonts/TTF/NotoSansCJK-Regular.ttc", "sys_cjk7"),
        ("/System/Library/Fonts/PingFang.ttc", "sys_cjk8"),
        ("/System/Library/Fonts/Apple Symbols.ttf", "sys_sym2"),
    ];
    for (path, key) in sys {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() < 64 * 1024 * 1024 {
                fonts.font_data.insert(key.into(), std::sync::Arc::new(FontData::from_owned(bytes)));
                extra.push(key.into());
            }
        }
    }
    for fam in [FontFamily::Monospace, FontFamily::Proportional] {
        if let Some(list) = fonts.families.get_mut(&fam) {
            // after the primary font, before the emoji fall-backs
            let at = list.len().min(2);
            for (k, name) in extra.iter().enumerate() {
                list.insert(at + k, name.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
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

pub(crate) fn pill(ui: &mut Ui, rect: Rect, text: &str, on: bool) -> bool {
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
        user: list_user_presets(),
        user_sel: None,
        save_name: String::new(),
        save_cat: String::new(),
        browser: false,
        bs: BState::default(),
        meta: load_meta(),
        last_scan: 0.0,
        msg: String::new(),
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
            install_fonts(ctx);
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

                // ---- preset browser (modal)
                if st.browser {
                    let now = ui.input(|i| i.time);
                    if now - st.last_scan > 2.5 {
                        st.user = list_user_presets();
                        st.last_scan = now;
                    }
                    let n_fac = PRESETS.len();
                    let items: Vec<BItem> = PRESETS
                        .iter()
                        .enumerate()
                        .map(|(i, n)| {
                            let (fav, rating) = st.meta.get(&format!("f:{n}")).copied().unwrap_or((false, 0));
                            BItem { name: n.to_string(), cat: PRESET_CATS[i].to_string(), fav, user: false, rating }
                        })
                        .chain(st.user.iter().map(|(n, c)| {
                            let (fav, rating) = st.meta.get(&format!("u:{n}")).copied().unwrap_or((false, 0));
                            BItem { name: n.clone(), cat: c.clone(), fav, user: true, rating }
                        }))
                        .collect();
                    let cur_raw = p.preset.load(Ordering::Relaxed) as usize;
                    let sel: Option<usize> = if cur_raw < n_fac { Some(cur_raw) } else { st.user_sel.as_ref().and_then(|n| st.user.iter().position(|(u, _)| u == n)).map(|i| n_fac + i) };
                    let area = rect(8.0, 8.0, 884.0, 654.0);
                    let act = browse::browser(ui, area, "PRESETS", &items, sel, &mut st.bs);
                    let key_of = |i: usize| if i < n_fac { format!("f:{}", PRESETS[i]) } else { format!("u:{}", items[i].name) };
                    match act {
                        BAct::Pick(i) => {
                            if i < n_fac {
                                apply_preset(setter, p, i);
                                st.user_sel = None;
                            } else if load_user_preset(setter, p, &items[i].name, &items[i].cat) {
                                p.preset.store(u32::MAX, Ordering::Relaxed);
                                st.user_sel = Some(items[i].name.clone());
                                st.save_name = items[i].name.clone();
                                st.save_cat = items[i].cat.clone();
                            }
                        }
                        BAct::Fav(i) => {
                            let e = st.meta.entry(key_of(i)).or_insert((false, 0));
                            e.0 = !e.0;
                            save_meta(&st.meta);
                        }
                        BAct::Rate(i, v) => {
                            let e = st.meta.entry(key_of(i)).or_insert((false, 0));
                            e.1 = v;
                            save_meta(&st.meta);
                        }
                        BAct::Delete(i) => {
                            let (n, c) = (items[i].name.clone(), items[i].cat.clone());
                            delete_user_preset(&n, &c);
                            st.user = list_user_presets();
                            if st.user_sel.as_ref() == Some(&n) {
                                st.user_sel = None;
                            }
                            if !st.user.iter().any(|(_, uc)| *uc == c) && st.bs.cat == c {
                                st.bs.cat.clear();
                            }
                            st.msg = format!("deleted \"{n}\"");
                        }
                        BAct::OpenFolder(i) => open_preset_folder(&items[i].cat),
                        BAct::Close => st.browser = false,
                        BAct::None => {}
                    }
                    // footer: save as / category / save / delete
                    let f = browse::footer(area);
                    let y = f.top() + 4.0;
                    painter.text(Pos2::new(f.left(), y + 12.0), Align2::LEFT_CENTER, "SAVE AS", mono(10.0), DIM);
                    let nr = Rect::from_min_size(Pos2::new(f.left() + 62.0, y), Vec2::new(250.0, 24.0));
                    ui.put(nr, egui::TextEdit::singleline(&mut st.save_name).font(mono(12.0)).hint_text("preset name").desired_width(nr.width()).margin(egui::Margin::symmetric(6, 4)));
                    painter.text(Pos2::new(nr.right() + 14.0, y + 12.0), Align2::LEFT_CENTER, "CATEGORY", mono(10.0), DIM);
                    let cr = Rect::from_min_size(Pos2::new(nr.right() + 84.0, y), Vec2::new(170.0, 24.0));
                    ui.put(cr, egui::TextEdit::singleline(&mut st.save_cat).font(mono(12.0)).hint_text("Vocal, Fun, Mine...").desired_width(cr.width()).margin(egui::Margin::symmetric(6, 4)));
                    if pill(ui, Rect::from_min_size(Pos2::new(cr.right() + 10.0, y), Vec2::new(70.0, 24.0)), "SAVE", true) {
                        match save_user_preset(p, &st.save_name, &st.save_cat) {
                            Ok(n) => {
                                st.user = list_user_presets();
                                st.user_sel = Some(n.clone());
                                p.preset.store(u32::MAX, Ordering::Relaxed);
                                st.msg = format!("saved \"{n}\"");
                            }
                            Err(e) => st.msg = format!("not saved: {e}"),
                        }
                    }
                    if !st.msg.is_empty() {
                        painter.text(Pos2::new(f.left(), y + 40.0), Align2::LEFT_CENTER, &st.msg, mono(10.0), MID);
                    }
                    return;
                }

                // ---- header
                let time = ui.input(|i| i.time);
                painter.text(Pos2::new(o.x + 18.0, o.y + 34.0), Align2::LEFT_CENTER, "Nyashk", mono(19.0), WHITE);
                painter.text(Pos2::new(o.x + 18.0 + 6.0 * 11.4, o.y + 34.0), Align2::LEFT_CENTER, "Tune", mono(19.0), DIM);
                if ((time * 2.0) as i64) % 2 == 0 {
                    painter.text(Pos2::new(o.x + 18.0 + 10.0 * 11.4, o.y + 34.0), Align2::LEFT_CENTER, "_", mono(19.0), WHITE);
                }
                painter.hline(o.x + 14.0..=o.x + 886.0, o.y + 58.0, Stroke::new(1.0_f32, LINE));

                // preset
                // preset selector: < name (opens the browser) > SAVE
                let n_fac = PRESETS.len();
                let n_all = n_fac + st.user.len();
                let cur_raw = p.preset.load(Ordering::Relaxed) as usize;
                let cur_idx: Option<usize> = if cur_raw < n_fac {
                    Some(cur_raw)
                } else {
                    st.user_sel.as_ref().and_then(|n| st.user.iter().position(|(u, _)| u == n)).map(|i| n_fac + i)
                };
                let name: String = match cur_idx {
                    Some(i) if i < n_fac => PRESETS[i].to_string(),
                    Some(i) => st.user[i - n_fac].0.clone(),
                    None => "- preset -".to_string(),
                };
                let name: String = if name.chars().count() > 14 { name.chars().take(13).collect::<String>() + "~" } else { name };
                let mut pick: Option<usize> = None;
                if pill(ui, rect(150.0, 17.0, 24.0, 28.0), "<", false) {
                    pick = Some(cur_idx.map(|i| (i + n_all - 1) % n_all).unwrap_or(0));
                }
                if pill(ui, rect(178.0, 17.0, 114.0, 28.0), &name, false) {
                    st.browser = true;
                    st.user = list_user_presets();
                    st.save_name = st.user_sel.clone().unwrap_or_default();
                    st.msg.clear();
                }
                if pill(ui, rect(296.0, 17.0, 24.0, 28.0), ">", false) {
                    pick = Some(cur_idx.map(|i| (i + 1) % n_all).unwrap_or(0));
                }
                if pill(ui, rect(324.0, 17.0, 44.0, 28.0), "SAVE", false) {
                    st.browser = true;
                    st.user = list_user_presets();
                    st.save_name = st.user_sel.clone().unwrap_or_default();
                    st.msg.clear();
                }
                if let Some(i) = pick {
                    if i < n_fac {
                        apply_preset(setter, p, i);
                        st.user_sel = None;
                    } else if let Some((n, c)) = st.user.get(i - n_fac).cloned() {
                        if load_user_preset(setter, p, &n, &c) {
                            p.preset.store(u32::MAX, Ordering::Relaxed);
                            st.user_sel = Some(n);
                        }
                    }
                }
                painter.text(Pos2::new(o.x + 376.0, o.y + 31.0), Align2::LEFT_CENTER, "KEY", mono(10.0), DIM);
                enum_combo(ui, setter, &p.key, "key", rect(402.0, 17.0, 60.0, 28.0));
                painter.text(Pos2::new(o.x + 468.0, o.y + 31.0), Align2::LEFT_CENTER, "SCALE", mono(10.0), DIM);
                enum_combo(ui, setter, &p.scale, "scale", rect(510.0, 17.0, 116.0, 28.0));
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
                    if pill(ui, rect(14.0 + i as f32 * 100.0, 304.0, 96.0, 26.0), t, st.tab == i) {
                        st.tab = i;
                    }
                }
                // module switches: whatever is off is not computed at all
                painter.text(Pos2::new(o.x + 598.0 - 8.0, o.y + 317.0), Align2::RIGHT_CENTER, "modules", mono(9.5), DIM);
                let mods: [(&str, bool); 4] = [("FORMANT", p.formant_on.value()), ("VOICES", p.voices_on.value()), ("COLOUR", p.colour_on.value()), ("VIBRATO", p.vib_on.value())];
                for (i, (t, on)) in mods.iter().enumerate() {
                    if pill(ui, rect(598.0 + 72.0 * i as f32, 304.0, 68.0, 26.0), t, *on) {
                        match i {
                            0 => set_b(setter, &p.formant_on, !on),
                            1 => set_b(setter, &p.voices_on, !on),
                            2 => set_b(setter, &p.colour_on, !on),
                            _ => set_b(setter, &p.vib_on, !on),
                        }
                    }
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

                    let g3 = group(ui, 14.0, row2, 430.0, "03", "VIBRATO");
                    let c = cells(g3, 3);
                    knob(ui, setter, &p.vib_depth, "Depth", c[0], "", false);
                    knob(ui, setter, &p.vib_rate, "Rate", c[1], "", false);
                    knob(ui, setter, &p.vib_delay, "Delay", c[2], "Time after a note starts before the vibrato fades in.", false);
                    enum_combo(ui, setter, &p.vib_shape, "vibshape", Rect::from_min_size(Pos2::new(g3.left() + 112.0, g3.top() + 6.0), Vec2::new(108.0, 20.0)));
                    if pill(ui, Rect::from_min_size(Pos2::new(g3.right() - 54.0, g3.top() + 6.0), Vec2::new(44.0, 18.0)), "ON", p.vib_on.value()) {
                        set_b(setter, &p.vib_on, !p.vib_on.value());
                    }

                    let g5 = group(ui, 454.0, row2, 432.0, "04", "OUTPUT");
                    let c = cells(g5, 3);
                    knob(ui, setter, &p.mix, "Mix", c[0], "", false);
                    knob(ui, setter, &p.in_gain, "Input", c[1], "", true);
                    knob(ui, setter, &p.out_gain, "Output", c[2], "", true);
                } else if st.tab == 1 {
                    let g1 = group(ui, 14.0, row1, 190.0, "01", "PITCH");
                    let c = cells(g1, 2);
                    knob(ui, setter, &p.transpose, "Transpose", c[0], "", true);
                    knob(ui, setter, &p.fine, "Detune", c[1], "", true);

                    let g2 = group(ui, 214.0, row1, 400.0, "02", "FORMANT");
                    enum_combo(ui, setter, &p.fmode, "fmode", Rect::from_min_size(Pos2::new(g2.left() + 112.0, g2.top() + 6.0), Vec2::new(124.0, 20.0)));
                    if pill(ui, Rect::from_min_size(Pos2::new(g2.left() + 244.0, g2.top() + 6.0), Vec2::new(40.0, 18.0)), "ON", p.formant_on.value()) {
                        set_b(setter, &p.formant_on, !p.formant_on.value());
                    }
                    if pill(ui, Rect::from_min_size(Pos2::new(g2.right() - 106.0, g2.top() + 6.0), Vec2::new(98.0, 18.0)), "KEEP FORMANTS", p.keep_formants.value()) {
                        set_b(setter, &p.keep_formants, !p.keep_formants.value());
                    }
                    let c = cells(g2, 3);
                    knob(ui, setter, &p.formant, "Shift", c[0], "Moves the vocal-tract resonances (gender / size) independently of pitch.", true);
                    knob(ui, setter, &p.ftilt, "Tilt", c[1], "Spectral algorithms: + moves the high formants more than the low ones (brighter, smaller), - the opposite.", true);
                    knob(ui, setter, &p.ffollow, "Follow", c[2], "With KEEP FORMANTS off: how much the formants follow the pitch correction (100 % = like a varispeed).", false);

                    let g3 = group(ui, 624.0, row1, 262.0, "03", "COLOUR");
                    if pill(ui, Rect::from_min_size(Pos2::new(g3.right() - 52.0, g3.top() + 6.0), Vec2::new(44.0, 18.0)), "ON", p.colour_on.value()) {
                        set_b(setter, &p.colour_on, !p.colour_on.value());
                    }
                    let c = cells(g3, 2);
                    knob(ui, setter, &p.air, "Air", c[0], "High shelf (7 kHz) on the processed voice: brightness / breath.", false);
                    knob(ui, setter, &p.drive, "Drive", c[1], "Soft saturation of the processed voice: warmth and grit.", false);

                    // warp curve: where a formant of the source ends up
                    let g3 = group(ui, 14.0, row2, 430.0, "04", "FORMANT MAP");
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

                    let g4 = group(ui, 454.0, row2, 432.0, "05", "ALGORITHM");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_presets_roundtrip_files() {
        let dir = std::env::temp_dir().join(format!("nyashk-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::set_var("APPDATA", &dir);
        std::env::set_var("HOME", &dir);
        let p = NyashkTuneParams::default();
        assert_eq!(clean_preset_name("  a/b:c*d?\"e<f>g|h\\i  "), "abcdefghi");
        assert!(save_user_preset(&p, "   ", "").is_err());
        let n = save_user_preset(&p, "Мій пресет: ♥", "Мої").unwrap();
        assert_eq!(n, "Мій пресет ♥");
        assert!(list_user_presets().contains(&(n.clone(), "Мої".to_string())));
        let text = std::fs::read_to_string(preset_dir().unwrap().join("Мої").join(format!("{n}.nytune"))).unwrap();
        assert!(text.contains("retune=") && text.contains("fmode=") && text.contains("@note_bypass=0") && !text.contains("\nbypass="));
        assert!(preset_dir().unwrap().join("Мої").is_dir());
        delete_user_preset(&n, "Мої");
        assert!(!preset_dir().unwrap().join("Мої").exists(), "empty category folder must go");
        assert!(!list_user_presets().iter().any(|(x, _)| *x == n));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

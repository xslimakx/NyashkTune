//! Offline harness: run the tuner engine over a raw mono f32 file at 48 kHz.
//! usage: run_raw in.f32 out.f32 trace.csv [key=val ...]
//! keys: key scale tracking ultra retune transition amount humanize flex natvib min max refa
//! writes the output aligned to the input (latency removed) and per-64-sample state:
//! t, voiced, detected_hz, target_midi, correction_semis
use nyashktune::engine::{Engine, Params};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let mut p = Params::default();
    for kv in &a[4..] {
        let (k, v) = kv.split_once('=').unwrap();
        let f: f32 = v.parse().unwrap();
        match k {
            "key" => p.key = f as usize,
            "scale" => p.scale = f as usize,
            "tracking" => p.tracking = f,
            "ultra" => p.ultra = f,
            "retune" => p.retune_ms = f,
            "transition" => p.transition_ms = f,
            "amount" => p.amount = f,
            "humanize" => p.humanize = f,
            "flex" => p.flex_cents = f,
            "natvib" => p.natural_vib = f,
            "min" => p.min_hz = f,
            "max" => p.max_hz = f,
            "refa" => p.ref_a = f,
            _ => panic!("unknown key {k}"),
        }
    }
    let fs = 48000.0;
    let mut e = Engine::new(fs, p.min_hz);
    let lat = e.latency_samples() as usize;
    let mut out = Vec::with_capacity(x.len() + lat);
    let mut csv = std::io::BufWriter::new(std::fs::File::create(&a[3]).unwrap());
    let pad = vec![0.0f32; lat];
    for (i, ch) in x.chunks(64).chain(pad.chunks(64)).enumerate() {
        e.set_params(&p);
        for &s in ch {
            out.push(e.process_frame([s, s], 1)[0]);
        }
        let m = &e.meters;
        writeln!(csv, "{:.5},{},{:.3},{},{:.4}", i as f64 * 64.0 / fs, m.voiced as u8, m.hz, m.target, m.corr).unwrap();
    }
    let out = &out[lat..lat + x.len()];
    let mut f = std::io::BufWriter::new(std::fs::File::create(&a[2]).unwrap());
    for s in out {
        f.write_all(&s.to_le_bytes()).unwrap();
    }
    eprintln!("latency {lat} samples");
}

//! Grain trace for one offset in a time window. usage: dbg_one in.f32 offset t0 t1 [min]
use nyashktune::engine::Engine;
use nyashktune::NyashkTuneParams;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let k: usize = a[2].parse().unwrap();
    let (t0, t1): (f64, f64) = (a[3].parse().unwrap(), a[4].parse().unwrap());
    let mut p = NyashkTuneParams::default().engine_params();
    if let Some(m) = a.get(5) { p.min_hz = m.parse().unwrap(); }
    let mut e = Engine::new(48000.0, p.min_hz);
    let lat = e.latency_samples() as f64;
    eprintln!("latency {lat}");
    let mut inp = vec![0.0f32; k];
    inp.extend_from_slice(&x);
    let mut bs = 37usize; let mut i = 0;
    while i < inp.len() {
        let n = bs.min(inp.len() - i);
        e.set_params(&p);
        for j in i..i + n {
            let t = (j as f64 - k as f64 - lat) / 48000.0;
            e.debug = t >= t0 && t <= t1;
            e.process_frame([inp[j], inp[j]], 2);
        }
        i += n; bs = 16 + (bs * 7 + 13) % 497;
    }
}

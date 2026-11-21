//! Per-block CPU timing of the engine on a real voice: mean / p99 / max against the block budget.
//! usage: bench_blocks in.f32 [block]
use nyashktune::engine::Engine;
use nyashktune::NyashkTuneParams;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let bs: usize = a.get(2).map(|s| s.parse().unwrap()).unwrap_or(128);
    let fs = 48000.0;
    let pp = NyashkTuneParams::default();
    let p = pp.engine_params();
    let reps = 5;
    let mut t = vec![f64::MAX; x.len().div_ceil(bs)];
    let mut sink = 0.0f32;
    for _ in 0..reps {
        let mut e = Engine::new(fs, p.min_hz);
        for (bi, ch) in x.chunks(bs).enumerate() {
            let t0 = Instant::now();
            let p = pp.engine_params();
            e.set_params(&p);
            for &s in ch {
                let o = e.process_frame([s, s], 2);
                sink += o[0];
            }
            t[bi] = t[bi].min(t0.elapsed().as_secs_f64() * 1e6);
        }
    }
    let budget = bs as f64 / fs * 1e6;
    let mut s = t.clone();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = t.iter().sum::<f64>() / t.len() as f64;
    let over = t.iter().filter(|&&v| v > budget * 0.5).count();
    println!(
        "block {bs}: budget {:.0} us  mean {:.1} ({:.1}%)  p99 {:.1}  p99.9 {:.1}  max {:.1} ({:.0}%)  blocks>50%: {over}/{}  [{sink:.1}]",
        budget, mean, mean / budget * 100.0, s[s.len() * 99 / 100], s[s.len() * 999 / 1000], s[s.len() - 1], s[s.len() - 1] / budget * 100.0, t.len()
    );
    let mut idx: Vec<usize> = (0..t.len()).collect();
    idx.sort_by(|&a, &b| t[b].partial_cmp(&t[a]).unwrap());
    for &i in idx.iter().take(8) {
        println!("  slow block {i} at {:.3}s: {:.1} us", i as f64 * bs as f64 / fs, t[i]);
    }
}

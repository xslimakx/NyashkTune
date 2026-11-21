//! Same input, different start offsets (silence before it): the output should not depend on where
//! the render started. usage: offsets in.f32 outdir [lowhz]
//! writes outdir/o<k>.f32 aligned to the input (offset and latency removed)
use nyashktune::engine::Engine;
use nyashktune::NyashkTuneParams;
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let raw = std::fs::read(&a[1]).unwrap();
    let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let pp = NyashkTuneParams::default();
    let mut p = pp.engine_params();
    for kv in &a[3..] {
        let (k, v) = kv.split_once('=').unwrap();
        let f: f32 = v.parse().unwrap();
        match k {
            "min" => p.min_hz = f,
            "retune" => p.retune_ms = f,
            "transition" => p.transition_ms = f,
            "amount" => p.amount = f,
            _ => panic!("{k}"),
        }
    }
    let offs = [0usize, 1, 7, 33, 64, 100, 129, 250, 333, 480, 517, 777, 1000, 1500, 2047, 3001];
    for &k in &offs {
        let mut e = Engine::new(48000.0, p.min_hz);
        let lat = e.latency_samples() as usize;
        let mut inp = vec![0.0f32; k];
        inp.extend_from_slice(&x);
        inp.extend(std::iter::repeat(0.0).take(lat + 64));
        let mut out = Vec::with_capacity(inp.len());
        let mut tr: Vec<(usize, f32, f32, f32, bool)> = Vec::new();
        // irregular block sizes like a host
        let mut i = 0;
        let mut bs = 37usize;
        while i < inp.len() {
            let n = bs.min(inp.len() - i);
            e.set_params(&p);
            for &s in &inp[i..i + n] {
                out.push(e.process_frame([s, s], 2)[0]);
                let t = out.len();
                if t >= k && (t - k) % 64 == 0 {
                    let m = &e.meters;
                    tr.push((t - k, m.hz, m.target, m.corr, m.voiced));
                }
            }
            i += n;
            bs = 16 + (bs * 7 + 13) % 497;
        }
        let mut c = std::io::BufWriter::new(std::fs::File::create(format!("{}/t{k}.csv", a[2])).unwrap());
        for (t, hz, tg, co, v) in &tr {
            writeln!(c, "{t},{hz},{tg},{co},{}", *v as u8).unwrap();
        }
        let o = &out[k + lat..k + lat + x.len()];
        let mut f = std::fs::File::create(format!("{}/o{k}.f32", a[2])).unwrap();
        for v in o {
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
    println!("{}", offs.map(|v| v.to_string()).join(" "));
}

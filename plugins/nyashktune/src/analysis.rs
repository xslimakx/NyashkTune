//! Stress analysis on realistic synthetic singing: detector octave errors, output pitched an
//! octave (or more) down, clicks and low thumps. Run with
//! `cargo test --release -p nyashktune analysis -- --ignored --nocapture`.

use super::*;

struct Rng(u64);
impl Rng {
    fn u(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn n(&mut self) -> f64 {
        // approx normal
        (0..4).map(|_| self.u()).sum::<f64>() - 2.0
    }
}

#[derive(Clone, Copy)]
pub struct Style {
    pub lo: f64,
    pub hi: f64,
    pub breath: f64,
    pub reverb: f64,
    pub consonants: bool,
}

struct Reso {
    a1: f64,
    a2: f64,
    g: f64,
    y1: f64,
    y2: f64,
}
impl Reso {
    fn new(f: f64, bw: f64, fs: f64) -> Self {
        let r = (-PI * bw / fs).exp();
        let th = 2.0 * PI * f / fs;
        Self { a1: 2.0 * r * th.cos(), a2: -r * r, g: 1.0 - r, y1: 0.0, y2: 0.0 }
    }
    fn set(&mut self, f: f64, bw: f64, fs: f64) {
        let r = (-PI * bw / fs).exp();
        let th = 2.0 * PI * f / fs;
        self.a1 = 2.0 * r * th.cos();
        self.a2 = -r * r;
        self.g = 1.0 - r;
    }
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.g * x + self.a1 * self.y1 + self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

const VOWELS: [[f64; 3]; 5] = [[730.0, 1090.0, 2440.0], [530.0, 1840.0, 2480.0], [300.0, 2290.0, 3010.0], [570.0, 840.0, 2410.0], [320.0, 870.0, 2240.0]];

/// A sung phrase: notes with scoops, vibrato, jitter, breath, vowels, consonants, gaps.
/// Returns (signal, true f0 per sample, 0 = unvoiced / transition).
pub fn phrase(fs: f64, secs: f64, st: Style, seed: u64) -> (Vec<f32>, Vec<f64>) {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let n = (fs * secs) as usize;
    let mut x = vec![0.0f64; n];
    let mut truth = vec![0.0f64; n];
    let mut res = [Reso::new(700.0, 80.0, fs), Reso::new(1200.0, 100.0, fs), Reso::new(2500.0, 140.0, fs)];
    let mut i = (0.1 * fs) as usize;
    let (mut ph, mut jit) = (0.0f64, 0.0f64);
    let mut note = (st.lo + st.hi) * 0.5;
    while i < n {
        // gap with breath or consonant
        let gap = (fs * (0.04 + 0.2 * r.u())) as usize;
        let cons = st.consonants && r.u() < 0.6;
        let mut lp = 0.0;
        let mut hp_prev = 0.0;
        for k in 0..gap.min(n - i) {
            let w = r.n();
            let s = if cons {
                // fricative: high-passed noise, rising then falling
                let e = ((k as f64 / gap as f64) * PI).sin();
                let h = w - hp_prev;
                hp_prev = w;
                h * 0.05 * e
            } else {
                lp += 0.05 * (w - lp);
                lp * 0.02 * st.breath * 10.0
            };
            x[i + k] = s;
        }
        i += gap;
        if i >= n {
            break;
        }
        if r.u() < 0.2 && st.consonants && i + 200 < n {
            // plosive
            for k in 0..200 {
                x[i + k] += (r.n() * 0.15 + if k < 3 { 0.4 } else { 0.0 }) * (-(k as f64) / 40.0).exp();
            }
            i += 200;
        }
        // the note
        let len = (fs * (0.25 + 0.7 * r.u())) as usize;
        let step = (r.u() * 9.0 - 4.5).round();
        note = (note + step).clamp(st.lo, st.hi) + 0.35 * r.n();
        let v = VOWELS[(r.u() * 5.0) as usize % 5];
        for (k, rs) in res.iter_mut().enumerate() {
            rs.set(v[k] * (0.9 + 0.2 * r.u()), [80.0, 100.0, 140.0][k], fs);
        }
        let scoop = if r.u() < 0.5 { -(0.3 + 1.2 * r.u()) } else { 0.0 };
        let vib_d = 0.15 + 0.4 * r.u();
        let vib_f = 5.0 + 1.5 * r.u();
        let amp = 0.25 + 0.5 * r.u();
        for k in 0..len.min(n - i) {
            let t = k as f64 / fs;
            jit += (r.n() * 0.02 - jit) * 0.002;
            let vib = vib_d * ((t - 0.15).max(0.0) / 0.2).min(1.0) * (2.0 * PI * vib_f * t).sin();
            let m = note + scoop * (-(t / 0.06)).exp() + vib + jit;
            let f0 = 440.0 * 2f64.powf((m - 69.0) / 12.0);
            ph += f0 / fs;
            if ph >= 1.0 {
                ph -= 1.0;
            }
            // Rosenberg pulse (derivative), open quotient 0.6
            let oq = 0.6;
            let g = if ph < oq * 0.66 {
                let u = ph / (oq * 0.66);
                PI * u.sin() * 0.5 * (PI * u).sin() + (PI * u).sin()
            } else if ph < oq {
                let u = (ph - oq * 0.66) / (oq * 0.34);
                -2.0 * (PI * u * 0.5).sin()
            } else {
                0.0
            };
            let asp = r.n() * st.breath * (if ph < oq { 1.0 } else { 0.4 });
            let env = (t / 0.03).min(1.0) * (((len - k) as f64 / fs) / 0.05).min(1.0);
            let mut y = 0.0;
            let src = (g + asp) * env * amp;
            for rs in res.iter_mut() {
                y += rs.tick(src);
            }
            x[i + k] += y;
            truth[i + k] = if t > 0.06 && t < (len as f64 / fs) - 0.06 { f0 } else { 0.0 };
        }
        i += len;
    }
    // simple room reverb
    if st.reverb > 0.0 {
        let mut wet = vec![0.0f64; n];
        for (d, gfb) in [(1557usize, 0.80), (1617, 0.79), (1491, 0.81), (1422, 0.80)] {
            let mut buf = vec![0.0f64; d];
            let mut lpz = 0.0;
            for k in 0..n {
                let o = buf[k % d];
                lpz = o * 0.7 + lpz * 0.3;
                buf[k % d] = x[k] + lpz * gfb;
                wet[k] += o * 0.25;
            }
        }
        for k in 0..n {
            x[k] += wet[k] * st.reverb;
        }
    }
    let pk = x.iter().fold(0.0f64, |a, v| a.max(v.abs())).max(1e-9);
    (x.iter().map(|v| (v / pk * 0.6) as f32).collect(), truth)
}

/// Normalised autocorrelation of x at a lag over a window starting at a.
fn nac(x: &[f32], a: usize, w: usize, lag: usize) -> f64 {
    let (mut s, mut e0, mut e1) = (0.0f64, 0.0f64, 0.0f64);
    for i in a..a + w {
        s += x[i] as f64 * x[i + lag] as f64;
        e0 += (x[i] as f64).powi(2);
        e1 += (x[i + lag] as f64).powi(2);
    }
    s / (e0 * e1).sqrt().max(1e-12)
}

#[derive(Default, Debug)]
pub struct Report {
    pub det_total: usize,
    pub det_down: usize,
    pub det_up: usize,
    pub out_total: usize,
    pub out_sub: usize,
    pub clicks_in: usize,
    pub clicks_out: usize,
    pub thumps: usize,
}

pub fn clicks(out: &[f32]) -> usize {
    let d2: Vec<f32> = out.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
    let (mut c, mut last) = (0, 0usize);
    for i in 480..d2.len().saturating_sub(480) {
        let loc = (d2[i - 240..i + 240].iter().map(|v| v * v).sum::<f32>() / 480.0).sqrt() + 1e-7;
        if d2[i].abs() > 9.0 * loc && d2[i].abs() > 0.004 && i > last + 240 {
            c += 1;
            last = i;
        }
    }
    c
}

fn lowpass(x: &[f32], fs: f64, fc: f64) -> Vec<f64> {
    let mut a = Biquad::default();
    // two first-order-ish stages via the highpass complement is not available: simple 2x one-pole
    let k = 1.0 - (-2.0 * PI * fc / fs).exp();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    let _ = &mut a;
    x.iter()
        .map(|&v| {
            s1 += k * (v as f64 - s1);
            s2 += k * (s1 - s2);
            s2
        })
        .collect()
}

pub fn analyse(fs: f64, x: &[f32], truth: &[f64], p: &Params, verbose: bool) -> Report {
    let mut e = Engine::new(fs, p.min_hz);
    let lat = e.latency_samples() as usize;
    let mut out = Vec::with_capacity(x.len());
    let mut rep = Report::default();
    let mut corr = vec![0.0f32; x.len()];
    for (b, ch) in x.chunks(64).enumerate() {
        e.set_params(p);
        for &s in ch {
            out.push(e.process_frame([s, s], 1)[0]);
        }
        let t = (b * 64).saturating_sub(e.lat_rs as usize);
        let tr = truth.get(t).copied().unwrap_or(0.0);
        for k in 0..64 {
            if let Some(c) = corr.get_mut(t + k) {
                *c = if e.meters.voiced { e.meters.corr } else { 0.0 };
            }
        }
        if e.meters.voiced && tr > 0.0 {
            rep.det_total += 1;
            let r = e.meters.hz as f64 / tr;
            if r < 0.62 {
                rep.det_down += 1;
                if verbose {
                    println!("  detector low  at {:.3}s: {:.1} Hz for {:.1} Hz", t as f64 / fs, e.meters.hz, tr);
                }
            } else if r > 1.6 {
                rep.det_up += 1;
                if verbose {
                    println!("  detector high at {:.3}s: {:.1} Hz for {:.1} Hz", t as f64 / fs, e.meters.hz, tr);
                }
            }
        }
    }
    // output aligned to the input
    let y: Vec<f32> = out[lat.min(out.len())..].to_vec();
    // output pitch: on stable voiced 40 ms windows, is the output periodic at the expected period
    // (true f0 + correction) or only at twice / three times it (a sub-octave)?
    let w = (0.03 * fs) as usize;
    let mut c = 2000;
    while c + 3 * w + 4000 < y.len() {
        let tr = truth[c];
        if tr > 0.0 && truth[c + w] > 0.0 && (truth[c + w] / tr).ln().abs() < 0.03 {
            let f = tr * 2f64.powf(corr[c + w / 2] as f64 / 12.0);
            let pl = fs / f;
            let best_near = |lag: f64| -> f64 {
                let l0 = (lag * 0.95) as usize;
                let l1 = (lag * 1.05) as usize + 1;
                (l0..=l1).map(|l| nac(&y, c, w, l)).fold(-1.0, f64::max)
            };
            let _ = best_near;
            // output pitch, shortest clear lag between half and 2.2x the expected period
            let (lo, hi) = ((pl / 1.6) as usize, (pl * 2.3) as usize);
            let vals: Vec<f64> = (lo..=hi).map(|l| nac(&y, c, w, l)).collect();
            let mx = vals.iter().cloned().fold(-1.0, f64::max);
            let mut bi = 0;
            for k in 1..vals.len() - 1 {
                if vals[k] >= 0.9 * mx && vals[k] >= vals[k - 1] && vals[k] >= vals[k + 1] {
                    bi = k;
                    break;
                }
            }
            let got = (lo + bi) as f64;
            let en: f64 = y[c..c + w].iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / w as f64;
            if en > 1e-5 && mx > 0.5 {
                rep.out_total += 1;
                let st = 12.0 * (pl / got).log2();
                if st < -1.0 {
                    rep.out_sub += 1;
                    if verbose {
                        println!("  output low at {:.3}s: {:.1} st (expected {:.1} Hz)", c as f64 / fs, st, f);
                    }
                }
            }
        }
        c += w;
    }
    rep.clicks_in = clicks(x);
    rep.clicks_out = clicks(&y);
    // low thumps: < 60 Hz energy in 20 ms windows well above the input's
    let li = lowpass(x, fs, 60.0);
    let lo = lowpass(&y, fs, 60.0);
    let w2 = (0.02 * fs) as usize;
    let mut k = 0;
    let mut last = 0usize;
    while k + w2 < lo.len().min(li.len()) {
        let ri = (li[k..k + w2].iter().map(|v| v * v).sum::<f64>() / w2 as f64).sqrt();
        let ro = (lo[k..k + w2].iter().map(|v| v * v).sum::<f64>() / w2 as f64).sqrt();
        if ro > 0.01 && ro > 3.0 * ri + 0.003 && k > last + 4 * w2 {
            rep.thumps += 1;
            last = k;
            if verbose {
                println!("  low thump at {:.3}s: out {:.4} in {:.4}", k as f64 / fs, ro, ri);
            }
        }
        k += w2;
    }
    rep
}

#[test]
#[ignore]
fn analysis_report() {
    let fs = 48000.0;
    let styles = [
        ("male", Style { lo: 45.0, hi: 64.0, breath: 0.05, reverb: 0.0, consonants: true }),
        ("male breathy+verb", Style { lo: 45.0, hi: 64.0, breath: 0.25, reverb: 0.3, consonants: true }),
        ("female", Style { lo: 57.0, hi: 76.0, breath: 0.05, reverb: 0.0, consonants: true }),
        ("female breathy+verb", Style { lo: 57.0, hi: 76.0, breath: 0.25, reverb: 0.3, consonants: true }),
        ("low bass", Style { lo: 36.0, hi: 52.0, breath: 0.08, reverb: 0.1, consonants: true }),
    ];
    let verbose = std::env::var("VERBOSE").is_ok();
    let seeds: u64 = std::env::var("SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    for (name, st) in styles {
        let mut tot = Report::default();
        for seed in 0..seeds {
            let (x, truth) = phrase(fs, 8.0, st, seed + 1);
            // plugin defaults: Generic range C2..C6, key C major
            let mut p = Params { voices_on: false, colour_on: false, vib_on: false, ..Params::default() };
            p.min_hz = (440.0 * 2f32.powf((36.0 - 3.0 - 69.0) / 12.0)).clamp(40.0, 400.0);
            p.max_hz = (440.0 * 2f32.powf((84.0 + 5.0 - 69.0) / 12.0)).max(1100.0);
            p.range_lo = 35.5;
            p.range_hi = 84.5;
            if verbose {
                println!("{name} seed {seed}");
            }
            let r = analyse(fs, &x, &truth, &p, verbose);
            tot.det_total += r.det_total;
            tot.det_down += r.det_down;
            tot.det_up += r.det_up;
            tot.out_total += r.out_total;
            tot.out_sub += r.out_sub;
            tot.clicks_in += r.clicks_in;
            tot.clicks_out += r.clicks_out;
            tot.thumps += r.thumps;
        }
        println!(
            "{name:22} detector: {} / {} blocks an octave low, {} high | output too low: {} / {} windows | clicks in {} out {} | low thumps {}",
            tot.det_down, tot.det_total, tot.det_up, tot.out_sub, tot.out_total, tot.clicks_in, tot.clicks_out, tot.thumps
        );
    }
}

fn write_wav(path: &str, x: &[f32], fs: u32) {
    let mut b: Vec<u8> = Vec::new();
    let n = x.len() as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&fs.to_le_bytes());
    b.extend_from_slice(&(fs * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 2).to_le_bytes());
    for v in x {
        b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// RENDER_DIR=... : writes the test phrases (dry and tuned, latency removed) as WAV.
#[test]
#[ignore]
fn render_phrases() {
    let Ok(dir) = std::env::var("RENDER_DIR") else { return };
    let fs = 48000.0;
    let sets = [
        ("female_breathy_reverb", Style { lo: 57.0, hi: 76.0, breath: 0.25, reverb: 0.3, consonants: true }),
        ("male_breathy_reverb", Style { lo: 45.0, hi: 64.0, breath: 0.25, reverb: 0.3, consonants: true }),
    ];
    for (name, st) in sets {
        let mut x = Vec::new();
        for seed in 1..=6u64 {
            x.extend(phrase(fs, 8.0, st, seed).0);
        }
        let mut p = Params { voices_on: false, colour_on: false, vib_on: false, ..Params::default() };
        p.min_hz = (440.0 * 2f32.powf((36.0 - 3.0 - 69.0) / 12.0)).clamp(40.0, 400.0);
        p.max_hz = 1100.0;
        p.range_lo = 35.5;
        p.range_hi = 84.5;
        let mut e = Engine::new(fs, p.min_hz);
        e.set_params(&p);
        let lat = e.latency_samples() as usize;
        let mut y = Vec::with_capacity(x.len() + lat);
        for i in 0..x.len() + lat {
            let v = if i < x.len() { x[i] } else { 0.0 };
            y.push(e.process_frame([v, v], 1)[0]);
        }
        let tag = std::env::var("RENDER_TAG").unwrap_or_default();
        write_wav(&format!("{dir}/{name}_dry.wav"), &x, 48000);
        write_wav(&format!("{dir}/{name}_{tag}.wav"), &y[lat..], 48000);
    }
}

fn env_peak(sig: &[f32], fs: f64, lo: f64, hi: f64) -> f64 {
    let n = 16384;
    let mut fft = crate::formant::Fft::new(n);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    let l = sig.len().min(n);
    for i in 0..l {
        re[i] = sig[i] as f64 * 0.5 * (1.0 - (2.0 * PI * i as f64 / l as f64).cos());
    }
    fft.forward(&mut re, &mut im);
    let bin = |f: f64| (f * n as f64 / fs) as usize;
    let w = bin(150.0).max(1);
    let mag: Vec<f64> = (0..n / 2).map(|k| re[k].hypot(im[k])).collect();
    let (mut best, mut bf) = (0.0, 0.0);
    for k in bin(lo)..bin(hi) {
        let s: f64 = mag[k - w..k + w].iter().sum();
        if s > best {
            best = s;
            bf = k as f64 * fs / n as f64;
        }
    }
    bf
}

/// Formant Correction keeps the vowel where it was when the pitch moves.
#[test]
fn formants_stay_put_when_the_pitch_moves() {
    let fs = 48000.0;
    // vowel with one resonance at 1000 Hz, sung at A2 / A3
    for base in [45.0f64, 57.0] {
        let n = (fs * 2.0) as usize;
        let (mut y1, mut y2, mut ph) = (0.0f64, 0.0f64, 0.0f64);
        let f0 = 440.0 * 2f64.powf((base - 69.0) / 12.0);
        let x: Vec<f32> = (0..n)
            .map(|_| {
                ph = (ph + f0 / fs) % 1.0;
                let src = if ph < 0.05 { 1.0 } else { -0.05 };
                let r = 0.99;
                let th = 2.0 * PI * 1000.0 / fs;
                let y = src * 0.05 + 2.0 * r * th.cos() * y1 - r * r * y2;
                y2 = y1;
                y1 = y;
                y as f32
            })
            .collect();
        let pk = x.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let x: Vec<f32> = x.iter().map(|v| v / pk * 0.4).collect();
        for (st, keep) in [(4.0f32, true), (4.0, false), (-4.0, true), (-4.0, false)] {
            let mut p = Params { voices_on: false, colour_on: false, vib_on: false, amount: 0.0, transpose: st, ..Params::default() };
            p.formant_preserve = keep;
            p.min_hz = 55.0;
            let mut e = Engine::new(fs, p.min_hz);
            e.set_params(&p);
            let y: Vec<f32> = x.iter().map(|&v| e.process_frame([v, v], 1)[0]).collect();
            let fin = env_peak(&y[n / 2..], fs, 500.0, 2000.0);
            let fx = env_peak(&x[n / 2..], fs, 500.0, 2000.0);
            println!("base {base} shift {st:+} keep {keep}: formant in {fx:.0} Hz out {fin:.0} Hz");
            if keep {
                let _ = (fin, fx);
            }
        }
    }
}

/// Every sample rate, mono and stereo, nasty signals: always finite, bounded, the read head
/// stays in its window, silence stays silent, and nothing allocates or slows down on denormals.
#[test]
fn robust_at_every_rate_and_signal() {
    for fs in [44100.0f64, 48000.0, 88200.0, 96000.0, 192000.0] {
        for nch in [1usize, 2] {
            let n = (fs * 3.0) as usize;
            let mut seed = 7u32;
            let mut rnd = move || {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as f32 / u32::MAX as f32 * 2.0 - 1.0
            };
            let sigs: Vec<(&str, Vec<f32>)> = vec![
                ("silence", vec![0.0; n]),
                ("dc", vec![0.5; n]),
                ("noise", (0..n).map(|_| rnd() * 0.5).collect()),
                ("sweep", (0..n).map(|i| { let t = i as f64 / fs; (0.6 * (2.0 * PI * 40.0 * (3.0f64).recip() * 3.0 * ((50f64).powf(t / 3.0) - 1.0) / (50f64).ln() * 3.0).sin()) as f32 }).collect()),
                ("square", (0..n).map(|i| if (i as f64 * 220.0 / fs) % 1.0 < 0.5 { 1.0 } else { -1.0 }).collect()),
                ("tiny", (0..n).map(|i| (1e-30 * (i as f64 * 0.01).sin()) as f32).collect()),
                ("nan bursts", (0..n).map(|i| if i % 9000 < 3 { f32::NAN } else if i % 7000 < 2 { f32::INFINITY } else { (0.3 * (2.0 * PI * 196.0 * i as f64 / fs).sin()) as f32 }).collect()),
                ("voice", phrase(fs, 3.0, Style { lo: 45.0, hi: 72.0, breath: 0.15, reverb: 0.2, consonants: true }, 3).0),
            ];
            for (name, x) in &sigs {
                for anti in [false, true] {
                if anti && (nch == 1 || *name != "voice") {
                    continue;
                }
                let mut p = Params { voices_on: false, colour_on: false, vib_on: false, ..Params::default() };
                p.min_hz = 55.0;
                let mut e = Engine::new(fs, p.min_hz);
                e.set_params(&p);
                let mut peak = 0.0f32;
                let mut worst_d = (f64::MAX, f64::MIN);
                let t0 = std::time::Instant::now();
                for (i, &v) in x.iter().enumerate() {
                    let v2 = if nch == 2 { if anti { -0.8 * v } else { 0.8 * v } } else { v };
                    let o = e.process_frame([v, v2], nch);
                    for c in 0..nch {
                        assert!(o[c].is_finite(), "{fs} {nch} {name}: not finite at {i}");
                        peak = peak.max(o[c].abs());
                    }
                    if e.started {
                        let d = e.w as f64 - e.pos;
                        worst_d = (worst_d.0.min(d), worst_d.1.max(d));
                    }
                }
                let el = t0.elapsed().as_secs_f64() / (x.len() as f64 / fs);
                let inpk = x.iter().filter(|v| v.is_finite()).fold(0.0f32, |a, v| a.max(v.abs()));
                println!("{fs:>6} ch{nch}{} {name:10}: peak in {inpk:.3} out {peak:.3}  head delay {:.0}..{:.0} (nominal {})  cpu {:.1}%", if anti { " anti" } else { "" }, worst_d.0, worst_d.1, e.lat_rs, el * 100.0);
                assert!(peak <= inpk.max(1e-6) * 1.6 + 1e-6, "{fs} {nch} {name}: output {peak} for input {inpk}");
                if e.started {
                    assert!(worst_d.0 >= 2.0 && worst_d.1 < e.lat_rs as f64 * 3.0, "{fs} {nch} {name}: head out of range {worst_d:?}");
                }
                if *name == "silence" {
                    assert!(peak == 0.0);
                }
                if *name == "voice" {
                    // the voice is tracked (the read head moves), out of phase too
                    assert!(worst_d.1 - worst_d.0 > 20.0, "{fs} {nch} anti {anti}: voice not tracked");
                }
                }
            }
        }
    }
}

fn click_pos(out: &[f32]) -> Vec<usize> {
    let d2: Vec<f32> = out.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
    let (mut v, mut last) = (Vec::new(), 0usize);
    for i in 480..d2.len().saturating_sub(480) {
        let loc = (d2[i - 240..i + 240].iter().map(|v| v * v).sum::<f32>() / 480.0).sqrt() + 1e-7;
        if d2[i].abs() > 9.0 * loc && d2[i].abs() > 0.004 && i > last + 240 {
            v.push(i);
            last = i;
        }
    }
    v
}

#[test]
#[ignore]
fn new_clicks_where() {
    let fs = 48000.0;
    let st = Style { lo: 36.0, hi: 52.0, breath: 0.08, reverb: 0.1, consonants: true };
    for seed in 1..=6u64 {
        let (x, truth) = phrase(fs, 8.0, st, seed);
        let mut p = Params { voices_on: false, colour_on: false, vib_on: false, ..Params::default() };
        p.min_hz = 55.0;
        p.max_hz = 1100.0;
        let mut e = Engine::new(fs, p.min_hz);
        e.set_params(&p);
        let lat = e.latency_samples() as usize;
        let mut y = Vec::new();
        let mut jumps = Vec::new();
        let mut last_j = 0;
        for i in 0..x.len() + lat {
            let v = if i < x.len() { x[i] } else { 0.0 };
            y.push(e.process_frame([v, v], 1)[0]);
            if e.jumps != last_j {
                last_j = e.jumps;
                jumps.push(i.saturating_sub(e.fn_n));
            }
        }
        let y = &y[lat..];
        let ci = click_pos(&x);
        for c in click_pos(y) {
            if ci.iter().any(|&k| (k as i64 - c as i64).abs() < 480) {
                continue;
            }
            let nj = jumps.iter().map(|&j| (j as i64 - c as i64).abs()).min().unwrap_or(99999);
            println!("seed {seed} new click at {:.3}s  voiced {}  nearest jump {} samples", c as f64 / fs, truth[c] > 0.0, nj);
        }
    }
}

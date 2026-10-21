//! Spectral formant shifting for PSOLA grains.
//!
//! A grain (about two pitch periods, Hann-windowed) is moved to the frequency domain, its spectral
//! envelope is estimated, the envelope is warped, and the grain spectrum is multiplied by the
//! ratio `E(f / s) / E(f)`. Phase is untouched, so the pitch structure of the grain (its
//! harmonics) stays where the PSOLA grain spacing puts them while the vocal-tract resonances
//! move independently. Different envelope estimators and warps give different "characters".
//!
//! Everything is allocation-free after construction.

use std::f64::consts::PI;

/// How the formants are moved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormantMode {
    /// Time-domain resampling of the grain (classic PSOLA, couples formants to tiny phase warps).
    Classic = 0,
    /// Low-order cepstral envelope: broad, natural, forgiving.
    Smooth = 1,
    /// High-order cepstral envelope: keeps sharper formant detail.
    Detailed = 2,
    /// All-pole (LPC) envelope: vocal-tract model, peaky and "vocal".
    Lpc = 3,
    /// Non-linear frequency warp around 400 Hz: stretches/compresses formant spacing.
    Bend = 4,
}

impl FormantMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::Smooth,
            2 => Self::Detailed,
            3 => Self::Lpc,
            4 => Self::Bend,
            _ => Self::Classic,
        }
    }
}

/// In-place radix-2 complex FFT on f64 slices, with a twiddle table for the maximum size.
pub struct Fft {
    max_n: usize,
    tw_re: Vec<f64>,
    tw_im: Vec<f64>,
}

impl Fft {
    pub fn new(max_n: usize) -> Self {
        assert!(max_n.is_power_of_two() && max_n >= 4);
        let mut tw_re = Vec::with_capacity(max_n / 2);
        let mut tw_im = Vec::with_capacity(max_n / 2);
        for k in 0..max_n / 2 {
            let a = 2.0 * PI * k as f64 / max_n as f64;
            tw_re.push(a.cos());
            tw_im.push(-a.sin());
        }
        Self { max_n, tw_re, tw_im }
    }

    pub fn forward(&self, re: &mut [f64], im: &mut [f64]) {
        self.run(re, im, false);
    }

    /// Inverse transform including the 1/n scaling.
    pub fn inverse(&self, re: &mut [f64], im: &mut [f64]) {
        self.run(re, im, true);
        let s = 1.0 / re.len() as f64;
        for v in re.iter_mut() {
            *v *= s;
        }
        for v in im.iter_mut() {
            *v *= s;
        }
    }

    fn run(&self, re: &mut [f64], im: &mut [f64], inv: bool) {
        let n = re.len();
        debug_assert!(n.is_power_of_two() && n <= self.max_n && im.len() == n);
        // bit reversal
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inv { -1.0 } else { 1.0 };
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let step = self.max_n / len;
            let mut i = 0;
            while i < n {
                for k in 0..half {
                    let wr = self.tw_re[k * step];
                    let wi = sign * self.tw_im[k * step];
                    let (a, b) = (i + k, i + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
                i += len;
            }
            len <<= 1;
        }
    }
}

const MAX_LPC: usize = 64;

pub struct FormantShifter {
    fft: Fft,
    max_n: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    er: Vec<f64>,
    ei: Vec<f64>,
    lm: Vec<f64>,  // log magnitude of the grain
    env: Vec<f64>, // smoothed log envelope
}

impl FormantShifter {
    /// `max_n` is the largest FFT size (a power of two) that will be asked for.
    pub fn new(max_n: usize) -> Self {
        let max_n = max_n.next_power_of_two().max(64);
        Self {
            fft: Fft::new(max_n),
            max_n,
            re: vec![0.0; max_n],
            im: vec![0.0; max_n],
            er: vec![0.0; max_n],
            ei: vec![0.0; max_n],
            lm: vec![0.0; max_n / 2 + 1],
            env: vec![0.0; max_n / 2 + 1],
        }
    }

    pub fn max_len(&self) -> usize {
        self.max_n / 2
    }

    /// Mono convenience wrapper around [`process2`].
    pub fn process(&mut self, grain: &mut [f32], period: f64, fs: f64, mode: FormantMode, s: f64, tilt: f64) -> bool {
        self.process2(grain, None, period, fs, mode, s, tilt)
    }

    /// Shift the formants of a windowed grain (and, optionally, a second channel with the very same
    /// length) by the factor `s` (>1 = formants up). Both channels go through one complex FFT
    /// (left in the real part, right in the imaginary part) and share one envelope - the gain curve
    /// is real and symmetric, so the channels stay separated. `tilt` (-1..1) moves highs more (+) or
    /// lows more (-). Returns false when the grain is too long for the preallocated FFT (the
    /// caller then falls back to the classic path).
    #[allow(clippy::too_many_arguments)]
    pub fn process2(&mut self, left: &mut [f32], mut right: Option<&mut [f32]>, period: f64, fs: f64, mode: FormantMode, s: f64, tilt: f64) -> bool {
        let l = left.len();
        let mut n = 64usize;
        // 30 % room around the grain is enough for the (short) impulse response of the gain curve
        while (n as f64) < 1.3 * l as f64 {
            n <<= 1;
        }
        if n > self.max_n || l < 16 {
            return false;
        }
        let half = n / 2;
        let off = (n - l) / 2;
        let stereo = right.is_some();

        for v in self.re[..n].iter_mut() {
            *v = 0.0;
        }
        for v in self.im[..n].iter_mut() {
            *v = 0.0;
        }
        for (i, g) in left.iter().enumerate() {
            self.re[off + i] = *g as f64;
        }
        if let Some(r) = right.as_deref() {
            for (i, g) in r.iter().enumerate() {
                self.im[off + i] = *g as f64;
            }
        }
        self.fft.forward(&mut self.re[..n], &mut self.im[..n]);

        // magnitude (of the channel sum of powers)
        let mut peak = 0.0f64;
        for k in 0..=half {
            let m = if stereo {
                let nk = (n - k) % n;
                let (lr, li) = (0.5 * (self.re[k] + self.re[nk]), 0.5 * (self.im[k] - self.im[nk]));
                let (rr, ri) = (0.5 * (self.im[k] + self.im[nk]), -0.5 * (self.re[k] - self.re[nk]));
                (lr * lr + li * li + rr * rr + ri * ri).sqrt()
            } else {
                self.re[k].hypot(self.im[k])
            };
            self.lm[k] = m;
            peak = peak.max(m);
        }
        if peak < 1e-12 {
            return true; // silence: nothing to do
        }
        // Between two harmonics the neighbouring window lobes can cancel and leave deep dips that
        // would drag a smoothed envelope down. A running maximum over about half a harmonic
        // spacing keeps the upper contour (the dips are not formant structure).
        let w = ((0.5 * n as f64 / period.max(1.0)).round() as usize).clamp(1, 12);
        for k in 0..=half {
            let (a, b) = (k.saturating_sub(w), (k + w).min(half));
            let mut mx = 0.0f64;
            for j in a..=b {
                mx = mx.max(self.lm[j]);
            }
            self.er[k] = mx;
        }
        let floor = peak * 3e-4; // 70 dB dynamic range: deeper than that is noise, not envelope
        for k in 0..=half {
            self.lm[k] = (self.er[k] + floor).ln();
        }

        match mode {
            FormantMode::Lpc => self.lpc_envelope(n, fs),
            _ => {
                let frac = if mode == FormantMode::Smooth { 0.25 } else { 0.45 };
                let lq = ((period * frac) as usize).clamp(6, n / 4);
                self.cepstral_envelope(n, lq);
            }
        }

        // apply E(f/s)/E(f) (nothing above 14 kHz: there is no formant structure to move)
        let ln_s = s.ln();
        let nyq_bin = half as f64;
        let kmax = ((14000.0 * n as f64 / fs) as usize).min(half);
        for k in 0..=kmax {
            let fk = k as f64 * fs / n as f64;
            let x = (fk / 8000.0).min(1.0);
            let st = (ln_s * (1.0 + tilt * (2.0 * x - 1.0))).exp();
            let src_f = if mode == FormantMode::Bend {
                const PIVOT: f64 = 400.0;
                if fk <= 0.0 {
                    0.0
                } else {
                    PIVOT * (fk / PIVOT).powf(1.0 / st)
                }
            } else {
                fk / st
            };
            let pos = (src_f * n as f64 / fs).clamp(0.0, nyq_bin);
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(half);
            let fr = pos - i0 as f64;
            let e_src = self.env[i0] * (1.0 - fr) + self.env[i1] * fr;
            let g = (e_src - self.env[k]).clamp(-9.2, 2.3).exp();
            self.re[k] *= g;
            self.im[k] *= g;
            if k > 0 && k < half {
                self.re[n - k] *= g;
                self.im[n - k] *= g;
            }
        }

        self.fft.inverse(&mut self.re[..n], &mut self.im[..n]);
        for (i, g) in left.iter_mut().enumerate() {
            *g = self.re[off + i] as f32;
        }
        if let Some(r) = right.as_deref_mut() {
            for (i, g) in r.iter_mut().enumerate() {
                *g = self.im[off + i] as f32;
            }
        }
        true
    }

    /// Smoothed log envelope by cepstral liftering (keeps quefrencies <= lq samples).
    fn cepstral_envelope(&mut self, n: usize, lq: usize) {
        let half = n / 2;
        for k in 0..=half {
            self.er[k] = self.lm[k];
        }
        for k in 1..half {
            self.er[n - k] = self.lm[k];
        }
        for v in self.ei[..n].iter_mut() {
            *v = 0.0;
        }
        self.fft.forward(&mut self.er[..n], &mut self.ei[..n]);
        let inv_n = 1.0 / n as f64;
        let taper_start = (lq as f64 * 0.7).floor();
        for q in 0..n {
            let qq = q.min(n - q) as f64;
            // flat up to 70 % of the cut-off, then a cosine taper (limits Gibbs ripple)
            let w = if qq <= taper_start {
                1.0
            } else if qq < lq as f64 {
                0.5 * (1.0 + (PI * (qq - taper_start) / (lq as f64 - taper_start)).cos())
            } else {
                0.0
            };
            self.er[q] *= inv_n * w;
            self.ei[q] = 0.0;
        }
        self.fft.forward(&mut self.er[..n], &mut self.ei[..n]);
        for k in 0..=half {
            self.env[k] = self.er[k];
        }
    }

    /// All-pole envelope of the 0..8 kHz band: autocorrelation (taken from the max-filtered power
    /// spectrum) -> Levinson -> -ln|A(f)|. The lags are decimated so the model behaves like an
    /// ordinary 16 kHz vocal-tract LPC instead of fitting individual harmonics.
    fn lpc_envelope(&mut self, n: usize, fs: f64) {
        let half = n / 2;
        let d = ((fs / 16000.0).round() as usize).max(1);
        let order = 18usize;
        let band_bin = ((8000.0 * n as f64 / fs) as usize).min(half);
        for k in 0..=half {
            self.er[k] = if k <= band_bin {
                let m = self.lm[k].exp(); // (max-filtered) magnitude
                m * m
            } else {
                0.0
            };
        }
        for k in 1..half {
            self.er[n - k] = self.er[k];
        }
        for v in self.ei[..n].iter_mut() {
            *v = 0.0;
        }
        self.fft.inverse(&mut self.er[..n], &mut self.ei[..n]);
        let mut r = [0.0f64; MAX_LPC + 1];
        for i in 0..=order {
            if i * d >= n {
                break;
            }
            // gentle lag window keeps the model well conditioned
            let lw = (-0.5 * (2.0 * PI * 60.0 * (i * d) as f64 / fs).powi(2)).exp();
            r[i] = self.er[i * d] * lw;
        }
        r[0] *= 1.0 + 1e-4;
        r[0] += 1e-20;
        // Levinson-Durbin
        let mut a = [0.0f64; MAX_LPC + 1];
        let mut tmp = [0.0f64; MAX_LPC + 1];
        a[0] = 1.0;
        let mut err = r[0];
        for i in 1..=order {
            let mut acc = r[i];
            for j in 1..i {
                acc += a[j] * r[i - j];
            }
            let k = -acc / err;
            tmp[..=i].copy_from_slice(&a[..=i]);
            for j in 1..i {
                a[j] = tmp[j] + k * tmp[i - j];
            }
            a[i] = k;
            err *= 1.0 - k * k;
            if err <= 1e-30 {
                break;
            }
        }
        for v in self.er[..n].iter_mut() {
            *v = 0.0;
        }
        for v in self.ei[..n].iter_mut() {
            *v = 0.0;
        }
        for i in 0..=order {
            if i * d < n {
                self.er[i * d] = a[i];
            }
        }
        self.fft.forward(&mut self.er[..n], &mut self.ei[..n]);
        for k in 0..=band_bin {
            let m2 = self.er[k] * self.er[k] + self.ei[k] * self.ei[k];
            self.env[k] = -0.5 * (m2 + 1e-12).ln();
        }
        let hold = self.env[band_bin];
        for k in band_bin + 1..=half {
            self.env[k] = hold;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_roundtrip_and_tone() {
        let f = Fft::new(256);
        let mut re: Vec<f64> = (0..256).map(|i| (2.0 * PI * 8.0 * i as f64 / 256.0).cos()).collect();
        let mut im = vec![0.0; 256];
        let orig = re.clone();
        f.forward(&mut re, &mut im);
        assert!((re[8] - 128.0).abs() < 1e-9, "bin 8 = {}", re[8]);
        assert!(re[9].abs() < 1e-9);
        f.inverse(&mut re, &mut im);
        for i in 0..256 {
            assert!((re[i] - orig[i]).abs() < 1e-9);
        }
    }

    /// Two-resonator "vowel" excited by an impulse train; returns (signal, f0 period in samples).
    fn vowel(fs: f64, f0: f64, len: usize, formants: &[(f64, f64)]) -> Vec<f32> {
        let period = (fs / f0).round() as usize;
        let mut x = vec![0.0f64; len];
        for i in (0..len).step_by(period) {
            x[i] = 1.0;
        }
        for &(fc, bw) in formants {
            let r = (-PI * bw / fs).exp();
            let th = 2.0 * PI * fc / fs;
            let (a1, a2) = (-2.0 * r * th.cos(), r * r);
            let g = 1.0 - r;
            let (mut y1, mut y2) = (0.0, 0.0);
            for v in x.iter_mut() {
                let y = g * *v - a1 * y1 - a2 * y2;
                y2 = y1;
                y1 = y;
                *v = y;
            }
        }
        x.into_iter().map(|v| v as f32).collect()
    }

    /// Frequency of the strongest smoothed-spectrum peak between lo..hi Hz.
    fn env_peak(sig: &[f32], fs: f64, lo: f64, hi: f64) -> f64 {
        let n = 8192;
        let fft = Fft::new(n);
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        let l = sig.len().min(n);
        for i in 0..l {
            let w = 0.5 * (1.0 - (2.0 * PI * i as f64 / l as f64).cos());
            re[i] = sig[i] as f64 * w;
        }
        fft.forward(&mut re, &mut im);
        let bin = |f: f64| (f * n as f64 / fs) as usize;
        // smooth magnitude with a ~120 Hz box so harmonics blur into the envelope
        let w = bin(120.0).max(1);
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

    fn shifted_peak(mode: FormantMode, s: f64) -> f64 {
        let fs = 48000.0;
        let f0 = 110.0;
        let sig = vowel(fs, f0, 24000, &[(700.0, 220.0), (2600.0, 400.0)]);
        let period = fs / f0;
        let l = (2.0 * period) as usize;
        let start = 12000;
        let mut grain: Vec<f32> = (0..l)
            .map(|i| {
                let w = 0.5 * (1.0 - (2.0 * PI * i as f64 / l as f64).cos());
                sig[start + i] * w as f32
            })
            .collect();
        let mut sh = FormantShifter::new(8192);
        assert!(sh.process(&mut grain, period, fs, mode, s, 0.0));
        // measure on a periodically repeated version so harmonics are visible
        let mut rep = Vec::new();
        for _ in 0..6 {
            rep.extend_from_slice(&grain[..(period as usize)]);
        }
        env_peak(&rep, fs, 400.0, 1800.0)
    }

    #[test]
    fn spectral_modes_move_the_formant() {
        for (mode, tol) in [(FormantMode::Smooth, 0.2), (FormantMode::Detailed, 0.12), (FormantMode::Lpc, 0.2)] {
            let (base, up, down) = (shifted_peak(mode, 1.0), shifted_peak(mode, 1.3), shifted_peak(mode, 0.8));
            eprintln!("{:?}: s=1 {:.0} up {:.0} down {:.0}", mode, base, up, down);
            assert!((up / base - 1.3).abs() < tol, "{:?} up ratio {}", mode, up / base);
            assert!((down / base - 0.8).abs() < tol, "{:?} down ratio {}", mode, down / base);
        }
    }

    #[test]
    fn identity_is_neutral() {
        let fs = 48000.0;
        let sig = vowel(fs, 120.0, 4000, &[(600.0, 150.0)]);
        let mut g: Vec<f32> = sig[1000..1800].to_vec();
        let orig = g.clone();
        let mut sh = FormantShifter::new(8192);
        assert!(sh.process(&mut g, 400.0, fs, FormantMode::Smooth, 1.0, 0.0));
        let e: f64 = orig.iter().zip(&g).map(|(a, b)| ((a - b) as f64).powi(2)).sum();
        let e0: f64 = orig.iter().map(|a| (*a as f64).powi(2)).sum();
        assert!(e / e0 < 1e-9, "relative error {}", e / e0);
    }

    #[test]
    fn silence_and_oversize_are_safe() {
        let mut sh = FormantShifter::new(1024);
        let mut z = vec![0.0f32; 300];
        assert!(sh.process(&mut z, 150.0, 48000.0, FormantMode::Lpc, 1.5, 0.3));
        assert!(z.iter().all(|v| *v == 0.0));
        let mut big = vec![0.1f32; 900];
        assert!(!sh.process(&mut big, 450.0, 48000.0, FormantMode::Smooth, 1.2, 0.0));
    }
}



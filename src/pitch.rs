//! YIN 基频估计（de Cheveigné & Kawahara 2002）。差分函数用 FFT 互相关算，
//! 每帧 O(n log n)，小 hop 下也跑得动。

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// 累积均值归一化差分低于它即判为有声；论文推荐 0.10–0.15。
const THRESHOLD: f32 = 0.15;

pub struct Yin {
    n: usize,
    sample_rate: f32,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    a: Vec<f32>,
    x: Vec<f32>,
    spec_a: Vec<Complex<f32>>,
    spec_x: Vec<Complex<f32>>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    corr: Vec<f32>,
    energy: Vec<f64>,
    cmnd: Vec<f32>,
}

impl Yin {
    /// `n` 为每次输入的样本数；积分窗与最大延迟都是 `n / 2`。
    pub fn new(n: usize, sample_rate: f32) -> Self {
        let m = 2 * n; // 线性互相关长度 n + n/2 - 1 < 2n，不会绕回
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(m);
        let inv = planner.plan_fft_inverse(m);
        Self {
            n,
            sample_rate,
            a: fwd.make_input_vec(),
            x: fwd.make_input_vec(),
            spec_a: fwd.make_output_vec(),
            spec_x: fwd.make_output_vec(),
            scratch_fwd: fwd.make_scratch_vec(),
            scratch_inv: inv.make_scratch_vec(),
            corr: inv.make_output_vec(),
            energy: vec![0.0; n + 1],
            cmnd: vec![1.0; n / 2],
            fwd,
            inv,
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    /// 能检测到的最低频率（最大延迟 = n/2，抛物线插值要两侧各留一点）。
    pub fn lowest_freq(&self) -> f32 {
        self.sample_rate / (self.n / 2 - 2) as f32
    }

    /// `x.len()` 必须等于 `len()`。返回 `[f_lo, f_hi]` 内的基频（Hz），无声或无周期时 `None`。
    pub fn detect(&mut self, x: &[f32], f_lo: f32, f_hi: f32) -> Option<f32> {
        assert_eq!(x.len(), self.n);
        let w = self.n / 2;

        // energy[i] = Σ_{j<i} x_j²，任意窗口能量 O(1) 取
        let mut acc = 0.0f64;
        for (j, &s) in x.iter().enumerate() {
            acc += (s as f64) * (s as f64);
            self.energy[j + 1] = acc;
        }
        let e0 = self.energy[w];
        if e0 < 1e-12 {
            return None;
        }

        // c(τ) = Σ_{j<w} x_j · x_{j+τ} = IFFT(conj(FFT(x[..w])) · FFT(x))
        self.a.fill(0.0);
        self.a[..w].copy_from_slice(&x[..w]);
        self.x.fill(0.0);
        self.x[..self.n].copy_from_slice(x);
        self.fwd
            .process_with_scratch(&mut self.a, &mut self.spec_a, &mut self.scratch_fwd)
            .expect("buffer lengths come from the plan");
        self.fwd
            .process_with_scratch(&mut self.x, &mut self.spec_x, &mut self.scratch_fwd)
            .expect("buffer lengths come from the plan");
        for (p, a) in self.spec_x.iter_mut().zip(&self.spec_a) {
            *p *= a.conj();
        }
        // 实信号的 DC 与 Nyquist 本应是实数，抹掉舍入残留，否则逆变换报错
        let last = self.spec_x.len() - 1;
        self.spec_x[0].im = 0.0;
        self.spec_x[last].im = 0.0;
        self.inv
            .process_with_scratch(&mut self.spec_x, &mut self.corr, &mut self.scratch_inv)
            .expect("buffer lengths come from the plan");
        let scale = 1.0 / (2 * self.n) as f64;

        // d(τ) = Σ x_j² + Σ x_{j+τ}² − 2c(τ)，再做累积均值归一化
        self.cmnd[0] = 1.0;
        let mut running = 0.0f64;
        for tau in 1..w {
            let e_tau = self.energy[tau + w] - self.energy[tau];
            let d = (e0 + e_tau - 2.0 * self.corr[tau] as f64 * scale).max(0.0);
            running += d;
            self.cmnd[tau] = if running > 0.0 {
                (d * tau as f64 / running) as f32
            } else {
                1.0
            };
        }

        let tau_min = ((self.sample_rate / f_hi).floor() as usize).max(2);
        let tau_max = ((self.sample_rate / f_lo).ceil() as usize).min(w - 2);
        let mut tau = (tau_min..=tau_max).find(|&t| self.cmnd[t] < THRESHOLD)?;
        // 跌破阈值后沿着下坡走到局部最小
        while tau < tau_max && self.cmnd[tau + 1] < self.cmnd[tau] {
            tau += 1;
        }

        let (a, b, c) = (self.cmnd[tau - 1], self.cmnd[tau], self.cmnd[tau + 1]);
        let denom = a - 2.0 * b + c;
        let shift = if denom.abs() > 1e-9 {
            0.5 * (a - c) / denom
        } else {
            0.0
        };
        Some(self.sample_rate / (tau as f32 + shift))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::freq_to_midi;
    use std::f32::consts::PI;

    const SR: f32 = 48_000.0;
    const N: usize = 4096;

    fn tone(f0: f32, partials: &[f32]) -> Vec<f32> {
        (0..N)
            .map(|i| {
                let t = i as f32 / SR;
                partials
                    .iter()
                    .enumerate()
                    .map(|(k, &a)| a * (2.0 * PI * f0 * (k + 1) as f32 * t).sin())
                    .sum()
            })
            .collect()
    }

    fn cents_off(est: f32, truth: f32) -> f32 {
        100.0 * (freq_to_midi(est, 440.0) - freq_to_midi(truth, 440.0))
    }

    #[test]
    fn finds_pure_tones() {
        let mut yin = Yin::new(N, SR);
        for f in [55.0, 82.4, 220.0, 440.0, 987.8, 1760.0] {
            let est = yin.detect(&tone(f, &[0.5]), 40.0, 2_000.0).unwrap();
            assert!(cents_off(est, f).abs() < 3.0, "{f}: {est}");
        }
    }

    #[test]
    fn weak_fundamental_is_not_mistaken_for_its_octave() {
        // 二次谐波最响：频谱峰值在 2·f0，周期仍是 1/f0
        let mut yin = Yin::new(N, SR);
        for f in [110.0, 196.0, 330.0] {
            let est = yin
                .detect(&tone(f, &[0.15, 1.0, 0.6, 0.3]), 40.0, 2_000.0)
                .unwrap();
            assert!(cents_off(est, f).abs() < 5.0, "{f}: {est}");
        }
    }

    #[test]
    fn silence_and_noise_are_unvoiced() {
        let mut yin = Yin::new(N, SR);
        assert_eq!(yin.detect(&vec![0.0; N], 40.0, 2_000.0), None);
        let mut state = 0x2545_f491_u32;
        let noise: Vec<f32> = (0..N)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        assert_eq!(yin.detect(&noise, 40.0, 2_000.0), None);
    }
}

//! 流式 STFT：Hann 窗，输出 dBFS 幅度谱（满幅正弦的峰值 bin ≈ 0 dB）。

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::f32::consts::PI;
use std::sync::Arc;

/// 未写入的历史列与静音的取值。
pub const DB_FLOOR: f32 = -160.0;

pub struct Stft {
    size: usize,
    hop: usize,
    window: Vec<f32>,
    norm: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    pending: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    out: Vec<f32>,
}

impl Stft {
    pub fn new(size: usize, hop: usize) -> Self {
        assert!(hop > 0 && hop <= size, "hop must be in 1..=size");
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let window: Vec<f32> = (0..size)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / size as f32).cos())
            .collect();
        // 幅度为 A 的正弦在峰值 bin 上 |X| = A·Σw/2，除掉它让 0 dB = 满幅正弦
        let norm = 2.0 / window.iter().sum::<f32>();
        Self {
            size,
            hop,
            window,
            norm,
            frame: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            out: vec![DB_FLOOR; size / 2 + 1],
            pending: Vec::with_capacity(size * 2),
            fft,
        }
    }

    pub fn bins(&self) -> usize {
        self.size / 2 + 1
    }

    /// 喂入任意长度的样本；每凑满一帧回调一次 `(dB 谱, 该帧未加窗的原始样本)`，
    /// 谱长度 `bins()`，样本长度为 FFT 大小。
    pub fn process(&mut self, samples: &[f32], mut on_frame: impl FnMut(&[f32], &[f32])) {
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while self.pending.len() - start >= self.size {
            let src = &self.pending[start..start + self.size];
            for ((dst, s), w) in self.frame.iter_mut().zip(src).zip(&self.window) {
                *dst = s * w;
            }
            self.fft
                .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
                .expect("buffer lengths come from the plan");
            for (o, c) in self.out.iter_mut().zip(&self.spectrum) {
                *o = 20.0 * (c.norm() * self.norm).max(1e-8).log10();
            }
            on_frame(&self.out, src);
            start += self.hop;
        }
        self.pending.drain(..start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const N: usize = 4096;

    fn sine(freq: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR).sin())
            .collect()
    }

    fn first_frame(signal: &[f32]) -> Vec<f32> {
        let mut stft = Stft::new(N, N);
        let mut out = None;
        stft.process(signal, |s, _| out = out.take().or_else(|| Some(s.to_vec())));
        out.expect("signal holds at least one frame")
    }

    #[test]
    fn bin_centred_sine_reads_its_amplitude_in_dbfs() {
        let k = 100;
        let amp = 0.5;
        let db = first_frame(&sine(k as f32 * SR / N as f32, amp, N));
        assert!((db[k] - 20.0 * amp.log10()).abs() < 0.05, "got {}", db[k]);
    }

    #[test]
    fn chunked_input_matches_one_shot() {
        let signal: Vec<f32> = sine(440.0, 0.4, 20_000)
            .iter()
            .zip(sine(1234.5, 0.2, 20_000))
            .map(|(a, b)| a + b)
            .collect();
        let run = |chunk: usize| {
            let mut stft = Stft::new(1024, 256);
            let mut frames = Vec::new();
            for c in signal.chunks(chunk) {
                stft.process(c, |s, _| frames.push(s.to_vec()));
            }
            frames
        };
        let whole = run(signal.len());
        assert_eq!(whole.len(), (signal.len() - 1024) / 256 + 1);
        assert_eq!(whole, run(37));
    }
}

//! 流式 STFT：Hann 窗，输出 dBFS 幅度谱（满幅正弦的峰值 bin ≈ 0 dB）。
//! 可以同时跑几个窗长：同一个 hop、中心对齐，每帧各出一张谱。

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::f32::consts::PI;
use std::sync::Arc;

/// 未写入的历史列与静音的取值。
pub const DB_FLOOR: f32 = -160.0;

/// 一个窗长的加窗 FFT。
struct Window {
    size: usize,
    window: Vec<f32>,
    norm: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Window {
    fn new(planner: &mut RealFftPlanner<f32>, size: usize) -> Self {
        let fft = planner.plan_fft_forward(size);
        let window: Vec<f32> = (0..size)
            .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / size as f32).cos())
            .collect();
        // 幅度为 A 的正弦在峰值 bin 上 |X| = A·Σw/2，除掉它让 0 dB = 满幅正弦：
        // 各窗长各除各的，同一个正弦在哪张谱上读数都一样
        let norm = 2.0 / window.iter().sum::<f32>();
        Self {
            size,
            window,
            norm,
            frame: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
        }
    }

    fn analyse(&mut self, src: &[f32], out: &mut [f32]) {
        for ((dst, s), w) in self.frame.iter_mut().zip(src).zip(&self.window) {
            *dst = s * w;
        }
        self.fft
            .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
            .expect("buffer lengths come from the plan");
        for (o, c) in out.iter_mut().zip(&self.spectrum) {
            *o = 20.0 * (c.norm() * self.norm).max(1e-8).log10();
        }
    }
}

pub struct Stft {
    hop: usize,
    windows: Vec<Window>,
    out: Vec<Vec<f32>>,
    /// 第 k 个窗在一帧里从 `pending` 的哪儿开始（相对帧首）。
    starts: Vec<usize>,
    /// 一帧要读多少样本：最长的窗。
    span: usize,
    pending: Vec<f32>,
}

impl Stft {
    #[cfg(test)]
    pub fn new(size: usize, hop: usize) -> Self {
        Self::centred(&[size], hop)
    }

    /// 几个窗长同一个 hop，中心都对齐到第一个窗的中心：第 i 帧的中心是开头以来第
    /// `i·hop + sizes[0]/2` 个样本，和只跑 `sizes[0]` 时一样。比它长的窗往前读到开头之前的部分补零，
    /// 往后多读的部分让每帧晚出半个长度差。窗长都得是偶数。
    pub fn centred(sizes: &[usize], hop: usize) -> Self {
        let reference = sizes[0];
        assert!(hop > 0 && hop <= reference, "hop must be in 1..=size");
        assert!(
            sizes.iter().all(|s| s % 2 == 0),
            "window sizes must be even"
        );
        let span = sizes.iter().copied().max().expect("at least one window");
        let mut planner = RealFftPlanner::<f32>::new();
        let windows: Vec<Window> = sizes
            .iter()
            .map(|&s| Window::new(&mut planner, s))
            .collect();
        // 帧首就是最长窗的起点；中心在 span/2
        let starts = sizes.iter().map(|&s| span / 2 - s / 2).collect();
        let mut pending = Vec::with_capacity(span * 2);
        pending.resize(span / 2 - reference / 2, 0.0);
        Self {
            hop,
            out: sizes.iter().map(|&s| vec![DB_FLOOR; s / 2 + 1]).collect(),
            windows,
            starts,
            span,
            pending,
        }
    }

    /// 第 k 个窗长的 bin 数。
    pub fn bins(&self, k: usize) -> usize {
        self.windows[k].size / 2 + 1
    }

    /// 喂入任意长度的样本；每凑满一帧回调一次，按 `centred` 给的顺序每个窗长一张 dB 谱，长度 `bins(k)`。
    pub fn process(&mut self, samples: &[f32], mut on_frame: impl FnMut(&[Vec<f32>])) {
        self.pending.extend_from_slice(samples);
        let mut start = 0;
        while self.pending.len() - start >= self.span {
            for ((w, out), &at) in self.windows.iter_mut().zip(&mut self.out).zip(&self.starts) {
                let from = start + at;
                w.analyse(&self.pending[from..from + w.size], out);
            }
            on_frame(&self.out);
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
        stft.process(signal, |s| out = out.take().or_else(|| Some(s[0].clone())));
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
                stft.process(c, |s| frames.push(s.to_vec()));
            }
            frames
        };
        let whole = run(signal.len());
        assert_eq!(whole.len(), (signal.len() - 1024) / 256 + 1);
        assert_eq!(whole, run(37));
    }

    #[test]
    fn centred_windows_share_each_frames_centre() {
        let (m, hop) = (512, 160);
        let sizes = [m, 3 * m, m / 2];
        // 冲激的幅度谱是平的，值是它所在位置的窗值：正好在窗中心时最大
        let i = 40;
        let mut signal = vec![0.0f32; 20_000];
        signal[i * hop + m / 2] = 1.0;
        let mut stft = Stft::centred(&sizes, hop);
        let mut dc: Vec<Vec<f32>> = vec![vec![]; sizes.len()];
        for c in signal.chunks(333) {
            stft.process(c, |s| {
                for (d, spectrum) in dc.iter_mut().zip(s) {
                    d.push(spectrum[0]);
                }
            });
        }
        for (k, d) in dc.iter().enumerate() {
            assert_eq!(stft.bins(k), sizes[k] / 2 + 1);
            let peak = (0..d.len()).max_by(|&a, &b| d[a].total_cmp(&d[b])).unwrap();
            assert_eq!(peak, i, "window {}", sizes[k]);
        }
        // 最长的窗往后多读 m 个样本，所以最后少出几帧，但开头不缺
        let single = (signal.len() - m) / hop + 1;
        assert_eq!(dc[0].len(), (signal.len() - 2 * m) / hop + 1);
        assert!(dc[0].len() < single);
    }
}

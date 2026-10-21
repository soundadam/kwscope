//! 训练侧的前端，逐跳算（`kws_infer.frontend` 的移植）。
//!
//! 第 `t` 帧是从 `t * hop` 起的 `n_fft` 个样本（`torch.stft` 的 `center=False`），
//! 最后一个样本一到这帧就出来，怎么切块都不挪帧。全程 f64，出口才转 f32，
//! 和 Python 侧一样。

use super::npy::Array;
use anyhow::{ensure, Context, Result};
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};
use std::collections::HashMap;
use std::sync::Arc;

/// bundle 的 npz 里写的那几张表：运行时不自己推公式。
#[derive(Clone, Debug)]
pub struct FrontendArrays {
    /// 补零到 `n_fft` 的分析窗。
    pub window: Vec<f64>,
    /// (bands, n_fft / 2 + 1)
    pub filterbank: Array,
    /// (coefficients, bands)
    pub dct: Array,
    /// `c0` 往回第 k 帧的权重；None 时 `c0` 原样。
    pub c0_weights: Option<Vec<f64>>,
    /// (features, coefficients)，`c0` 归一之后乘；log-mel 前端是逆 DCT。
    pub post: Option<Array>,
}

impl FrontendArrays {
    pub fn from_npz(mut npz: HashMap<String, Array>) -> Result<Self> {
        let mut take = |name: &str| npz.remove(name);
        let window = take("window").context("frontend npz has no window")?.data;
        let filterbank = take("filterbank").context("frontend npz has no filterbank")?;
        let dct = take("dct").context("frontend npz has no dct")?;
        let c0_weights = take("c0_weights").map(|a| a.data);
        let post = take("post");
        ensure!(filterbank.shape.len() == 2, "filterbank must be 2-D");
        ensure!(
            dct.shape.len() == 2 && dct.shape[1] == filterbank.shape[0],
            "dct is {:?}, filterbank has {} bands",
            dct.shape,
            filterbank.shape[0]
        );
        if let Some(post) = &post {
            ensure!(
                post.shape.len() == 2 && post.shape[1] == dct.shape[0],
                "post is {:?}, dct has {} coefficients",
                post.shape,
                dct.shape[0]
            );
        }
        Ok(Self {
            window,
            filterbank,
            dct,
            c0_weights,
            post,
        })
    }

    pub fn feature_dim(&self) -> usize {
        match &self.post {
            Some(post) => post.shape[0],
            None => self.dct.shape[0],
        }
    }
}

/// 加窗、`|rfft|²`、滤波器组、log、DCT：一帧 `n_fft` 个样本 → `dct` 行数个系数。
pub struct LogFeatures {
    pub n_fft: usize,
    pub hop: usize,
    log_offset: f64,
    window: Vec<f64>,
    filterbank: Array,
    dct: Array,
    fft: Arc<dyn RealToComplex<f64>>,
    frame: Vec<f64>,
    spectrum: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    power: Vec<f64>,
    bands: Vec<f64>,
}

impl LogFeatures {
    pub fn new(n_fft: usize, hop: usize, log_offset: f64, arrays: &FrontendArrays) -> Result<Self> {
        ensure!(n_fft > 0 && hop > 0, "n_fft and hop must be positive");
        ensure!(
            arrays.window.len() == n_fft,
            "window has {} taps, n_fft is {n_fft}",
            arrays.window.len()
        );
        ensure!(
            arrays.filterbank.shape[1] == n_fft / 2 + 1,
            "filterbank has {} bins, n_fft {n_fft} gives {}",
            arrays.filterbank.shape[1],
            n_fft / 2 + 1
        );
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(n_fft);
        Ok(Self {
            n_fft,
            hop,
            log_offset,
            window: arrays.window.clone(),
            filterbank: arrays.filterbank.clone(),
            dct: arrays.dct.clone(),
            frame: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            power: vec![0.0; n_fft / 2 + 1],
            bands: vec![0.0; arrays.filterbank.shape[0]],
            fft,
        })
    }

    pub fn dim(&self) -> usize {
        self.dct.shape[0]
    }

    /// 能凑出几整帧：`1 + (len - n_fft) / hop`。
    pub fn frame_count(&self, len: usize) -> usize {
        if len < self.n_fft {
            0
        } else {
            1 + (len - self.n_fft) / self.hop
        }
    }

    /// `samples` 从 `start` 起的一帧，系数追加到 `out`。
    pub fn frame_into(&mut self, samples: &[f64], start: usize, out: &mut Vec<f64>) {
        let src = &samples[start..start + self.n_fft];
        for ((dst, s), w) in self.frame.iter_mut().zip(src).zip(&self.window) {
            *dst = s * w;
        }
        self.fft
            .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
            .expect("buffer lengths come from the plan");
        for (p, c) in self.power.iter_mut().zip(&self.spectrum) {
            *p = c.norm_sqr();
        }
        for (b, band) in self.bands.iter_mut().enumerate() {
            let weights = self.filterbank.row(b);
            let energy: f64 = weights.iter().zip(&self.power).map(|(w, p)| w * p).sum();
            *band = (energy + self.log_offset).ln();
        }
        for c in 0..self.dct.shape[0] {
            let row = self.dct.row(c);
            out.push(row.iter().zip(&self.bands).map(|(d, b)| d * b).sum());
        }
    }

    /// 整段 `samples` 的每一帧，(n, dim) 按行摊平。
    pub fn all(&mut self, samples: &[f64]) -> Vec<f64> {
        let n = self.frame_count(samples.len());
        let mut out = Vec::with_capacity(n * self.dim());
        for t in 0..n {
            self.frame_into(samples, t * self.hop, &mut out);
        }
        out
    }
}

/// 一级的流式前端：上面那串之后，`c0` 减它自己的因果滑动均值，再乘 `post`。
pub struct StreamingFrontend {
    log: LogFeatures,
    c0_weights: Option<Vec<f64>>,
    c0_denominator: Vec<f64>,
    /// 最近 `taps` 个原始 c0，新的在前。
    c0_history: Vec<f64>,
    c0_seen: usize,
    post: Option<Array>,
    pending: Vec<f64>,
    coeffs: Vec<f64>,
    /// 开机以来出过的帧数。
    pub frames: usize,
}

impl StreamingFrontend {
    pub fn new(n_fft: usize, hop: usize, log_offset: f64, arrays: &FrontendArrays) -> Result<Self> {
        let c0_denominator = arrays
            .c0_weights
            .as_ref()
            .map(|w| {
                w.iter()
                    .scan(0.0, |acc, v| {
                        *acc += v;
                        Some(*acc)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            log: LogFeatures::new(n_fft, hop, log_offset, arrays)?,
            c0_history: vec![0.0; arrays.c0_weights.as_ref().map_or(0, Vec::len)],
            c0_weights: arrays.c0_weights.clone(),
            c0_denominator,
            c0_seen: 0,
            post: arrays.post.clone(),
            pending: Vec::new(),
            coeffs: Vec::new(),
            frames: 0,
        })
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.frames = 0;
        self.c0_history.fill(0.0);
        self.c0_seen = 0;
    }

    pub fn feature_dim(&self) -> usize {
        match &self.post {
            Some(post) => post.shape[0],
            None => self.log.dim(),
        }
    }

    /// [-1, 1] 的样本 → 它们凑齐的每一帧，(n, dim) 按行摊平。
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        self.pending.extend(samples.iter().map(|&s| s as f64));
        let n = self.log.frame_count(self.pending.len());
        let dim = self.feature_dim();
        let mut out = Vec::with_capacity(n * dim);
        let hop = self.log.hop;
        for t in 0..n {
            self.coeffs.clear();
            self.log
                .frame_into(&self.pending, t * hop, &mut self.coeffs);
            if let Some(weights) = &self.c0_weights {
                let value = self.coeffs[0];
                let history = &mut self.c0_history;
                history.rotate_right(1);
                history[0] = value;
                let taps = self.c0_seen.min(history.len() - 1);
                self.c0_seen += 1;
                let dot: f64 = weights.iter().zip(history.iter()).map(|(w, h)| w * h).sum();
                self.coeffs[0] = value - dot / self.c0_denominator[taps];
            }
            match &self.post {
                Some(post) => {
                    for f in 0..post.shape[0] {
                        let row = post.row(f);
                        let v: f64 = row.iter().zip(&self.coeffs).map(|(p, c)| p * c).sum();
                        out.push(v as f32);
                    }
                }
                None => out.extend(self.coeffs.iter().map(|&v| v as f32)),
            }
        }
        if n > 0 {
            self.pending.drain(..n * hop);
        }
        self.frames += n;
        out
    }
}

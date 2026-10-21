//! 语谱图：自己的 STFT，列与一级的帧在同一条时间轴上（开机以来的样本）。
//! 线性 bin 按对数频率行重采样后存下，着色时只取看得见的那一段。

use crate::axis::LogAxis;
use crate::dsp::{Stft, DB_FLOOR};
use eframe::egui::{Color32, ColorImage};

/// 32 ms 窗、10 ms 一列：看得清音节，和 16 kHz 一级的帧一样密。
pub const WINDOW_S: f64 = 0.032;
pub const HOP_S: f64 = 0.010;
pub const ROWS: usize = 192;
pub const F_MIN: f32 = 60.0;

#[derive(Clone, Copy, Debug)]
enum Tap {
    Lerp(usize, f32),
    Max(usize, usize),
}

/// 线性 bin → 对数频率行。行 0 在顶部（`f_max`）。
pub struct RowMap {
    taps: Vec<Tap>,
}

impl RowMap {
    pub fn new(axis: LogAxis, rows: usize, bins: usize, bin_hz: f32) -> Self {
        let last = bins - 1;
        let taps = (0..rows)
            .map(|r| {
                let t_hi = 1.0 - r as f32 / rows as f32;
                let t_lo = 1.0 - (r + 1) as f32 / rows as f32;
                let lo = (axis.freq_at(t_lo) / bin_hz).ceil() as usize;
                let hi = ((axis.freq_at(t_hi) / bin_hz).floor() as usize).min(last);
                if lo <= hi {
                    // 一行盖住至少一个 bin（高频）：取最大，窄峰不会被抽稀掉
                    Tap::Max(lo, hi)
                } else {
                    // 一行比一个 bin 还窄（低频）：相邻 bin 间线性插值
                    let x = (axis.freq_at(0.5 * (t_lo + t_hi)) / bin_hz).clamp(0.0, last as f32);
                    let i = (x as usize).min(last - 1);
                    Tap::Lerp(i, x - i as f32)
                }
            })
            .collect();
        Self { taps }
    }

    pub fn apply(&self, spectrum: &[f32], out: &mut [f32]) {
        for (o, tap) in out.iter_mut().zip(&self.taps) {
            *o = match *tap {
                Tap::Lerp(i, frac) => spectrum[i] + (spectrum[i + 1] - spectrum[i]) * frac,
                Tap::Max(lo, hi) => spectrum[lo..=hi]
                    .iter()
                    .copied()
                    .fold(f32::NEG_INFINITY, f32::max),
            };
        }
    }
}

pub struct Spectrogram {
    pub sample_rate: u32,
    pub size: usize,
    pub hop: usize,
    pub axis: LogAxis,
    stft: Stft,
    map: RowMap,
    /// 每列 `ROWS` 个 dB，行 0 在顶部。
    cols: Vec<f32>,
    /// `cols` 里第一列是开机以来第几列。
    first: usize,
}

impl Spectrogram {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f64;
        let size = ((WINDOW_S * sr).round() as usize).next_power_of_two();
        let hop = ((HOP_S * sr).round() as usize).max(1);
        let stft = Stft::new(size, hop);
        let axis = LogAxis {
            f_min: F_MIN,
            f_max: sample_rate as f32 / 2.0,
        };
        let map = RowMap::new(axis, ROWS, stft.bins(), sample_rate as f32 / size as f32);
        Self {
            sample_rate,
            size,
            hop,
            axis,
            stft,
            map,
            cols: vec![],
            first: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let (map, cols) = (&self.map, &mut self.cols);
        self.stft.process(samples, |spectrum, _| {
            let at = cols.len();
            cols.resize(at + ROWS, DB_FLOOR);
            map.apply(spectrum, &mut cols[at..]);
        });
    }

    /// 开机以来出过的列数。
    pub fn end(&self) -> usize {
        self.first + self.cols.len() / ROWS
    }

    /// 第 `i` 列中心的秒数。
    pub fn time_of(&self, i: f64) -> f64 {
        (i * self.hop as f64 + self.size as f64 / 2.0) / self.sample_rate as f64
    }

    /// 中心在 `t` 秒的列（小数）。
    pub fn column_at(&self, t: f64) -> f64 {
        (t * self.sample_rate as f64 - self.size as f64 / 2.0) / self.hop as f64
    }

    pub fn column(&self, i: usize) -> Option<&[f32]> {
        let j = i.checked_sub(self.first)?;
        self.cols.get(j * ROWS..(j + 1) * ROWS)
    }

    /// 只留最近 `keep` 列。
    pub fn trim(&mut self, keep: usize) {
        let have = self.cols.len() / ROWS;
        // 攒够四分之一再挪，免得每帧都搬整块
        if have > keep + keep / 4 {
            let drop = have - keep;
            self.cols.drain(..drop * ROWS);
            self.first += drop;
        }
    }

    /// 列 `[lo, hi)` 着色成至多 `width` 列宽的图；列多时等距抽取。没出过的列是底色。
    pub fn render(
        &self,
        lut: &[Color32; 256],
        db_lo: f32,
        db_hi: f32,
        lo: usize,
        hi: usize,
        width: usize,
    ) -> ColorImage {
        let n = hi.saturating_sub(lo).max(1);
        let w = n.min(width.max(1));
        let mut pixels = vec![lut[0]; w * ROWS];
        let scale = 255.0 / (db_hi - db_lo);
        for x in 0..w {
            let i = lo + x * n / w;
            if let Some(col) = self.column(i) {
                for (y, &db) in col.iter().enumerate() {
                    pixels[y * w + x] = lut[((db - db_lo) * scale).clamp(0.0, 255.0) as usize];
                }
            }
        }
        ColorImage {
            size: [w, ROWS],
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spike_lands_on_the_row_holding_its_frequency() {
        let axis = LogAxis {
            f_min: 50.0,
            f_max: 8_000.0,
        };
        let (rows, bins, bin_hz) = (600, 2049, 48_000.0 / 4096.0);
        let map = RowMap::new(axis, rows, bins, bin_hz);
        // 低频走插值、高频走取最大，两种分支各取一个
        for k in [6, 85, 600] {
            let mut spectrum = vec![-100.0; bins];
            spectrum[k] = 0.0;
            let mut out = vec![0.0; rows];
            map.apply(&spectrum, &mut out);
            let r = (0..rows)
                .max_by(|&a, &b| out[a].total_cmp(&out[b]))
                .unwrap();
            let edge = |i: usize| axis.freq_at(1.0 - i as f32 / rows as f32);
            let (f_lo, f_hi) = (edge(r + 1), edge(r));
            let f = k as f32 * bin_hz;
            assert!(
                f_lo - bin_hz <= f && f <= f_hi + bin_hz,
                "bin {k} ({f} Hz) mapped to row {r} [{f_lo}, {f_hi}]"
            );
        }
    }

    #[test]
    fn tone_lands_on_its_row_at_its_time() {
        let sr = 16_000;
        let mut spec = Spectrogram::new(sr);
        // 一秒静音后一秒 1 kHz
        let mut x = vec![0.0f32; sr as usize];
        x.extend(
            (0..sr)
                .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin() * 0.5),
        );
        for c in x.chunks(777) {
            spec.push(c);
        }
        let loud = |i: usize| {
            spec.column(i)
                .unwrap()
                .iter()
                .copied()
                .fold(f32::MIN, f32::max)
        };
        let at = |t: f64| spec.column_at(t).round() as usize;
        assert!(loud(at(0.5)) < -100.0);
        assert!(loud(at(1.5)) > -10.0);
        let col = spec.column(at(1.5)).unwrap();
        let row = (0..ROWS)
            .max_by(|&a, &b| col[a].total_cmp(&col[b]))
            .unwrap();
        let f = spec.axis.freq_at(1.0 - (row as f32 + 0.5) / ROWS as f32);
        assert!((f / 1000.0).log2().abs() < 0.05, "{f}");

        let end = spec.end();
        spec.trim(10);
        assert_eq!(spec.end(), end);
        assert!(spec.column(end - 10).is_some() && spec.column(end - 14).is_none());
    }
}

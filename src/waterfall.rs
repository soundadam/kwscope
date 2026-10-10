//! 滚动瀑布图：保留原始线性 bin 谱，另存按对数频率行重采样后的副本用于着色。
//! 改频率范围时从原始谱重映射，暂停后也能缩放已有历史。

use crate::axis::LogAxis;
use crate::dsp::DB_FLOOR;
use eframe::egui::{Color32, ColorImage};

#[derive(Clone, Copy, Debug)]
enum Tap {
    Lerp(usize, f32),
    Max(usize, usize),
}

/// 线性 bin → 对数频率行。行 0 在顶部（`f_max`）。
pub struct RowMap {
    taps: Vec<Tap>,
    bins: usize,
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
        Self { taps, bins }
    }

    pub fn rows(&self) -> usize {
        self.taps.len()
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

pub struct Waterfall {
    cols: usize,
    map: RowMap,
    raw: Vec<f32>,
    mapped: Vec<f32>,
    head: usize,
    pushed: usize,
}

impl Waterfall {
    pub fn new(cols: usize, map: RowMap) -> Self {
        Self {
            cols,
            raw: vec![DB_FLOOR; cols * map.bins],
            mapped: vec![DB_FLOOR; cols * map.rows()],
            map,
            head: 0,
            pushed: 0,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.map.rows()
    }

    pub fn push(&mut self, spectrum: &[f32]) {
        let (b, r) = (self.map.bins, self.map.rows());
        let c = self.head;
        self.raw[c * b..(c + 1) * b].copy_from_slice(spectrum);
        self.map
            .apply(spectrum, &mut self.mapped[c * r..(c + 1) * r]);
        self.head = (self.head + 1) % self.cols;
        self.pushed += 1;
    }

    /// 换频率轴：bin 数不变时保留历史并重映射。
    pub fn set_map(&mut self, map: RowMap) {
        assert_eq!(
            map.bins, self.map.bins,
            "bin count changed; rebuild instead"
        );
        self.map = map;
        let (b, r) = (self.map.bins, self.map.rows());
        self.mapped.resize(self.cols * r, DB_FLOOR);
        for c in 0..self.cols {
            self.map.apply(
                &self.raw[c * b..(c + 1) * b],
                &mut self.mapped[c * r..(c + 1) * r],
            );
        }
    }

    /// `x = 0` 最老，`x = cols - 1` 最新。
    pub fn mapped_column(&self, x: usize) -> &[f32] {
        let c = (self.head + x) % self.cols;
        let r = self.map.rows();
        &self.mapped[c * r..(c + 1) * r]
    }

    pub fn latest_raw(&self) -> Option<&[f32]> {
        (self.pushed > 0).then(|| {
            let c = (self.head + self.cols - 1) % self.cols;
            let b = self.map.bins;
            &self.raw[c * b..(c + 1) * b]
        })
    }

    pub fn render(&self, lut: &[Color32; 256], db_lo: f32, db_hi: f32) -> ColorImage {
        let (w, h) = (self.cols, self.map.rows());
        let mut pixels = vec![Color32::BLACK; w * h];
        let scale = 255.0 / (db_hi - db_lo);
        for x in 0..w {
            for (y, &db) in self.mapped_column(x).iter().enumerate() {
                pixels[y * w + x] = lut[((db - db_lo) * scale).clamp(0.0, 255.0) as usize];
            }
        }
        ColorImage {
            size: [w, h],
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
}

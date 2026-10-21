//! 语谱图：自己的 STFT，列与一级的帧在同一条时间轴上（开机以来的样本）。
//!
//! 每列同时算三个窗长，中心对齐：中窗（32 ms）整张存下，长窗（约 96 ms）只存 500 Hz 往下、
//! 短窗（16 ms）只存 2 kHz 往上（各带交叉渐变用的余量）。多分辨率时低频读长窗、中频读中窗、
//! 高频读短窗，交界处在 dB 上交叉渐变；单分辨率只读中窗。存的是 bin，着色时才按当前的轴与行数
//! 映射到行，所以换轴、换分辨率、改窗口高度都不用重算。

use crate::axis::{FreqAxis, Scale};
use crate::dsp::{Stft, DB_FLOOR};
use eframe::egui::{Color32, ColorImage};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// 中窗 32 ms、10 ms 一列：看得清音节，和 16 kHz 一级的帧一样密。
pub const WINDOW_S: f64 = 0.032;
pub const HOP_S: f64 = 0.010;
/// 行数跟着屏幕像素走，但不超过这么多。
pub const MAX_ROWS: usize = 1024;
/// 长窗管到哪、短窗从哪管起。
pub const LOW_EDGE_HZ: f32 = 500.0;
pub const HIGH_EDGE_HZ: f32 = 2_000.0;
/// 交界处交叉渐变的宽度（八度）。
pub const CROSSFADE_OCT: f32 = 1.0 / 3.0;
/// 存成 i16 的 dB × 这个数。
const DB_SCALE: f32 = 100.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolution {
    /// 长 / 中 / 短三个窗分管低 / 中 / 高频。
    Multi,
    /// 只用 32 ms 的中窗。
    Single,
}

impl Resolution {
    pub const ALL: [Self; 2] = [Self::Multi, Self::Single];

    pub fn name(self) -> &'static str {
        match self {
            Self::Multi => "Multi",
            Self::Single => "Single 32 ms",
        }
    }
}

/// 画成什么样：轴、行数、分辨率。变了就重建 `RowMap`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub scale: Scale,
    pub rows: usize,
    pub resolution: Resolution,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            scale: Scale::Mel,
            rows: 256,
            resolution: Resolution::Multi,
        }
    }
}

/// 频率 `f` 处 [长, 中, 短] 三个窗各占多少：各自的带内为 1，交界两侧各 1/6 八度里平滑地交接，和为 1。
pub fn band_weights(f: f32) -> [f32; 3] {
    let fade = |edge: f32| {
        // 0 在交界下沿，1 在上沿
        let x = ((f.max(1e-3) / edge).log2() / CROSSFADE_OCT + 0.5).clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    let long = 1.0 - fade(LOW_EDGE_HZ);
    let short = fade(HIGH_EDGE_HZ);
    [long, 1.0 - long - short, short]
}

/// 一个窗长的谱在一列里存了哪一段：bin `[lo, hi]`，放在列里 `offset` 起。
#[derive(Clone, Copy, Debug)]
struct Band {
    bin_hz: f32,
    lo: usize,
    hi: usize,
    offset: usize,
}

impl Band {
    fn len(&self) -> usize {
        self.hi - self.lo + 1
    }

    /// 频率 `[f_lo, f_hi]` 这一行从这段 bin 里怎么取。
    fn tap(&self, f_lo: f32, f_hi: f32) -> Tap {
        let lo = ((f_lo / self.bin_hz).ceil().max(0.0) as usize).max(self.lo);
        let hi = ((f_hi / self.bin_hz).floor().max(0.0) as usize).min(self.hi);
        if lo <= hi {
            // 一行盖住至少一个 bin（高频）：取最大，窄峰不会被抽稀掉
            Tap::Max(self.offset + lo - self.lo, self.offset + hi - self.lo)
        } else {
            // 一行比一个 bin 还窄（低频）：相邻 bin 间线性插值
            let x = (0.5 * (f_lo + f_hi) / self.bin_hz).clamp(self.lo as f32, self.hi as f32);
            let i = (x as usize).min(self.hi - 1);
            Tap::Lerp(self.offset + i - self.lo, x - i as f32)
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Tap {
    Lerp(usize, f32),
    Max(usize, usize),
}

impl Tap {
    fn read(self, col: &[i16]) -> f32 {
        match self {
            Tap::Lerp(i, frac) => {
                let (a, b) = (col[i] as f32, col[i + 1] as f32);
                (a + (b - a) * frac) / DB_SCALE
            }
            Tap::Max(lo, hi) => {
                col[lo..=hi].iter().copied().max().unwrap_or(i16::MIN) as f32 / DB_SCALE
            }
        }
    }
}

/// 存下的 bin → 频率行。行 0 在顶部（`f_max`）。每行至多从两个窗长取，按权重在 dB 上混。
pub struct RowMap {
    rows: Vec<[(Tap, f32); 2]>,
}

impl RowMap {
    /// `bands` 是 [长, 中, 短]；单分辨率时只用中窗。
    fn new(axis: FreqAxis, rows: usize, bands: &[Band; 3], resolution: Resolution) -> Self {
        let rows = (0..rows)
            .map(|r| {
                let t_hi = 1.0 - r as f32 / rows as f32;
                let t_lo = 1.0 - (r + 1) as f32 / rows as f32;
                let (f_lo, f_hi) = (axis.freq_at(t_lo), axis.freq_at(t_hi));
                let weights = match resolution {
                    Resolution::Multi => band_weights(axis.freq_at(0.5 * (t_lo + t_hi))),
                    Resolution::Single => [0.0, 1.0, 0.0],
                };
                let mut taps = [(Tap::Lerp(0, 0.0), 0.0); 2];
                let mut n = 0;
                for (band, w) in bands.iter().zip(weights) {
                    // 渐变带外两个窗不会同时有份
                    if w > 0.0 && n < 2 {
                        taps[n] = (band.tap(f_lo, f_hi), w);
                        n += 1;
                    }
                }
                taps
            })
            .collect();
        Self { rows }
    }

    fn rows(&self) -> usize {
        self.rows.len()
    }

    fn apply(&self, col: &[i16], out: &mut [f32]) {
        for (o, taps) in out.iter_mut().zip(&self.rows) {
            let [(a, wa), (b, wb)] = *taps;
            *o = if wb == 0.0 {
                a.read(col)
            } else {
                a.read(col) * wa + b.read(col) * wb
            };
        }
    }
}

pub struct Spectrogram {
    /// 每个语谱图一个，纹理认它判断是不是换了一份。
    id: u64,
    pub sample_rate: u32,
    /// 中窗的长度：列的中心在 `i·hop + size/2`。
    pub size: usize,
    pub hop: usize,
    stft: Stft,
    /// [长, 中, 短] 各自存在列里的哪一段。
    bands: [Band; 3],
    /// 一列存多少个数。
    stride: usize,
    layout: Layout,
    map: RowMap,
    /// 每列 `stride` 个 dB × `DB_SCALE`。
    cols: Vec<i16>,
    /// `cols` 里第一列是开机以来第几列。
    first: usize,
}

impl Spectrogram {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f64;
        let size = ((WINDOW_S * sr).round() as usize)
            .next_power_of_two()
            .max(8);
        let hop = ((HOP_S * sr).round() as usize).max(1);
        // 三个窗长：3·size（16 kHz 下 96 ms）、size、size/2（16 ms）
        let sizes = [size, 3 * size, size / 2];
        let stft = Stft::centred(&sizes, hop);
        let bin_hz = |s: usize| sample_rate as f32 / s as f32;
        let half = 2f32.powf(CROSSFADE_OCT / 2.0);
        // 渐变带外再多存两个 bin，插值与取最大都不会读出界
        let long_hz = bin_hz(3 * size);
        let long_hi = ((LOW_EDGE_HZ * half / long_hz).ceil() as usize + 2).min(stft.bins(1) - 1);
        let short_hz = bin_hz(size / 2);
        let short_lo = ((HIGH_EDGE_HZ / half / short_hz).floor() as usize)
            .saturating_sub(2)
            .min(stft.bins(2) - 2);
        let mid = Band {
            bin_hz: bin_hz(size),
            lo: 0,
            hi: stft.bins(0) - 1,
            offset: 0,
        };
        let long = Band {
            bin_hz: long_hz,
            lo: 0,
            hi: long_hi,
            offset: mid.len(),
        };
        let short = Band {
            bin_hz: short_hz,
            lo: short_lo,
            hi: stft.bins(2) - 1,
            offset: mid.len() + long.len(),
        };
        let bands = [long, mid, short];
        let layout = Layout::default();
        let axis = FreqAxis::new(layout.scale, sample_rate as f32 / 2.0);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self {
            id: NEXT.fetch_add(1, Relaxed),
            sample_rate,
            size,
            hop,
            stft,
            stride: mid.len() + long.len() + short.len(),
            map: RowMap::new(axis, layout.rows, &bands, layout.resolution),
            bands,
            layout,
            cols: vec![],
            first: 0,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn axis(&self) -> FreqAxis {
        FreqAxis::new(self.layout.scale, self.sample_rate as f32 / 2.0)
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// 换轴、行数或分辨率：只重建映射，存下的列不动。
    pub fn set_layout(&mut self, layout: Layout) {
        let layout = Layout {
            rows: layout.rows.clamp(1, MAX_ROWS),
            ..layout
        };
        if layout != self.layout {
            self.layout = layout;
            self.map = RowMap::new(self.axis(), layout.rows, &self.bands, layout.resolution);
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        let (bands, stride, cols) = (&self.bands, self.stride, &mut self.cols);
        self.stft.process(samples, |spectra| {
            let at = cols.len();
            cols.resize(at + stride, (DB_FLOOR * DB_SCALE) as i16);
            let col = &mut cols[at..];
            // `spectra` 按 [中, 长, 短] 出
            for (band, spectrum) in [&bands[1], &bands[0], &bands[2]].into_iter().zip(spectra) {
                let dst = &mut col[band.offset..band.offset + band.len()];
                for (d, &db) in dst.iter_mut().zip(&spectrum[band.lo..=band.hi]) {
                    *d = (db * DB_SCALE)
                        .round()
                        .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
                }
            }
        });
    }

    /// 开机以来出过的列数。
    #[cfg(test)]
    pub fn end(&self) -> usize {
        self.first + self.cols.len() / self.stride
    }

    /// 第 `i` 列中心的秒数。
    pub fn time_of(&self, i: f64) -> f64 {
        (i * self.hop as f64 + self.size as f64 / 2.0) / self.sample_rate as f64
    }

    /// 中心在 `t` 秒的列（小数）。
    pub fn column_at(&self, t: f64) -> f64 {
        (t * self.sample_rate as f64 - self.size as f64 / 2.0) / self.hop as f64
    }

    fn column(&self, i: usize) -> Option<&[i16]> {
        let j = i.checked_sub(self.first)?;
        self.cols.get(j * self.stride..(j + 1) * self.stride)
    }

    /// 第 `i` 列按当前布局映射成的行（行 0 在顶部）。
    #[cfg(test)]
    pub fn rows_of(&self, i: usize) -> Option<Vec<f32>> {
        let col = self.column(i)?;
        let mut out = vec![DB_FLOOR; self.map.rows()];
        self.map.apply(col, &mut out);
        Some(out)
    }

    /// 只留最近 `keep` 列。
    pub fn trim(&mut self, keep: usize) {
        let have = self.cols.len() / self.stride;
        // 攒够四分之一再挪，免得每帧都搬整块
        if have > keep + keep / 4 {
            let drop = have - keep;
            self.cols.drain(..drop * self.stride);
            self.first += drop;
        }
    }

    pub fn has_column(&self, i: usize) -> bool {
        self.column(i).is_some()
    }

    /// 把 `cols` 这几列着色成一幅图，宽是列数、高是当前行数。没出过的列是底色。
    pub fn render_columns(
        &self,
        lut: &[Color32; 256],
        db_lo: f32,
        db_hi: f32,
        cols: impl ExactSizeIterator<Item = usize>,
    ) -> ColorImage {
        let (w, rows) = (cols.len(), self.map.rows());
        let mut pixels = vec![lut[0]; w * rows];
        let mut mapped = vec![DB_FLOOR; rows];
        let scale = 255.0 / (db_hi - db_lo);
        for (x, i) in cols.enumerate() {
            if let Some(col) = self.column(i) {
                self.map.apply(col, &mut mapped);
                for (y, &db) in mapped.iter().enumerate() {
                    pixels[y * w + x] = lut[((db - db_lo) * scale).clamp(0.0, 255.0) as usize];
                }
            }
        }
        ColorImage {
            size: [w, rows],
            pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const SR: u32 = 16_000;

    fn tone(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(secs * SR as f32) as usize)
            .map(|i| amp * (2.0 * PI * freq * i as f32 / SR as f32).sin())
            .collect()
    }

    fn feed(spec: &mut Spectrogram, x: &[f32]) {
        for c in x.chunks(777) {
            spec.push(c);
        }
    }

    /// 第 `i` 列最响的行和它的频率（行中心）。
    fn peak(spec: &Spectrogram, i: usize) -> (usize, f32, f32) {
        let col = spec.rows_of(i).unwrap();
        let rows = col.len();
        let r = (0..rows)
            .max_by(|&a, &b| col[a].total_cmp(&col[b]))
            .unwrap();
        let f = spec.axis().freq_at(1.0 - (r as f32 + 0.5) / rows as f32);
        (r, f, col[r])
    }

    #[test]
    fn bands_own_their_ranges_and_fade_smoothly() {
        assert_eq!(band_weights(100.0), [1.0, 0.0, 0.0]);
        assert_eq!(band_weights(1_000.0), [0.0, 1.0, 0.0]);
        assert_eq!(band_weights(6_000.0), [0.0, 0.0, 1.0]);
        // 渐变带只有 1/3 八度宽
        let sixth = 2f32.powf(1.0 / 6.0) * 1.001;
        assert_eq!(band_weights(LOW_EDGE_HZ / sixth), [1.0, 0.0, 0.0]);
        assert_eq!(band_weights(LOW_EDGE_HZ * sixth), [0.0, 1.0, 0.0]);
        assert_eq!(band_weights(HIGH_EDGE_HZ / sixth), [0.0, 1.0, 0.0]);
        assert_eq!(band_weights(HIGH_EDGE_HZ * sixth), [0.0, 0.0, 1.0]);
        for (edge, a, b) in [(LOW_EDGE_HZ, 0, 1), (HIGH_EDGE_HZ, 1, 2)] {
            let w = band_weights(edge);
            assert!((w[a] - 0.5).abs() < 1e-5 && (w[b] - 0.5).abs() < 1e-5);
        }
        let mut last = band_weights(20.0);
        for i in 1..=400 {
            let f = 20.0 * 2f32.powf(i as f32 / 50.0);
            let w = band_weights(f);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            assert!(w.iter().filter(|&&v| v > 0.0).count() <= 2);
            // 长窗一路往下交，短窗一路往上接，每步都小：没有台阶
            assert!(w[0] <= last[0] && w[2] >= last[2]);
            assert!((w[1] - last[1]).abs() < 0.15, "{f} Hz");
            last = w;
        }
    }

    #[test]
    fn tones_peak_on_their_rows_in_multi_mode() {
        for scale in [Scale::Mel, Scale::Log] {
            for f0 in [200.0, 4_000.0] {
                let mut spec = Spectrogram::new(SR);
                spec.set_layout(Layout {
                    scale,
                    rows: 600,
                    resolution: Resolution::Multi,
                });
                feed(&mut spec, &tone(f0, 0.5, 1.0));
                let (_, f, db) = peak(&spec, spec.end() / 2);
                assert!((f / f0).log2().abs() < 0.02, "{scale:?} {f0}: {f}");
                assert!((db + 6.02).abs() < 1.5, "{scale:?} {f0}: {db}");
            }
        }
    }

    #[test]
    fn long_window_resolves_low_frequencies_better() {
        // 200 Hz：长窗 bin 10.4 Hz，中窗 31 Hz；峰两侧 40 Hz 处多分辨率应该暗得多
        let width = |resolution| {
            let mut spec = Spectrogram::new(SR);
            spec.set_layout(Layout {
                scale: Scale::Mel,
                rows: 1024,
                resolution,
            });
            feed(&mut spec, &tone(200.0, 0.5, 1.0));
            let col = spec.rows_of(spec.end() / 2).unwrap();
            let axis = spec.axis();
            let row = |f: f32| ((1.0 - axis.unit_of(f)) * 1024.0) as usize;
            col[row(160.0)].max(col[row(240.0)])
        };
        assert!(width(Resolution::Multi) < width(Resolution::Single) - 20.0);
    }

    #[test]
    fn a_sine_reads_the_same_dbfs_in_every_band() {
        // 每个频率都落在自己那个窗的 bin 中心上：读数应该都是 20·log10(0.5)
        let amp: f32 = 0.5;
        let expect = 20.0 * amp.log10();
        let spec = Spectrogram::new(SR);
        for (k, bins) in [(0, 20.0), (1, 32.0), (2, 64.0)] {
            let band = spec.bands[k];
            let f = bins * band.bin_hz;
            let mut spec = Spectrogram::new(SR);
            feed(&mut spec, &tone(f, amp, 1.0));
            let col = spec.column(spec.end() / 2).unwrap();
            let raw = col[band.offset + bins as usize - band.lo] as f32 / DB_SCALE;
            assert!((raw - expect).abs() < 0.05, "band {k} at {f} Hz: {raw}");
            // 映射到很密的行上，峰值行读数也差不多
            spec.set_layout(Layout {
                scale: Scale::Log,
                rows: MAX_ROWS,
                resolution: Resolution::Multi,
            });
            let (_, _, db) = peak(&spec, spec.end() / 2);
            assert!((db - expect).abs() < 0.5, "band {k} at {f} Hz: {db}");
        }
    }

    #[test]
    fn crossfade_has_no_seam_on_a_sweep_of_tones() {
        // 同幅度的一串正弦扫过两个交界：峰值读数不该在交界处跳
        let mut last: Option<f32> = None;
        for i in 0..60 {
            let f = 300.0 * 2f32.powf(i as f32 / 20.0);
            let mut spec = Spectrogram::new(SR);
            spec.set_layout(Layout {
                scale: Scale::Mel,
                rows: MAX_ROWS,
                resolution: Resolution::Multi,
            });
            feed(&mut spec, &tone(f, 0.5, 0.5));
            let (_, _, db) = peak(&spec, spec.end() / 2);
            assert!((db + 6.02).abs() < 1.6, "{f} Hz: {db}");
            if let Some(prev) = last {
                assert!((db - prev).abs() < 1.6, "{f} Hz: {prev} → {db}");
            }
            last = Some(db);
        }
    }

    #[test]
    fn rows_follow_the_layout() {
        let mut spec = Spectrogram::new(SR);
        feed(&mut spec, &tone(1_000.0, 0.5, 0.5));
        let i = spec.end() / 2;
        for rows in [32, 300, 777, MAX_ROWS] {
            for scale in Scale::ALL {
                for resolution in Resolution::ALL {
                    spec.set_layout(Layout {
                        scale,
                        rows,
                        resolution,
                    });
                    assert_eq!(spec.rows_of(i).unwrap().len(), rows);
                    let (_, f, _) = peak(&spec, i);
                    let tol = 1.5 / rows as f32 * 8_000.0 + 20.0;
                    assert!(
                        (f - 1_000.0).abs() < tol,
                        "{rows} {scale:?} {resolution:?}: {f}"
                    );
                }
            }
        }
        spec.set_layout(Layout {
            rows: 5_000,
            ..spec.layout()
        });
        assert_eq!(spec.layout().rows, MAX_ROWS);
        let image = spec.render_columns(
            &[Color32::BLACK; 256],
            -100.0,
            0.0,
            [0, 3, 9, 10_000].into_iter(),
        );
        assert_eq!(image.size, [4, MAX_ROWS]);
    }

    #[test]
    fn tone_lands_on_its_row_at_its_time() {
        for resolution in Resolution::ALL {
            let mut spec = Spectrogram::new(SR);
            spec.set_layout(Layout {
                resolution,
                ..Layout::default()
            });
            // 列的时间和只有 32 ms 窗时一样：中心在 i·hop + size/2
            assert_eq!((spec.size, spec.hop), (512, 160));
            assert_eq!(spec.time_of(0.0), 256.0 / 16_000.0);
            assert_eq!(spec.time_of(100.0), (100.0 * 160.0 + 256.0) / 16_000.0);
            assert_eq!(spec.column_at(spec.time_of(37.0)), 37.0);
            // 一秒静音后一秒 1 kHz
            let mut x = vec![0.0f32; SR as usize];
            x.extend(tone(1_000.0, 0.5, 1.0));
            feed(&mut spec, &x);
            let loud = |i: usize| {
                spec.rows_of(i)
                    .unwrap()
                    .into_iter()
                    .fold(f32::MIN, f32::max)
            };
            let at = |t: f64| spec.column_at(t).round() as usize;
            assert!(loud(at(0.5)) < -100.0);
            assert!(loud(at(1.5)) > -10.0);
            // 起音落在中心刚过 1 s 的列上：中窗那列里已经有一半是正弦
            let onset = (0..spec.end()).find(|&i| loud(i) > -30.0).unwrap();
            assert!(
                (spec.time_of(onset as f64) - 1.0).abs() <= 0.012,
                "{resolution:?}"
            );
            let (_, f, _) = peak(&spec, at(1.5));
            assert!((f / 1000.0).log2().abs() < 0.05, "{f}");

            let end = spec.end();
            spec.trim(10);
            assert_eq!(spec.end(), end);
            assert!(spec.column(end - 10).is_some() && spec.column(end - 14).is_none());
        }
    }
}

//! 读 wav / flac，重采样到模型的采样率。
//!
//! 整数 PCM 按 soundfile 的 float32 读法归一（除以 2^(bits-1)），所以 16 kHz 的
//! 文件喂进去和 `kws stream` 读到的是同一串数。

use anyhow::{bail, ensure, Context, Result};
use std::path::Path;

pub struct Audio {
    pub sample_rate: u32,
    /// 每个声道一串样本。
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

pub fn decode(path: &Path) -> Result<Audio> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "wav" | "wave" => decode_wav(path),
        "flac" => decode_flac(path),
        _ => bail!("{}: open a .wav or .flac file", path.display()),
    }
    .with_context(|| path.display().to_string())
}

fn split(interleaved: impl Iterator<Item = f32>, channels: usize) -> Vec<Vec<f32>> {
    let mut out = vec![Vec::new(); channels];
    for (i, s) in interleaved.enumerate() {
        out[i % channels].push(s);
    }
    out
}

fn decode_wav(path: &Path) -> Result<Audio> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    ensure!(channels > 0, "no channels");
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => {
            let v: Vec<f32> = reader.samples::<f32>().collect::<Result<_, _>>()?;
            split(v.into_iter(), channels)
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            let v: Vec<i32> = reader.samples::<i32>().collect::<Result<_, _>>()?;
            split(v.into_iter().map(|s| s as f32 * scale), channels)
        }
    };
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels: samples,
    })
}

fn decode_flac(path: &Path) -> Result<Audio> {
    let mut reader = claxon::FlacReader::open(path)?;
    let info = reader.streaminfo();
    let channels = info.channels as usize;
    let scale = 1.0 / (1u64 << (info.bits_per_sample - 1)) as f32;
    let v: Vec<i32> = reader.samples().collect::<Result<_, _>>()?;
    Ok(Audio {
        sample_rate: info.sample_rate,
        channels: split(v.into_iter().map(|s| s as f32 * scale), channels),
    })
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// 零阶修正贝塞尔函数，Kaiser 窗用。
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..64 {
        term *= half / k as f64;
        sum += term * term;
        if term * term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// 多相加窗 sinc 的流式重采样。输出第 n 个样本正对输入时刻 `n * from / to`
/// （核居中，不带群延迟），所以重采样前后的时间轴是同一条。
pub struct Resampler {
    up: usize,
    down: usize,
    half: usize,
    /// `up` 个相位，每个 `2 * half` 个抽头，对输入 `i0 - half + 1 ..= i0 + half`。
    table: Vec<Vec<f32>>,
    /// 还要用到的输入；`buf[0]` 是开机以来第 `offset` 个输入样本。
    buf: Vec<f32>,
    offset: usize,
    /// 下一个要出的输出样本序号。
    next: usize,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let g = gcd(from as u64, to as u64).max(1);
        let up = (to as u64 / g) as usize;
        let down = (from as u64 / g) as usize;
        let ratio = (up as f64 / down as f64).min(1.0);
        // 截止留 5 % 过渡带；降采样时核按比例拉宽
        let cutoff = 0.5 * ratio * 0.95;
        let half = if up == down {
            0
        } else {
            (16.0 / ratio).ceil() as usize
        };
        let beta = 8.6;
        let norm = bessel_i0(beta);
        let table = (0..up)
            .map(|phase| {
                let frac = phase as f64 / up as f64;
                let taps: Vec<f64> = (0..2 * half)
                    .map(|j| {
                        // 抽头 j 对输入 i0 - half + 1 + j，离输出时刻 x 个输入样本
                        let x = j as f64 - half as f64 + 1.0 - frac;
                        let u = 2.0 * cutoff * x;
                        let sinc = if u.abs() < 1e-12 {
                            1.0
                        } else {
                            (std::f64::consts::PI * u).sin() / (std::f64::consts::PI * u)
                        };
                        let r = x / half as f64;
                        let w = if r.abs() >= 1.0 {
                            0.0
                        } else {
                            bessel_i0(beta * (1.0 - r * r).sqrt()) / norm
                        };
                        2.0 * cutoff * sinc * w
                    })
                    .collect();
                // 每个相位的直流增益归一成 1
                let sum: f64 = taps.iter().sum();
                taps.iter().map(|t| (t / sum) as f32).collect()
            })
            .collect();
        Self {
            up,
            down,
            half,
            table,
            buf: Vec::new(),
            offset: 0,
            next: 0,
        }
    }

    pub fn is_identity(&self) -> bool {
        self.up == self.down
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.offset = 0;
        self.next = 0;
    }

    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if self.is_identity() {
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let mut out = vec![];
        let have = self.offset + self.buf.len();
        loop {
            let num = self.next * self.down;
            let i0 = num / self.up;
            if i0 + self.half >= have {
                break;
            }
            let taps = &self.table[num % self.up];
            let mut acc = 0.0f32;
            for (j, t) in taps.iter().enumerate() {
                // 开机之前当零
                if let Some(i) = (i0 + 1 + j).checked_sub(self.half) {
                    acc += t * self.buf[i - self.offset];
                }
            }
            out.push(acc);
            self.next += 1;
        }
        // 留下下一个输出还要用的输入
        let i0 = self.next * self.down / self.up;
        let keep_from = (i0 + 1).saturating_sub(self.half).max(self.offset);
        self.buf.drain(..keep_from - self.offset);
        self.offset = keep_from;
        out
    }

    /// 流结束：补零把输入末尾之前该出的输出都出完。
    pub fn flush(&mut self, input_len: usize) -> Vec<f32> {
        if self.is_identity() {
            return vec![];
        }
        let want = (input_len as u64 * self.up as u64).div_ceil(self.down as u64) as usize;
        let mut out = self.push(&vec![0.0; self.half + 1]);
        out.truncate(want.saturating_sub(self.next - out.len()));
        out
    }
}

/// 整段重采样。
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    let mut r = Resampler::new(from, to);
    if r.is_identity() {
        return input.to_vec();
    }
    let mut out = r.push(input);
    out.extend(r.flush(input.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn resampled_tone_stays_on_the_time_axis() {
        for from in [48_000, 44_100, 8_000] {
            let x = tone(from, 1000.0, from as usize);
            let y = resample(&x, from, 16_000);
            assert_eq!(y.len(), 16_000, "{from}");
            let want = tone(16_000, 1000.0, 16_000);
            // 两头核伸到开机前与结束后，只看中间
            let err = y[200..15_800]
                .iter()
                .zip(&want[200..15_800])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(err < 2e-3, "{from}: {err}");
        }
    }

    #[test]
    fn chunked_equals_whole() {
        let x = tone(48_000, 440.0, 10_000);
        let whole = resample(&x, 48_000, 16_000);
        let mut r = Resampler::new(48_000, 16_000);
        let mut parts = vec![];
        for c in x.chunks(333) {
            parts.extend(r.push(c));
        }
        parts.extend(r.flush(x.len()));
        assert_eq!(parts, whole);
    }

    #[test]
    fn identity_passes_through() {
        let x = vec![0.1, -0.2, 0.3];
        assert_eq!(resample(&x, 16_000, 16_000), x);
    }
}

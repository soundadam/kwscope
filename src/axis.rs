/// 频率轴怎么拉伸。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    Mel,
    Log,
    Linear,
}

impl Scale {
    pub const ALL: [Self; 3] = [Self::Mel, Self::Log, Self::Linear];

    pub fn name(self) -> &'static str {
        match self {
            Self::Mel => "Mel",
            Self::Log => "Log",
            Self::Linear => "Linear",
        }
    }
}

/// 对数轴的下限；mel 与线性轴从 0 Hz 起。
pub const LOG_F_MIN: f32 = 60.0;
/// 刻度标签之间至少隔多少点。
const TICK_GAP: f32 = 22.0;
const MEL_TICKS: [f32; 17] = [
    100.0, 200.0, 300.0, 500.0, 700.0, 1e3, 1.5e3, 2e3, 3e3, 4e3, 5e3, 6e3, 8e3, 1e4, 1.2e4, 1.6e4,
    2e4,
];
const LOG_TICKS: [f32; 11] = [
    100.0, 200.0, 500.0, 1e3, 2e3, 3e3, 5e3, 8e3, 1.2e4, 1.6e4, 2e4,
];
const LINEAR_STEPS: [f32; 5] = [500.0, 1e3, 2e3, 4e3, 5e3];

/// 频率轴：`f_min` ↦ 0，`f_max` ↦ 1，中间按 `scale` 拉伸。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FreqAxis {
    pub scale: Scale,
    pub f_min: f32,
    pub f_max: f32,
}

impl FreqAxis {
    pub fn new(scale: Scale, f_max: f32) -> Self {
        let f_min = match scale {
            Scale::Log => LOG_F_MIN,
            Scale::Mel | Scale::Linear => 0.0,
        };
        Self {
            scale,
            f_min,
            f_max,
        }
    }

    fn warp(self, f: f32) -> f32 {
        match self.scale {
            // HTK 的 mel
            Scale::Mel => 2595.0 * (1.0 + f / 700.0).log10(),
            Scale::Log => f.ln(),
            Scale::Linear => f,
        }
    }

    fn unwarp(self, w: f32) -> f32 {
        match self.scale {
            Scale::Mel => 700.0 * (10f32.powf(w / 2595.0) - 1.0),
            Scale::Log => w.exp(),
            Scale::Linear => w,
        }
    }

    pub fn unit_of(self, f: f32) -> f32 {
        let lo = self.warp(self.f_min);
        (self.warp(f) - lo) / (self.warp(self.f_max) - lo)
    }

    pub fn freq_at(self, t: f32) -> f32 {
        let lo = self.warp(self.f_min);
        self.unwarp(lo + t * (self.warp(self.f_max) - lo))
    }

    /// 高 `height` 点的轴上该标哪些频率：从下往上取，和上一个标签挤得太近的跳过。
    pub fn ticks(self, height: f32) -> Vec<f32> {
        let candidates: Vec<f32> = match self.scale {
            Scale::Mel => MEL_TICKS.to_vec(),
            Scale::Log => LOG_TICKS.to_vec(),
            Scale::Linear => {
                // 等距，挑一个放得下的步长
                let step = LINEAR_STEPS
                    .into_iter()
                    .find(|&s| self.unit_of(s) - self.unit_of(0.0) >= TICK_GAP / height)
                    .unwrap_or(LINEAR_STEPS[LINEAR_STEPS.len() - 1]);
                (1..)
                    .map(|i| i as f32 * step)
                    .take_while(|&f| f <= self.f_max)
                    .collect()
            }
        };
        let mut out: Vec<f32> = vec![];
        for f in candidates {
            if f <= self.f_min || f > self.f_max {
                continue;
            }
            let y = self.unit_of(f) * height;
            let clear = out
                .last()
                .is_none_or(|&prev| y - self.unit_of(prev) * height >= TICK_GAP);
            // 顶边也留出半个标签
            if clear && height - y >= TICK_GAP / 2.0 {
                out.push(f);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: FreqAxis = FreqAxis {
        scale: Scale::Log,
        f_min: 30.0,
        f_max: 12_000.0,
    };

    #[test]
    fn unit_roundtrip() {
        for scale in Scale::ALL {
            let axis = FreqAxis::new(scale, 8_000.0);
            for i in 0..=100 {
                let t = i as f32 / 100.0;
                assert!(
                    (axis.unit_of(axis.freq_at(t)) - t).abs() < 1e-4,
                    "{scale:?}"
                );
            }
            assert!(axis.unit_of(axis.f_min).abs() < 1e-6);
            assert!((axis.unit_of(axis.f_max) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn octaves_are_equally_spaced() {
        let width = LOG.unit_of(200.0) - LOG.unit_of(100.0);
        for f in [40.0, 333.0, 2_500.0, 5_000.0] {
            let w = LOG.unit_of(2.0 * f) - LOG.unit_of(f);
            assert!((w - width).abs() < 1e-5);
        }
    }

    #[test]
    fn mel_is_linear_low_and_logarithmic_high() {
        let mel = FreqAxis::new(Scale::Mel, 8_000.0);
        let step = |f: f32| mel.unit_of(f + 100.0) - mel.unit_of(f);
        // 1 kHz 以下 100 Hz 一格几乎等宽，往上越来越窄
        assert!((step(100.0) / step(500.0) - 1.0).abs() < 0.5);
        assert!(step(6_000.0) < step(500.0) / 3.0);
        assert!((mel.freq_at(mel.unit_of(1_000.0)) - 1_000.0).abs() < 0.5);
        let linear = FreqAxis::new(Scale::Linear, 8_000.0);
        assert!((linear.unit_of(2_000.0) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn ticks_are_spread_out_and_inside_the_axis() {
        for scale in Scale::ALL {
            for (f_max, height) in [(8_000.0, 560.0), (24_000.0, 300.0), (4_000.0, 120.0)] {
                let axis = FreqAxis::new(scale, f_max);
                let ticks = axis.ticks(height);
                assert!(ticks.len() >= 3, "{scale:?} {f_max} {height}: {ticks:?}");
                for w in ticks.windows(2) {
                    let gap = (axis.unit_of(w[1]) - axis.unit_of(w[0])) * height;
                    assert!(gap >= TICK_GAP - 1e-3, "{scale:?}: {ticks:?}");
                }
                assert!(ticks.iter().all(|&f| f > axis.f_min && f <= axis.f_max));
            }
        }
        // 线性轴等距
        let linear = FreqAxis::new(Scale::Linear, 8_000.0).ticks(560.0);
        assert_eq!(linear[0], 500.0);
        assert!(linear.windows(2).all(|w| w[1] - w[0] == 500.0));
    }
}

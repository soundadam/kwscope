/// 对数频率轴：`f_min` ↦ 0，`f_max` ↦ 1，每个八度等宽。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LogAxis {
    pub f_min: f32,
    pub f_max: f32,
}

impl LogAxis {
    pub fn unit_of(self, f: f32) -> f32 {
        (f / self.f_min).ln() / (self.f_max / self.f_min).ln()
    }

    pub fn freq_at(self, t: f32) -> f32 {
        self.f_min * (self.f_max / self.f_min).powf(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AXIS: LogAxis = LogAxis {
        f_min: 30.0,
        f_max: 12_000.0,
    };

    #[test]
    fn unit_roundtrip() {
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            assert!((AXIS.unit_of(AXIS.freq_at(t)) - t).abs() < 1e-5);
        }
    }

    #[test]
    fn octaves_are_equally_spaced() {
        let width = AXIS.unit_of(200.0) - AXIS.unit_of(100.0);
        for f in [40.0, 333.0, 2_500.0, 5_000.0] {
            let w = AXIS.unit_of(2.0 * f) - AXIS.unit_of(f);
            assert!((w - width).abs() < 1e-5);
        }
    }
}

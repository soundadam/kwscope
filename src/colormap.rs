use eframe::egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Colormap {
    Magma,
    Viridis,
    Gray,
}

impl Colormap {
    pub const ALL: [Self; 3] = [Self::Magma, Self::Viridis, Self::Gray];

    pub fn name(self) -> &'static str {
        match self {
            Self::Magma => "Magma",
            Self::Viridis => "Viridis",
            Self::Gray => "Gray",
        }
    }

    // 等距取样的近似锚点，中间线性插值
    fn anchors(self) -> &'static [[u8; 3]] {
        match self {
            Self::Magma => &[
                [0, 0, 4],
                [28, 16, 68],
                [79, 18, 123],
                [129, 37, 129],
                [181, 54, 122],
                [229, 80, 100],
                [251, 135, 97],
                [254, 194, 135],
                [252, 253, 191],
            ],
            Self::Viridis => &[
                [68, 1, 84],
                [72, 40, 120],
                [62, 73, 137],
                [49, 104, 142],
                [38, 130, 142],
                [31, 158, 137],
                [53, 183, 121],
                [110, 206, 88],
                [181, 222, 43],
                [253, 231, 37],
            ],
            Self::Gray => &[[0, 0, 0], [255, 255, 255]],
        }
    }

    pub fn lut(self) -> [Color32; 256] {
        let a = self.anchors();
        let n = a.len() - 1;
        std::array::from_fn(|i| {
            let x = i as f32 / 255.0 * n as f32;
            let k = (x as usize).min(n - 1);
            let t = x - k as f32;
            let ch = |j: usize| {
                let (p, q) = (a[k][j] as f32, a[k + 1][j] as f32);
                (p + (q - p) * t).round() as u8
            };
            Color32::from_rgb(ch(0), ch(1), ch(2))
        })
    }
}

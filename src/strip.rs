//! 语谱图的纹理当环形缓冲用：第 j 个纹理列放在槽 `j % cap`，每帧只着色新进来或新出数据的列。
//! 直播时一帧只多出两三列，不必把整幅图重新着色、重新上传。
//!
//! 列多于纹理宽度时每 `step` 列取一列（第 j 个纹理列是第 `j·step` 列）。`step` 只随看的时长与宽度变，
//! 平移时不变，所以平移也只补新露出来的列。

use crate::colormap::Colormap;
use crate::spectrogram::{Layout, Spectrogram};
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};

/// 换了这些就得整幅重画。
#[derive(Clone, Copy, PartialEq, Debug)]
struct Key {
    spectrogram: u64,
    layout: Layout,
    colormap: Colormap,
    db_lo: f32,
    db_hi: f32,
    step: usize,
    cap: usize,
}

/// 纹理里连续的一段：槽 `[slot, slot + len)` 放的是第 `first·step` 列起的 `len` 个纹理列。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Segment {
    pub slot: usize,
    pub len: usize,
    pub first: usize,
}

/// 环上的记账，和 egui 无关。
#[derive(Default)]
struct Ring {
    cap: usize,
    /// 每个槽现在画的是哪个纹理列、当时有没有数据。
    slots: Vec<Option<(usize, bool)>>,
}

impl Ring {
    fn reset(&mut self, cap: usize) {
        self.cap = cap;
        self.slots = vec![None; cap];
    }

    /// 纹理列 `[j0, j1)` 落在环上的一段或两段（绕回时）。
    fn segments(&self, j0: usize, j1: usize) -> Vec<Segment> {
        let n = j1.saturating_sub(j0).min(self.cap);
        if n == 0 {
            return vec![];
        }
        let slot = j0 % self.cap;
        let head = n.min(self.cap - slot);
        let mut out = vec![Segment {
            slot,
            len: head,
            first: j0,
        }];
        if head < n {
            out.push(Segment {
                slot: 0,
                len: n - head,
                first: j0 + head,
            });
        }
        out
    }

    /// `[j0, j1)` 里要重画的列，按槽连续的分成段；`ready(j)` 说第 j 个纹理列有没有数据。
    /// 返回的段记成已画。
    fn stale(&mut self, j0: usize, j1: usize, ready: impl Fn(usize) -> bool) -> Vec<Segment> {
        let mut out: Vec<Segment> = vec![];
        for seg in self.segments(j0, j1) {
            for k in 0..seg.len {
                let (j, slot) = (seg.first + k, seg.slot + k);
                let has = ready(j);
                if self.slots[slot] == Some((j, true)) || self.slots[slot] == Some((j, has)) {
                    continue;
                }
                self.slots[slot] = Some((j, has));
                match out.last_mut() {
                    Some(last) if last.slot + last.len == slot => last.len += 1,
                    _ => out.push(Segment {
                        slot,
                        len: 1,
                        first: j,
                    }),
                }
            }
        }
        out
    }
}

#[derive(Default)]
pub struct Strip {
    texture: Option<TextureHandle>,
    key: Option<Key>,
    ring: Ring,
}

/// 这一帧该怎么画：纹理、每列代表几列、要贴的段。
pub struct Frame {
    pub texture: egui::TextureId,
    pub step: usize,
    pub segments: Vec<Segment>,
}

impl Strip {
    /// 让纹理盖住列 `[lo, hi)`；`span_cols` 是看的时长折成的列数，`width` 是纹理最多多宽。
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        spec: &Spectrogram,
        lut: &[Color32; 256],
        colormap: Colormap,
        (db_lo, db_hi): (f32, f32),
        (lo, hi): (usize, usize),
        span_cols: usize,
        width: usize,
    ) -> Option<Frame> {
        let width = width.max(1);
        let step = span_cols.max(1).div_ceil(width);
        let cap = span_cols.div_ceil(step) + 3;
        let key = Key {
            spectrogram: spec.id(),
            layout: spec.layout(),
            colormap,
            db_lo,
            db_hi,
            step,
            cap,
        };
        let rows = key.layout.rows;
        if self.key != Some(key) {
            self.key = Some(key);
            self.ring.reset(cap);
            let blank = ColorImage::new([cap, rows], lut[0]);
            match &mut self.texture {
                Some(t) => t.set(blank, TextureOptions::LINEAR),
                None => {
                    self.texture =
                        Some(ctx.load_texture("spectrogram", blank, TextureOptions::LINEAR))
                }
            }
        }
        let texture = self.texture.as_mut()?;
        let (j0, j1) = (lo / step, hi.div_ceil(step).max(lo / step + 1));
        // 看得见的比环还宽（刚缩放的一帧）时只画右边那一段
        let j0 = j0.max(j1.saturating_sub(cap));
        for seg in self.ring.stale(j0, j1, |j| spec.has_column(j * step)) {
            let cols = (seg.first..seg.first + seg.len).map(|j| j * step);
            let image = spec.render_columns(lut, db_lo, db_hi, cols);
            texture.set_partial([seg.slot, 0], image, TextureOptions::LINEAR);
        }
        Some(Frame {
            texture: texture.id(),
            step,
            segments: self.ring.segments(j0, j1),
        })
    }

    /// 段在纹理里的横向 uv。
    pub fn uv(&self, seg: &Segment) -> (f32, f32) {
        let cap = self.ring.cap.max(1) as f32;
        (seg.slot as f32 / cap, (seg.slot + seg.len) as f32 / cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_wrap_around_the_ring() {
        let mut ring = Ring::default();
        ring.reset(10);
        assert_eq!(
            ring.segments(3, 8),
            vec![Segment {
                slot: 3,
                len: 5,
                first: 3
            }]
        );
        assert_eq!(
            ring.segments(17, 24),
            vec![
                Segment {
                    slot: 7,
                    len: 3,
                    first: 17
                },
                Segment {
                    slot: 0,
                    len: 4,
                    first: 20
                }
            ]
        );
        assert!(ring.segments(5, 5).is_empty());
    }

    #[test]
    fn only_new_or_newly_ready_columns_are_repainted() {
        let mut ring = Ring::default();
        ring.reset(10);
        let count = |segs: Vec<Segment>| segs.iter().map(|s| s.len).sum::<usize>();
        // 第一帧全画；列 6 往后还没数据
        assert_eq!(count(ring.stale(0, 8, |j| j < 6)), 8);
        assert_eq!(count(ring.stale(0, 8, |j| j < 6)), 0);
        // 列 6、7 出了数据：只补它们
        let segs = ring.stale(0, 8, |j| j < 8);
        assert_eq!(
            segs,
            vec![Segment {
                slot: 6,
                len: 2,
                first: 6
            }]
        );
        // 往右滑三列：绕回槽 0..1，只画新露出来的 8..11
        let segs = ring.stale(3, 11, |_| true);
        assert_eq!(count(segs.clone()), 3);
        assert_eq!(segs.last().unwrap().slot, 0);
        assert_eq!(ring.slots[0], Some((10, true)));
        // 往回拖：槽 0 里是第 10 列，现在要第 0 列
        assert_eq!(count(ring.stale(0, 3, |_| true)), 1);
    }
}

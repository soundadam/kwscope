//! 分数流上逐帧判触发（`kws_infer.detector` 的移植），即 `det` 流式计数的规则：
//!
//! - 开机头 `warmup` 帧不找触发；
//! - 第一帧到阈值就 arm，判在从那里开的 `window` 帧里第一个最高的那帧，窗关了才知道；
//! - 有 `settle_drop` 时，比之前最高分低出这么多的第一帧让窗提前关，判那个峰；
//! - 下一个触发从「窗关处」与「判定帧加耳聋时长」较晚的那个起找：二级接受后是
//!   `lockout`，拒绝后是 `rearm`。
//!
//! 四个长度都是模型帧网格上的帧数，drop 是分数单位。

use anyhow::{ensure, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorConfig {
    pub threshold: f64,
    pub window: usize,
    pub lockout: usize,
    pub warmup: usize,
    pub rearm: usize,
    pub settle_drop: Option<f64>,
}

impl DetectorConfig {
    /// det.v8 的 window / lockout / warmup：500 / 800 / 1500 ms 在 10 ms 一跳上。
    pub fn det_v8(threshold: f64) -> Self {
        Self {
            threshold,
            window: 50,
            lockout: 80,
            warmup: 150,
            rearm: 0,
            settle_drop: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.window >= 1 && self.lockout >= 1,
            "window and lockout must be at least one frame"
        );
        if let Some(drop) = self.settle_drop {
            ensure!(drop > 0.0, "settle_drop must be positive");
        }
        Ok(())
    }
}

/// 判下来、等二级裁决的触发。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub arm: usize,
    pub decision: usize,
    pub score: f64,
    /// 窗读过的最后一帧的下一帧：窗关在这里。
    pub close: usize,
}

#[derive(Clone, Debug)]
pub struct TriggerDetector {
    pub config: DetectorConfig,
    /// 已经 step 过的帧数。
    pub frame: usize,
    listen_from: usize,
    arm: Option<usize>,
    peak: usize,
    peak_score: f64,
    open: Option<Candidate>,
}

impl TriggerDetector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            frame: 0,
            listen_from: config.warmup,
            arm: None,
            peak: 0,
            peak_score: f64::NEG_INFINITY,
            open: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.config);
    }

    /// 下一帧的分数；有窗在这帧关上时出一个候选，下一帧之前必须 [`settle`](Self::settle)。
    pub fn step(&mut self, score: f64) -> Option<Candidate> {
        assert!(
            self.open.is_none(),
            "settle the last candidate before the next frame"
        );
        let frame = self.frame;
        self.frame += 1;
        let arm = match self.arm {
            Some(arm) => arm,
            None => {
                if frame < self.listen_from || score < self.config.threshold {
                    return None;
                }
                self.arm = Some(frame);
                frame
            }
        };
        if score > self.peak_score {
            self.peak = frame;
            self.peak_score = score;
        } else if let Some(drop) = self.config.settle_drop {
            if score <= self.peak_score - drop {
                return Some(self.close(arm));
            }
        }
        if frame - arm + 1 < self.config.window {
            return None;
        }
        Some(self.close(arm))
    }

    /// 流结束：还开着的窗按已有的判。
    pub fn flush(&mut self) -> Option<Candidate> {
        if self.open.is_some() {
            return None;
        }
        let arm = self.arm?;
        Some(self.close(arm))
    }

    pub fn settle(&mut self, accepted: bool) {
        let open = self.open.take().expect("no candidate to settle");
        let deaf = if accepted {
            self.config.lockout
        } else {
            self.config.rearm
        };
        self.listen_from = (open.decision + deaf.max(1)).max(open.close);
    }

    fn close(&mut self, arm: usize) -> Candidate {
        let candidate = Candidate {
            arm,
            decision: self.peak,
            score: self.peak_score,
            close: self.frame,
        };
        self.open = Some(candidate);
        self.arm = None;
        self.peak_score = f64::NEG_INFINITY;
        candidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(config: DetectorConfig, scores: &[f64], accept: bool) -> Vec<Candidate> {
        let mut d = TriggerDetector::new(config);
        let mut out = vec![];
        for &s in scores {
            if let Some(c) = d.step(s) {
                d.settle(accept);
                out.push(c);
            }
        }
        if let Some(c) = d.flush() {
            d.settle(accept);
            out.push(c);
        }
        out
    }

    fn config() -> DetectorConfig {
        DetectorConfig {
            threshold: 0.5,
            window: 3,
            lockout: 5,
            warmup: 1,
            rearm: 0,
            settle_drop: None,
        }
    }

    #[test]
    fn decides_first_peak_of_the_window() {
        let got = run(config(), &[0.9, 0.0, 0.6, 0.8, 0.8, 0.1], true);
        assert_eq!(
            got,
            vec![Candidate {
                arm: 2,
                decision: 3,
                score: 0.8,
                close: 5
            }]
        );
    }

    #[test]
    fn lockout_after_accept_rearm_after_reject() {
        let scores = [0.0, 0.9, 0.0, 0.0, 0.9, 0.0, 0.0, 0.9, 0.0, 0.0];
        let accepted: Vec<_> = run(config(), &scores, true)
            .iter()
            .map(|c| c.decision)
            .collect();
        assert_eq!(accepted, vec![1, 7]);
        let rejected: Vec<_> = run(config(), &scores, false)
            .iter()
            .map(|c| c.decision)
            .collect();
        assert_eq!(rejected, vec![1, 4, 7]);
    }

    #[test]
    fn settle_drop_closes_early_and_flush_decides_open_window() {
        let mut c = config();
        c.window = 10;
        c.settle_drop = Some(0.3);
        let got = run(c, &[0.0, 0.7, 0.9, 0.5, 0.0, 0.8], true);
        assert_eq!((got[0].decision, got[0].close), (2, 4));
        let got = run(config(), &[0.0, 0.0, 0.7], true);
        assert_eq!((got[0].arm, got[0].decision, got[0].close), (2, 2, 3));
    }
}

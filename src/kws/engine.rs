//! 一级、检测器、二级串起来（`kws_infer.stage_one.StageOne` 的移植）。
//!
//! 流式的 [`Engine`] 每次推送把新凑齐的帧过一级，按关键词逐帧过检测器；判下来的
//! 触发交给二级，二级读开机以来留着的音频，裁决决定检测器耳聋多久。
//!
//! 离线的 [`Analysis`] 把整个文件按同样的推送步长过一级、把分数存下来；改阈值时
//! [`Analysis::detect`] 只重放检测器与二级，不再跑一级。两条路出的触发和
//! `kws stream` 一行不差（`tests/golden.rs` 钉这一条）。

use anyhow::{ensure, Result};
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use super::adapter::{Scores, StageOneInfo, StageOneModel, StageTwoModel};
use super::detector::{Candidate, DetectorConfig, TriggerDetector};

/// `kws stream` 一次推 100 ms。
pub const CHUNK_MS: f64 = 100.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    pub keyword: String,
    /// 开机以来的帧；第 t 帧是样本 [t * hop, t * hop + n_fft)。
    pub arm_frame: usize,
    pub decision_frame: usize,
    pub score: f64,
    /// 判定帧上的偏移头：词起点在这么多帧之前。
    pub start_offset_frames: f64,
    /// 开机以来的样本：偏移头给的词起点（截到 [0, 感受野] 帧前）与判定帧的末尾。
    pub start_sample: usize,
    pub end_sample: usize,
    /// 二级的裁决；没有二级时恒为 true。
    pub accepted: bool,
    pub stage_two_logit: Option<f64>,
    /// 二级最早读到的样本（画导程用）。
    pub stage_two_from: Option<usize>,
    /// 二级这次打分花的秒数；从缓存取的是 None。
    pub stage_two_seconds: Option<f64>,
}

/// 二级与它的阈值，外加按 (关键词, 起点, 终点) 记住的分：同一段音频分数不变，
/// 改阈值重放时不必再跑一遍。
pub struct Judge {
    pub model: Box<dyn StageTwoModel>,
    pub threshold: f64,
    pub cache: HashMap<(String, usize, usize), f64>,
}

impl Judge {
    pub fn new(model: Box<dyn StageTwoModel>, threshold: f64) -> Self {
        Self {
            model,
            threshold,
            cache: HashMap::new(),
        }
    }

    fn judge(
        &mut self,
        trigger: &mut Trigger,
        read: &dyn Fn(usize, usize) -> Vec<f32>,
    ) -> Result<()> {
        let key = (
            trigger.keyword.clone(),
            trigger.start_sample,
            trigger.end_sample,
        );
        let logit = match self.cache.get(&key) {
            Some(&logit) => logit,
            None => {
                let began = Instant::now();
                let logit = self.model.logit(
                    &trigger.keyword,
                    trigger.start_sample,
                    trigger.end_sample,
                    read,
                )?;
                trigger.stage_two_seconds = Some(began.elapsed().as_secs_f64());
                self.cache.insert(key, logit);
                logit
            }
        };
        trigger.stage_two_logit = Some(logit);
        trigger.stage_two_from = Some(
            self.model
                .read_from(trigger.start_sample, trigger.end_sample),
        );
        trigger.accepted = logit >= self.threshold;
        Ok(())
    }
}

/// 每个关键词一个检测器，外加最近一个窗的偏移：判定最多落在关窗那帧之前一个窗。
pub struct Detection {
    info: StageOneInfo,
    detectors: Vec<TriggerDetector>,
    offsets: Vec<VecDeque<f64>>,
}

impl Detection {
    pub fn new(info: StageOneInfo, config: DetectorConfig) -> Self {
        let k = info.keywords.len();
        Self {
            info,
            detectors: vec![TriggerDetector::new(config); k],
            offsets: vec![VecDeque::new(); k],
        }
    }

    pub fn config(&self) -> DetectorConfig {
        self.detectors[0].config
    }

    /// 换阈值等设置，不回到开机：直播里拖滑块用。
    pub fn set_config(&mut self, config: DetectorConfig) {
        for d in &mut self.detectors {
            d.config = config;
        }
    }

    pub fn reset(&mut self) {
        for d in &mut self.detectors {
            d.reset();
        }
        for o in &mut self.offsets {
            o.clear();
        }
    }

    /// 一次推送出的帧，按关键词逐帧过检测器。
    fn step(
        &mut self,
        scores: &Scores,
        frames: std::ops::Range<usize>,
        judge: &mut dyn FnMut(&mut Trigger) -> Result<()>,
    ) -> Result<Vec<Trigger>> {
        let k = self.info.keywords.len();
        let window = self.config().window;
        let mut out = vec![];
        for index in 0..k {
            for f in frames.clone() {
                let kept = &mut self.offsets[index];
                kept.push_back(scores.offsets[f * k + index]);
                while kept.len() > window {
                    kept.pop_front();
                }
                if let Some(candidate) = self.detectors[index].step(scores.probs[f * k + index]) {
                    out.push(self.settle(index, candidate, judge)?);
                }
            }
        }
        Ok(out)
    }

    /// 流结束：还开着的窗也判掉。
    fn flush(&mut self, judge: &mut dyn FnMut(&mut Trigger) -> Result<()>) -> Result<Vec<Trigger>> {
        let mut out = vec![];
        for index in 0..self.detectors.len() {
            if let Some(candidate) = self.detectors[index].flush() {
                out.push(self.settle(index, candidate, judge)?);
            }
        }
        Ok(out)
    }

    fn settle(
        &mut self,
        index: usize,
        candidate: Candidate,
        judge: &mut dyn FnMut(&mut Trigger) -> Result<()>,
    ) -> Result<Trigger> {
        let kept = &self.offsets[index];
        let back = self.detectors[index].frame - candidate.decision;
        ensure!(
            back >= 1 && back <= kept.len(),
            "decision fell out of the kept offsets"
        );
        let offset = kept[kept.len() - back];
        let hop = self.info.hop as f64;
        let rf = self.info.receptive_field as f64;
        let span = offset.clamp(0.0, rf);
        let end = self.info.frame_end(candidate.decision);
        // Python 的 round 是四舍六入五成双
        let start = ((candidate.decision as f64 - span) * hop)
            .round_ties_even()
            .max(0.0) as usize;
        let mut trigger = Trigger {
            keyword: self.info.keywords[index].clone(),
            arm_frame: candidate.arm,
            decision_frame: candidate.decision,
            score: candidate.score,
            start_offset_frames: offset,
            start_sample: start,
            end_sample: end,
            accepted: true,
            stage_two_logit: None,
            stage_two_from: None,
            stage_two_seconds: None,
        };
        let verdict = judge(&mut trigger);
        // 二级出错也要把检测器放走，否则下一帧就断言失败
        self.detectors[index].settle(trigger.accepted);
        verdict?;
        Ok(trigger)
    }
}

/// 一级留的音频：开机以来最近 `history` 个样本。
struct History {
    buf: VecDeque<f32>,
    /// 开机以来的样本数。
    samples: usize,
}

impl History {
    fn read(&self, start: usize, end: usize) -> Vec<f32> {
        let first = self.samples - self.buf.len();
        let lo = start.max(first) - first;
        let hi = end.min(self.samples).saturating_sub(first);
        if hi > lo {
            self.buf.range(lo..hi).copied().collect()
        } else {
            vec![]
        }
    }

    fn keep(&mut self, n: usize) {
        while self.buf.len() > n {
            self.buf.pop_front();
        }
    }
}

/// 二级读多远，一级就在自己的下限之上多留多少：`kws stream` 的 4 s 加二级的够度。
pub fn history(
    info: &StageOneInfo,
    config: &DetectorConfig,
    stage_two: Option<&dyn StageTwoModel>,
) -> usize {
    let mut seconds = 4.0;
    if let Some(two) = stage_two {
        seconds += two.info().reach as f64 / two.info().sample_rate as f64;
    }
    let floor = (info.receptive_field + config.window + 1) * info.hop + info.n_fft;
    ((seconds * info.sample_rate as f64).round_ties_even() as usize).max(floor)
}

/// 两级能不能接上：二级要认得一级的每个关键词，采样率一样。
pub fn check_pair(one: &StageOneInfo, two: &dyn StageTwoModel) -> Result<()> {
    let info = two.info();
    let missing: Vec<_> = one
        .keywords
        .iter()
        .filter(|k| !info.keywords.contains(k))
        .collect();
    ensure!(
        missing.is_empty() && info.sample_rate == one.sample_rate,
        "stage two reads {:?} at {} Hz, stage one fires {:?} at {} Hz",
        info.keywords,
        info.sample_rate,
        one.keywords,
        one.sample_rate
    );
    Ok(())
}

pub struct Pushed {
    pub scores: Scores,
    pub triggers: Vec<Trigger>,
}

/// 流式的两级：麦克风、`kwscope stream` 都走这条。
pub struct Engine {
    pub stage_one: Box<dyn StageOneModel>,
    pub judge: Option<Judge>,
    pub detection: Detection,
    history: History,
    keep: usize,
}

impl Engine {
    pub fn new(
        stage_one: Box<dyn StageOneModel>,
        judge: Option<Judge>,
        config: DetectorConfig,
    ) -> Result<Self> {
        config.validate()?;
        let info = stage_one.info().clone();
        if let Some(j) = &judge {
            check_pair(&info, j.model.as_ref())?;
        }
        let keep = history(&info, &config, judge.as_ref().map(|j| j.model.as_ref()));
        Ok(Self {
            detection: Detection::new(info, config),
            stage_one,
            judge,
            history: History {
                buf: VecDeque::new(),
                samples: 0,
            },
            keep,
        })
    }

    pub fn info(&self) -> &StageOneInfo {
        self.stage_one.info()
    }

    /// 拆回两级，换去跑别的（比如离线文件）。
    pub fn into_parts(self) -> (Box<dyn StageOneModel>, Option<Judge>) {
        (self.stage_one, self.judge)
    }

    /// 开机以来推进来的样本数。
    pub fn samples(&self) -> usize {
        self.history.samples
    }

    pub fn reset(&mut self) {
        self.stage_one.reset();
        self.detection.reset();
        self.history.buf.clear();
        self.history.samples = 0;
        if let Some(j) = &mut self.judge {
            j.cache.clear();
        }
    }

    /// [-1, 1] 的样本，任意个数；新帧的分数和它们判下来的触发。
    pub fn push(&mut self, samples: &[f32]) -> Result<Pushed> {
        self.history.samples += samples.len();
        // 这块里早判下来的触发要从那里往回读一个历史，块先整个留着
        self.history.buf.extend(samples);
        self.history.keep(self.keep + samples.len());
        let scores = self.stage_one.push(samples)?;
        let history = &self.history;
        let judge = &mut self.judge;
        let mut verdict = |t: &mut Trigger| match judge {
            Some(j) => j.judge(t, &|a, b| history.read(a, b)),
            None => Ok(()),
        };
        let triggers = self.detection.step(&scores, 0..scores.frames, &mut verdict);
        self.history.keep(self.keep);
        Ok(Pushed {
            triggers: triggers?,
            scores,
        })
    }

    pub fn flush(&mut self) -> Result<Vec<Trigger>> {
        let history = &self.history;
        let judge = &mut self.judge;
        let mut verdict = |t: &mut Trigger| match judge {
            Some(j) => j.judge(t, &|a, b| history.read(a, b)),
            None => Ok(()),
        };
        self.detection.flush(&mut verdict)
    }

    /// 整段样本按 `step` 一推，推完 flush：`kws stream` 对一个文件做的事。
    pub fn run(&mut self, samples: &[f32], step: usize) -> Result<Vec<Trigger>> {
        let mut out = vec![];
        for chunk in samples.chunks(step.max(1)) {
            out.extend(self.push(chunk)?.triggers);
        }
        out.extend(self.flush()?);
        Ok(out)
    }
}

/// 推送步长：`round(chunk_ms / 1000 * sr)`，至少 1。
pub fn step_samples(sample_rate: u32, chunk_ms: f64) -> usize {
    ((chunk_ms / 1000.0 * sample_rate as f64).round_ties_even() as usize).max(1)
}

/// 一个文件的一级分数，按推送分好组：重放检测器时触发的顺序跟流式一样。
pub struct Analysis {
    pub info: StageOneInfo,
    pub scores: Scores,
    /// 每次推送结束时累计的帧数。
    pub pushes: Vec<usize>,
}

impl Analysis {
    /// 一级从开机跑过整个文件；`progress` 收到已经跑过的样本数，返回 false 就停。
    pub fn run(
        stage_one: &mut dyn StageOneModel,
        samples: &[f32],
        step: usize,
        mut progress: impl FnMut(usize) -> bool,
    ) -> Result<Option<Self>> {
        stage_one.reset();
        let k = stage_one.info().keywords.len();
        let mut scores = Scores::default();
        let mut pushes = vec![];
        for (i, chunk) in samples.chunks(step.max(1)).enumerate() {
            let got = stage_one.push(chunk)?;
            ensure!(
                got.probs.len() == got.frames * k,
                "stage one gave a ragged push"
            );
            scores.frames += got.frames;
            scores.probs.extend(got.probs);
            scores.offsets.extend(got.offsets);
            pushes.push(scores.frames);
            if !progress((i + 1) * step) {
                return Ok(None);
            }
        }
        Ok(Some(Self {
            info: stage_one.info().clone(),
            scores,
            pushes,
        }))
    }

    /// 存下的分数上重放检测器，二级读整个文件（一级的历史总装得下二级要读的）。
    pub fn detect(
        &self,
        config: DetectorConfig,
        samples: &[f32],
        judge: Option<&mut Judge>,
    ) -> Result<Vec<Trigger>> {
        config.validate()?;
        if let Some(j) = &judge {
            check_pair(&self.info, j.model.as_ref())?;
        }
        let mut detection = Detection::new(self.info.clone(), config);
        let read =
            |a: usize, b: usize| samples[a.min(samples.len())..b.min(samples.len())].to_vec();
        let mut judge = judge;
        let mut verdict = |t: &mut Trigger| match &mut judge {
            Some(j) => j.judge(t, &read),
            None => Ok(()),
        };
        let mut out = vec![];
        let mut from = 0;
        for &to in &self.pushes {
            out.extend(detection.step(&self.scores, from..to, &mut verdict)?);
            from = to;
        }
        out.extend(detection.flush(&mut verdict)?);
        Ok(out)
    }

    /// 第 `frame` 帧、第 `index` 个关键词的分。
    pub fn prob(&self, frame: usize, index: usize) -> f64 {
        self.scores.probs[frame * self.info.keywords.len() + index]
    }
}

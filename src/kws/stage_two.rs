//! `kws_stage_two.v2`：`kws export stage-two` 写的一个检查器跑次或它们的集成。
//!
//! 每个跑次按每个导程切一段：从词起点往前 `lead` 个样本（或判定帧之前固定的
//! `fixed` 个）到判定帧的最后一个样本，开机处截住、至少一帧；算 log-mel，训练时
//! 减了每带均值就减，在触发的关键词上打分：`disc` 乘 logit 加 `phone` 乘
//! [`keyword_score`]。跑次的分是各导程的均值，集成的分是各跑次的均值。

use anyhow::{anyhow, bail, ensure, Context, Result};
use ort::session::Session;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

use super::adapter::{self, AudioReader, StageTwoInfo, StageTwoModel};
use super::frontend::{FrontendArrays, LogFeatures};
use super::{npy, onnx};

const LOGITS_OUTPUT: &str = "logits";
const PHONES_OUTPUT: &str = "phones";

pub struct Member {
    pub name: String,
    pub log: LogFeatures,
    pub cmn: bool,
    pub leads: Vec<usize>,
    pub fixed: Option<usize>,
    pub min_samples: usize,
    session: Session,
}

impl Member {
    /// 词起点 `start` 的触发在 `lead` 上读的 `[lo, end)`。
    pub fn span(&self, start: usize, end: usize, lead: usize) -> (usize, usize) {
        let lo = match self.fixed {
            Some(fixed) => end as i64 - fixed as i64,
            None => start as i64 - lead as i64,
        };
        let lo = lo.min(end as i64 - self.min_samples as i64).max(0);
        (lo as usize, end)
    }

    /// 一段的 (frames, n_mels) log-mel；`cmn` 时减每带在段上的均值。
    pub fn features(&mut self, audio: &[f32]) -> Vec<f32> {
        let samples: Vec<f64> = audio.iter().map(|&s| s as f64).collect();
        let mut feat = self.log.all(&samples);
        let dim = self.log.dim();
        let n = feat.len() / dim;
        if self.cmn && n > 0 {
            for b in 0..dim {
                let mean = (0..n).map(|t| feat[t * dim + b]).sum::<f64>() / n as f64;
                for t in 0..n {
                    feat[t * dim + b] -= mean;
                }
            }
        }
        feat.into_iter().map(|v| v as f32).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Scoring {
    pub disc: f64,
    pub phone: f64,
    pub phones: HashMap<String, Vec<usize>>,
    pub blank: Option<usize>,
    pub floor: Option<f64>,
}

pub struct KwsStageTwoV2 {
    info: StageTwoInfo,
    pub members: Vec<Member>,
    pub scoring: Scoring,
}

impl KwsStageTwoV2 {
    pub const FORMAT: &'static str = "kws_stage_two.v2";
    pub const META_FILE: &'static str = "stage_two.json";

    pub fn open(root: &Path) -> Result<Self> {
        let file = root.join(Self::META_FILE);
        let meta: Value = serde_json::from_slice(
            &std::fs::read(&file).with_context(|| format!("read {}", file.display()))?,
        )?;
        Self::load(root, &meta)
    }

    pub fn load(root: &Path, meta: &Value) -> Result<Self> {
        ensure!(
            meta.get("format").and_then(Value::as_str) == Some(Self::FORMAT),
            "not a {} bundle",
            Self::FORMAT
        );
        let scoring = parse_scoring(adapter::field(meta, "scoring")?)?;
        let members = adapter::field(meta, "members")?
            .as_array()
            .context("members must be a list")?
            .iter()
            .map(|m| load_member(root, m, &scoring))
            .collect::<Result<Vec<_>>>()?;
        ensure!(!members.is_empty(), "stage two has no members");
        let reach = members
            .iter()
            .map(|m| {
                m.fixed
                    .unwrap_or_else(|| *m.leads.iter().max().expect("checked"))
            })
            .max()
            .expect("checked");
        let keywords = adapter::strings(meta, "keywords")?;
        if scoring.phone > 0.0 {
            for k in &keywords {
                ensure!(
                    scoring.phones.contains_key(k),
                    "scoring has no phones for {k}"
                );
            }
        }
        Ok(Self {
            info: StageTwoInfo {
                format: Self::FORMAT.into(),
                root: root.to_path_buf(),
                sample_rate: adapter::uint(meta, "sample_rate")? as u32,
                keywords,
                members: members.len(),
                reach,
            },
            members,
            scoring,
        })
    }

    /// 一个跑次在一段特征上的分。
    pub fn span_score(
        &mut self,
        member: usize,
        channel: usize,
        keyword: &str,
        feats: Vec<f32>,
    ) -> Result<f64> {
        let scoring = &self.scoring;
        let m = &mut self.members[member];
        let dim = m.log.dim();
        let n = feats.len() / dim;
        let outputs = m
            .session
            .run(ort::inputs!["feats" => onnx::tensor(&[1, n, dim], feats)?])
            .map_err(|e| anyhow!("stage two {}: {e}", m.name))?;
        let mut score = 0.0;
        if scoring.disc != 0.0 {
            let (_, logits) = outputs[LOGITS_OUTPUT]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("stage two logits: {e}"))?;
            let logit = *logits
                .get(channel)
                .with_context(|| format!("stage two gave {} logits", logits.len()))?;
            score += scoring.disc * logit as f64;
        }
        if scoring.phone != 0.0 {
            let (shape, phones) = outputs[PHONES_OUTPUT]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("stage two phones: {e}"))?;
            ensure!(shape.len() == 3, "phones output is {shape:?}");
            let width = shape[2] as usize;
            let rows: Vec<f64> = phones.iter().map(|&v| v as f64).collect();
            score += scoring.phone
                * keyword_score(
                    &rows,
                    width,
                    &scoring.phones[keyword],
                    scoring.blank.expect("checked"),
                    scoring.floor.expect("checked"),
                );
        }
        Ok(score)
    }
}

impl StageTwoModel for KwsStageTwoV2 {
    fn info(&self) -> &StageTwoInfo {
        &self.info
    }

    fn read_from(&self, start: usize, end: usize) -> usize {
        self.members
            .iter()
            .flat_map(|m| m.leads.iter().map(move |&lead| m.span(start, end, lead).0))
            .min()
            .unwrap_or(start)
    }

    fn logit(&mut self, keyword: &str, start: usize, end: usize, read: AudioReader) -> Result<f64> {
        let channel = self
            .info
            .keywords
            .iter()
            .position(|k| k == keyword)
            .with_context(|| format!("stage two has no keyword {keyword}"))?;
        let mut total = 0.0;
        for member in 0..self.members.len() {
            let leads = self.members[member].leads.clone();
            let mut sum = 0.0;
            for &lead in &leads {
                let (lo, hi) = self.members[member].span(start, end, lead);
                let audio = read(lo, hi);
                if audio.len() != hi - lo {
                    bail!(
                        "stage one holds {} of the {} samples {} reads; keep {:.3} s more history",
                        audio.len(),
                        hi - lo,
                        self.members[member].name,
                        self.info.reach as f64 / self.info.sample_rate as f64
                    );
                }
                let feats = self.members[member].features(&audio);
                sum += self.span_score(member, channel, keyword, feats)?;
            }
            total += sum / leads.len() as f64;
        }
        Ok(total / self.members.len() as f64)
    }
}

fn parse_scoring(raw: &Value) -> Result<Scoring> {
    let phones = match raw.get("phones") {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                let ids = v
                    .as_array()
                    .context("phones must be lists")?
                    .iter()
                    .map(|p| {
                        p.as_u64()
                            .map(|p| p as usize)
                            .context("phone ids are integers")
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((k.clone(), ids))
            })
            .collect::<Result<HashMap<_, _>>>()?,
        _ => HashMap::new(),
    };
    let scoring = Scoring {
        disc: adapter::float(raw, "disc")?,
        phone: adapter::float(raw, "phone")?,
        phones,
        blank: raw.get("blank").and_then(Value::as_u64).map(|v| v as usize),
        floor: raw.get("floor").and_then(Value::as_f64),
    };
    ensure!(
        scoring.disc >= 0.0 && scoring.phone >= 0.0 && (scoring.disc > 0.0 || scoring.phone > 0.0),
        "scoring weights must be >= 0, not both 0"
    );
    ensure!(
        scoring.phone == 0.0
            || (!scoring.phones.is_empty() && scoring.blank.is_some() && scoring.floor.is_some()),
        "a phone weight needs each keyword's phones, the blank and the floor"
    );
    Ok(scoring)
}

fn load_member(root: &Path, raw: &Value, scoring: &Scoring) -> Result<Member> {
    let name = adapter::field(raw, "model")?
        .as_str()
        .context("model must be a file name")?
        .to_owned();
    let frontend = adapter::field(raw, "frontend")?
        .as_str()
        .context("frontend must be a file name")?;
    for file in [name.as_str(), frontend] {
        ensure!(root.join(file).is_file(), "{}: no {file}", root.display());
    }
    let leads = adapter::field(raw, "leads")?
        .as_array()
        .context("leads must be a list")?
        .iter()
        .map(|v| {
            v.as_u64()
                .map(|v| v as usize)
                .context("leads are sample counts")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        !leads.is_empty(),
        "{name}: a member reads at least one lead"
    );
    let arrays = FrontendArrays::from_npz(npy::load_npz(&root.join(frontend))?)?;
    let log = LogFeatures::new(
        adapter::uint(raw, "n_fft")?,
        adapter::uint(raw, "hop_length")?,
        adapter::float(raw, "log_offset")?,
        &arrays,
    )?;
    let feature_dim = adapter::uint(raw, "feature_dim")?;
    ensure!(
        log.dim() == feature_dim,
        "{frontend} gives {} features, the member says {feature_dim}",
        log.dim()
    );
    let session = onnx::session(&root.join(&name))?;
    let outputs = onnx::output_names(&session);
    for (output, weight) in [
        (LOGITS_OUTPUT, scoring.disc),
        (PHONES_OUTPUT, scoring.phone),
    ] {
        ensure!(
            weight == 0.0 || outputs.iter().any(|o| o == output),
            "{name} has no {output} output (has {outputs:?})"
        );
    }
    Ok(Member {
        log,
        cmn: adapter::field(raw, "cmn")?
            .as_bool()
            .context("cmn must be a bool")?,
        leads,
        fixed: match raw.get("fixed") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().context("fixed is a sample count")? as usize),
        },
        min_samples: adapter::uint(raw, "min_samples")?,
        session,
        name,
    })
}

/// 一段的音素帧 logits `(T', P + 1)`（按行摊平，每行 `width`）读 `phones` 读得多好。
///
/// 自由音素环、关键词的 CTC 状态（音素之间与两头各一个 blank）、再一个自由音素环
/// 上的 Viterbi，环上一帧取它最好的音素，在 log-softmax 上算。减去整段只走环的分、
/// 除以音素数；没有路径装得下关键词时取 `floor`，也不低于它。
pub fn keyword_score(
    logits: &[f64],
    width: usize,
    phones: &[usize],
    blank: usize,
    floor: f64,
) -> f64 {
    let t_len = logits.len() / width;
    let mut lp = vec![0.0; logits.len()];
    let mut best = vec![0.0; t_len];
    for t in 0..t_len {
        let row = &logits[t * width..(t + 1) * width];
        let top = row.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let norm = row.iter().map(|x| (x - top).exp()).sum::<f64>().ln();
        for (o, x) in lp[t * width..(t + 1) * width].iter_mut().zip(row) {
            *o = x - top - norm;
        }
        best[t] = lp[t * width..(t + 1) * width]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
    }
    let mut ext = vec![blank];
    for &p in phones {
        ext.push(p);
        ext.push(blank);
    }
    let l = ext.len();
    let emit = |t: usize, s: usize| lp[t * width + ext[s]];
    let skip: Vec<bool> = (0..l)
        .map(|s| s >= 2 && ext[s] != blank && ext[s] != ext[s - 2])
        .collect();
    let mut pre = best[0];
    let mut ctc = vec![f64::NEG_INFINITY; l];
    ctc[0] = emit(0, 0);
    ctc[1] = emit(0, 1);
    let mut post = f64::NEG_INFINITY;
    for (t, &b) in best.iter().enumerate().take(t_len).skip(1) {
        let mut step = ctc.clone();
        for s in 1..l {
            step[s] = step[s].max(ctc[s - 1]);
        }
        for s in 2..l {
            if skip[s] {
                step[s] = step[s].max(ctc[s - 2]);
            }
        }
        step[0] = step[0].max(pre);
        step[1] = step[1].max(pre);
        post = b + post.max(ctc[l - 1]).max(ctc[l - 2]);
        for s in 0..l {
            ctc[s] = step[s] + emit(t, s);
        }
        pre += b;
    }
    let mut held = post.max(ctc[l - 1]).max(ctc[l - 2]);
    if !held.is_finite() {
        held = f64::NEG_INFINITY;
    }
    ((held - best.iter().sum::<f64>()) / phones.len() as f64).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_score_prefers_the_keyword_and_floors_misses() {
        // 3 个通道：blank=0，音素 1、2；6 帧依次念 1 1 0 2 2 0
        let hot = |c: usize| (0..3).map(move |i| if i == c { 5.0 } else { 0.0 });
        let logits: Vec<f64> = [1, 1, 0, 2, 2, 0].iter().flat_map(|&c| hot(c)).collect();
        let hit = keyword_score(&logits, 3, &[1, 2], 0, -10.0);
        let miss = keyword_score(&logits, 3, &[2, 1], 0, -10.0);
        assert!(hit > miss, "{hit} vs {miss}");
        assert!(
            hit.abs() < 1e-9,
            "a path through every best frame costs nothing: {hit}"
        );
        // 两帧装不下三个音素
        assert_eq!(keyword_score(&logits[..6], 3, &[1, 2, 1], 0, -10.0), -10.0);
    }
}

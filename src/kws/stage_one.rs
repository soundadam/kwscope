//! `kws_stage_one.v1`：`kws export stage-one` 写的三个文件。
//!
//! - `stage_one.onnx`：`feats` (1, T, dim) 与打包的 `cache_in` (1, 1, cache_size) 进，
//!   `logits` (1, T, 2K) 与 `cache_out` 出；
//! - `frontend.npz`：窗、滤波器组、DCT、`c0` 滑动均值核、逆 DCT；
//! - `stage_one.json`：标量。
//!
//! 每次推送用上次留下的缓存跑新凑齐的帧，分数就是从开机起整条流一次前向的分数。

use anyhow::{anyhow, ensure, Context, Result};
use ort::session::Session;
use serde_json::Value;
use std::path::Path;

use super::adapter::{self, Scores, StageOneInfo, StageOneModel};
use super::frontend::{FrontendArrays, StreamingFrontend};
use super::{npy, onnx};

pub struct KwsStageOneV1 {
    info: StageOneInfo,
    pub frontend: StreamingFrontend,
    session: Session,
    cache_size: usize,
    cache: Vec<f32>,
}

impl KwsStageOneV1 {
    pub const FORMAT: &'static str = "kws_stage_one.v1";
    pub const META_FILE: &'static str = "stage_one.json";
    pub const MODEL_FILE: &'static str = "stage_one.onnx";
    pub const FRONTEND_FILE: &'static str = "frontend.npz";

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
        for name in [Self::MODEL_FILE, Self::FRONTEND_FILE] {
            ensure!(root.join(name).is_file(), "{}: no {name}", root.display());
        }
        let info = StageOneInfo {
            format: Self::FORMAT.into(),
            root: root.to_path_buf(),
            sample_rate: adapter::uint(meta, "sample_rate")? as u32,
            n_fft: adapter::uint(meta, "n_fft")?,
            hop: adapter::uint(meta, "hop_length")?,
            keywords: adapter::strings(meta, "keywords")?,
            receptive_field: adapter::uint(meta, "receptive_field")?,
        };
        ensure!(!info.keywords.is_empty(), "stage one has no keywords");
        let arrays = FrontendArrays::from_npz(npy::load_npz(&root.join(Self::FRONTEND_FILE))?)?;
        let frontend = StreamingFrontend::new(
            info.n_fft,
            info.hop,
            adapter::float(meta, "log_offset")?,
            &arrays,
        )?;
        let feature_dim = adapter::uint(meta, "feature_dim")?;
        ensure!(
            frontend.feature_dim() == feature_dim,
            "frontend.npz gives {} features, stage_one.json says {feature_dim}",
            frontend.feature_dim()
        );
        let cache_size = adapter::uint(meta, "cache_size")?;
        Ok(Self {
            session: onnx::session(&root.join(Self::MODEL_FILE))?,
            cache: vec![0.0; cache_size],
            cache_size,
            frontend,
            info,
        })
    }

    /// (n, dim) 的特征按缓存跑一遍：(n, 2K) 的原始 logits。
    pub fn forward(&mut self, feats: &[f32]) -> Result<Vec<f32>> {
        let dim = self.frontend.feature_dim();
        let n = feats.len() / dim;
        if n == 0 {
            return Ok(vec![]);
        }
        let outputs = self
            .session
            .run(ort::inputs![
                "feats" => onnx::tensor(&[1, n, dim], feats.to_vec())?,
                "cache_in" => onnx::tensor(&[1, 1, self.cache_size], self.cache.clone())?,
            ])
            .map_err(|e| anyhow!("stage one: {e}"))?;
        let (_, logits) = outputs["logits"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("stage one logits: {e}"))?;
        let k2 = 2 * self.info.keywords.len();
        ensure!(
            logits.len() == n * k2,
            "stage one gave {} logits for {n} frames",
            logits.len()
        );
        let logits = logits.to_vec();
        let (_, cache) = outputs["cache_out"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow!("stage one cache: {e}"))?;
        ensure!(
            cache.len() == self.cache_size,
            "stage one cache is {} long",
            cache.len()
        );
        self.cache = cache.to_vec();
        Ok(logits)
    }
}

impl StageOneModel for KwsStageOneV1 {
    fn info(&self) -> &StageOneInfo {
        &self.info
    }

    fn reset(&mut self) {
        self.frontend.reset();
        self.cache = vec![0.0; self.cache_size];
    }

    fn push(&mut self, samples: &[f32]) -> Result<Scores> {
        let feats = self.frontend.push(samples);
        let logits = self.forward(&feats)?;
        let k = self.info.keywords.len();
        let frames = logits.len() / (2 * k);
        let rf = self.info.receptive_field as f64;
        let mut scores = Scores {
            frames,
            probs: Vec::with_capacity(frames * k),
            offsets: Vec::with_capacity(frames * k),
        };
        for row in logits.chunks_exact(2 * k) {
            for &l in &row[..k] {
                scores.probs.push(1.0 / (1.0 + (-(l as f64)).exp()));
            }
            for &l in &row[k..] {
                scores.offsets.push(l as f64 * rf);
            }
        }
        Ok(scores)
    }
}

//! 导入层：bundle 的 json 里 `format` 字段选 adapter。
//!
//! 一级出逐帧分数与起点偏移，二级给一个触发打分；检测、留音频、画图都在
//! 调用方（[`super::engine`]、界面），所以新的模型族只要在 [`ADAPTERS`] 里加一个
//! adapter，界面不用改。

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

use super::{stage_one::KwsStageOneV1, stage_two::KwsStageTwoV2};

/// 一级的事实：帧网格、关键词、感受野。
#[derive(Clone, Debug)]
pub struct StageOneInfo {
    pub format: String,
    pub root: PathBuf,
    pub sample_rate: u32,
    pub n_fft: usize,
    pub hop: usize,
    pub keywords: Vec<String>,
    /// 最后一个输出看得见的帧数；偏移头的单位。
    pub receptive_field: usize,
}

impl StageOneInfo {
    /// 第 `frame` 帧最后一个样本之后：`frame * hop + n_fft`。
    pub fn frame_end(&self, frame: usize) -> usize {
        frame * self.hop + self.n_fft
    }
}

/// 一次推送新出的帧：每帧每个关键词一个概率、一个起点偏移（帧），按帧摊平。
#[derive(Clone, Debug, Default)]
pub struct Scores {
    pub frames: usize,
    pub probs: Vec<f64>,
    pub offsets: Vec<f64>,
}

pub trait StageOneModel: Send {
    fn info(&self) -> &StageOneInfo;
    /// 回到开机：清前端、缓存。
    fn reset(&mut self);
    /// [-1, 1] 的样本，任意个数；它们凑齐的帧的分数。
    fn push(&mut self, samples: &[f32]) -> Result<Scores>;
}

/// 读开机以来 `[start, end)` 的样本。
pub type AudioReader<'a> = &'a dyn Fn(usize, usize) -> Vec<f32>;

#[derive(Clone, Debug)]
pub struct StageTwoInfo {
    pub format: String,
    pub root: PathBuf,
    pub sample_rate: u32,
    pub keywords: Vec<String>,
    pub members: usize,
    /// 在起点（或判定）之前最多读多少样本：一级要多留这么多历史。
    pub reach: usize,
}

pub trait StageTwoModel: Send {
    fn info(&self) -> &StageTwoInfo;
    /// 起点 `start`、终点 `end` 的触发，二级最早读到哪个样本（画导程用）。
    fn read_from(&self, start: usize, end: usize) -> usize;
    /// 触发在关键词上的分，阈值就在这个分上。
    fn logit(&mut self, keyword: &str, start: usize, end: usize, read: AudioReader) -> Result<f64>;
}

pub enum Model {
    StageOne(Box<dyn StageOneModel>),
    StageTwo(Box<dyn StageTwoModel>),
}

pub struct Adapter {
    pub format: &'static str,
    /// bundle json 的文件名。
    pub meta_file: &'static str,
    pub load: fn(&Path, &Value) -> Result<Model>,
}

pub const ADAPTERS: &[Adapter] = &[
    Adapter {
        format: KwsStageOneV1::FORMAT,
        meta_file: KwsStageOneV1::META_FILE,
        load: |root, meta| Ok(Model::StageOne(Box::new(KwsStageOneV1::load(root, meta)?))),
    },
    Adapter {
        format: KwsStageTwoV2::FORMAT,
        meta_file: KwsStageTwoV2::META_FILE,
        load: |root, meta| Ok(Model::StageTwo(Box::new(KwsStageTwoV2::load(root, meta)?))),
    },
];

/// 拖进来的目录或 zip 里认出来的每个 bundle。zip 解到 `scratch` 下面。
pub fn import(path: &Path, scratch: &Path) -> Result<Vec<Model>> {
    let root = if path.is_dir() {
        path.to_path_buf()
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        unzip(path, scratch)?
    } else if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        // 直接拖 json：它所在的目录
        path.parent()
            .context("json has no directory")?
            .to_path_buf()
    } else {
        bail!(
            "{}: drop a bundle directory, a .zip of one, or its json",
            path.display()
        );
    };
    let mut found = vec![];
    find_bundles(&root, 0, &mut found)?;
    ensure!(
        !found.is_empty(),
        "{}: no bundle json with a known format ({}) within two levels",
        path.display(),
        ADAPTERS
            .iter()
            .map(|a| a.format)
            .collect::<Vec<_>>()
            .join(", ")
    );
    found
        .into_iter()
        .map(|(dir, adapter, meta)| {
            (adapter.load)(&dir, &meta)
                .with_context(|| format!("{} ({})", dir.display(), adapter.format))
        })
        .collect()
}

fn find_bundles(
    dir: &Path,
    depth: usize,
    out: &mut Vec<(PathBuf, &'static Adapter, Value)>,
) -> Result<()> {
    for adapter in ADAPTERS {
        let file = dir.join(adapter.meta_file);
        if !file.is_file() {
            continue;
        }
        let meta: Value = serde_json::from_slice(&std::fs::read(&file)?)
            .with_context(|| file.display().to_string())?;
        if meta.get("format").and_then(Value::as_str) == Some(adapter.format) {
            out.push((dir.to_path_buf(), adapter, meta));
        } else {
            bail!(
                "{}: format {} is not one kwscope reads",
                file.display(),
                meta.get("format").unwrap_or(&Value::Null)
            );
        }
    }
    if depth >= 2 {
        return Ok(());
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    subdirs.sort();
    for sub in subdirs {
        find_bundles(&sub, depth + 1, out)?;
    }
    Ok(())
}

fn unzip(path: &Path, scratch: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .with_context(|| format!("{} is not a zip", path.display()))?;
    let stem = path
        .file_stem()
        .map_or("bundle".into(), |s| s.to_string_lossy());
    let mut dest = scratch.join(&*stem);
    let mut n = 1;
    while dest.exists() {
        dest = scratch.join(format!("{stem}-{n}"));
        n += 1;
    }
    std::fs::create_dir_all(&dest)?;
    zip.extract(&dest)
        .with_context(|| format!("unzip {}", path.display()))?;
    Ok(dest)
}

/// json 字段的小帮手。
pub(crate) fn field<'a>(meta: &'a Value, key: &str) -> Result<&'a Value> {
    meta.get(key)
        .with_context(|| format!("bundle json has no {key}"))
}

pub(crate) fn uint(meta: &Value, key: &str) -> Result<usize> {
    field(meta, key)?
        .as_u64()
        .map(|v| v as usize)
        .with_context(|| format!("{key} must be a non-negative integer"))
}

pub(crate) fn float(meta: &Value, key: &str) -> Result<f64> {
    field(meta, key)?
        .as_f64()
        .with_context(|| format!("{key} must be a number"))
}

pub(crate) fn strings(meta: &Value, key: &str) -> Result<Vec<String>> {
    field(meta, key)?
        .as_array()
        .with_context(|| format!("{key} must be a list"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .with_context(|| format!("{key} must hold strings"))
        })
        .collect()
}

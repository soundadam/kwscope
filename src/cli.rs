//! `kwscope stream`：不开窗口，按 `kws stream` 的样子把触发一行一个写到 stdout。

use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use kwscope::kws::adapter::{self, Model};
use kwscope::kws::detector::DetectorConfig;
use kwscope::kws::engine::{self, Engine, Judge};
use kwscope::kws::{jsonl, media};
use std::io::Write;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct Stream {
    /// wav or flac files; each is its own stream from boot
    #[arg(required = true)]
    pub wavs: Vec<PathBuf>,
    /// Stage-one bundle: a folder, a .zip or its json
    #[arg(long)]
    pub bundle: PathBuf,
    /// Stage one's threshold
    #[arg(long, default_value_t = crate::app::DEFAULT_T1)]
    pub threshold: f64,
    /// Stage-two bundle that checks each trigger
    #[arg(long)]
    pub stage_two: Option<PathBuf>,
    /// Stage two's threshold on its logit
    #[arg(long, default_value_t = crate::app::DEFAULT_T2)]
    pub t2: f64,
    /// Channel of a multichannel file; each line then names it
    #[arg(long)]
    pub channel: Option<usize>,
}

pub fn stream(args: Stream) -> Result<()> {
    let scratch = std::env::temp_dir().join("kwscope-import");
    let mut one = None;
    let mut two = None;
    for (path, want_one) in [(Some(&args.bundle), true), (args.stage_two.as_ref(), false)] {
        let Some(path) = path else { continue };
        for m in adapter::import(path, &scratch)? {
            match (m, want_one) {
                (Model::StageOne(m), true) => one = Some(m),
                (Model::StageTwo(m), false) => two = Some(m),
                _ => {}
            }
        }
    }
    let Some(one) = one else {
        bail!("{}: no stage-one bundle", args.bundle.display())
    };
    if let (Some(p), None) = (&args.stage_two, &two) {
        bail!("{}: no stage-two bundle", p.display());
    }
    let rate = one.info().sample_rate;
    let judge = two.map(|m| Judge::new(m, args.t2));
    let mut engine = Engine::new(one, judge, DetectorConfig::det_v8(args.threshold))?;
    let step = engine::step_samples(rate, engine::CHUNK_MS);
    let mut out = std::io::stdout().lock();
    for wav in &args.wavs {
        let audio = media::decode(wav)?;
        let n = audio.channels.len();
        let index = match args.channel {
            None if n != 1 => bail!("{}: {n} channels; name one with --channel", wav.display()),
            Some(c) if c >= n => bail!("{}: no channel {c} in {n}", wav.display()),
            c => c.unwrap_or(0),
        };
        ensure!(audio.sample_rate > 0, "{}: no sample rate", wav.display());
        // `kws stream` 要求采样率一样；这里改成重采样过去
        let samples = media::resample(&audio.channels[index], audio.sample_rate, rate);
        engine.reset();
        let name = wav.display().to_string();
        for t in engine.run(&samples, step).with_context(|| name.clone())? {
            writeln!(out, "{}", jsonl::line(&name, &t, args.channel))?;
        }
    }
    Ok(())
}

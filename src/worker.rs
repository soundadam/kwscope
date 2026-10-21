//! 模型住在这条线程上：导入、直播推送、离线文件都在这里跑，界面只收结果。
//!
//! 每次重开直播或换文件，界面先把 `epoch` 加一再发命令；线程跑长活时看它，
//! 变了就放弃，回的消息也带着它，界面丢掉过时的。

use crate::spectrogram::Spectrogram;
use kwscope::kws::adapter::{self, Model, StageOneInfo, StageOneModel, StageTwoInfo};
use kwscope::kws::detector::DetectorConfig;
use kwscope::kws::engine::{self, Analysis, Engine, Judge, Trigger};
use kwscope::kws::media;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

pub enum Cmd {
    Import(PathBuf),
    Unload {
        stage_one: bool,
        stage_two: bool,
    },
    /// 从开机重来一条直播流。
    Live {
        epoch: u64,
    },
    /// 模型采样率上的麦克风样本。
    Samples {
        epoch: u64,
        samples: Vec<f32>,
    },
    Open {
        epoch: u64,
        path: PathBuf,
        channel: usize,
    },
    Thresholds {
        t1: f64,
        t2: f64,
    },
}

pub struct FileView {
    pub channels: usize,
    pub channel: usize,
    pub source_rate: u32,
    pub samples: usize,
    pub spec: Spectrogram,
    /// 一级的分，按帧摊平，每帧每个关键词一个；没有一级时为空。
    pub probs: Vec<f32>,
    pub triggers: Vec<Trigger>,
}

pub enum Reply {
    Models {
        one: Option<StageOneInfo>,
        two: Option<StageTwoInfo>,
    },
    Error(String),
    /// 一次直播推送：新帧的分与判下来的触发。
    Scores {
        epoch: u64,
        probs: Vec<f32>,
        triggers: Vec<Trigger>,
        seconds: f64,
    },
    Progress {
        epoch: u64,
        done: f32,
    },
    File {
        epoch: u64,
        file: Box<FileView>,
    },
    /// 离线文件改阈值后重放出的全部触发。
    Triggers {
        epoch: u64,
        triggers: Vec<Trigger>,
    },
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Reply>,
    pub epoch: Arc<AtomicU64>,
}

impl Handle {
    pub fn send(&self, cmd: Cmd) {
        // 线程只在进程退出时停
        let _ = self.tx.send(cmd);
    }

    /// 开一个新纪元，旧纪元的长活与回信作废。
    pub fn bump(&self) -> u64 {
        self.epoch.fetch_add(1, Relaxed) + 1
    }
}

pub fn spawn(wake: impl Fn() + Send + 'static, t1: f64, t2: f64) -> Handle {
    let (tx, cmds) = channel();
    let (replies, rx) = channel();
    let epoch = Arc::new(AtomicU64::new(0));
    let mut worker = Worker {
        out: replies,
        wake: Box::new(wake),
        epoch: epoch.clone(),
        stage_one: None,
        judge: None,
        engine: None,
        live_epoch: None,
        pending: vec![],
        file: None,
        t1,
        t2,
    };
    std::thread::Builder::new()
        .name("kws".into())
        .spawn(move || {
            while let Ok(cmd) = cmds.recv() {
                worker.handle(cmd);
            }
        })
        .expect("spawn the model thread");
    Handle { tx, rx, epoch }
}

struct OpenFile {
    epoch: u64,
    samples: Vec<f32>,
    analysis: Option<Analysis>,
}

struct Worker {
    out: Sender<Reply>,
    wake: Box<dyn Fn() + Send>,
    epoch: Arc<AtomicU64>,
    /// 直播时两级借给 `engine`，其余时候放在这里。
    stage_one: Option<Box<dyn StageOneModel>>,
    judge: Option<Judge>,
    engine: Option<Engine>,
    live_epoch: Option<u64>,
    /// 凑够一次推送（`kws stream` 的 100 ms）之前的样本。
    pending: Vec<f32>,
    file: Option<OpenFile>,
    t1: f64,
    t2: f64,
}

impl Worker {
    fn reply(&self, r: Reply) {
        let _ = self.out.send(r);
        (self.wake)();
    }

    fn fail(&self, e: impl std::fmt::Display) {
        self.reply(Reply::Error(e.to_string()));
    }

    fn config(&self) -> DetectorConfig {
        DetectorConfig::det_v8(self.t1)
    }

    /// 从直播引擎里收回两级。
    fn reclaim(&mut self) {
        if let Some(engine) = self.engine.take() {
            let (one, judge) = engine.into_parts();
            self.stage_one = Some(one);
            self.judge = judge;
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Import(path) => self.import(path),
            Cmd::Unload {
                stage_one,
                stage_two,
            } => {
                self.reclaim();
                if stage_one {
                    self.stage_one = None;
                }
                if stage_two {
                    self.judge = None;
                }
                self.models();
            }
            Cmd::Live { epoch } => {
                self.file = None;
                self.reclaim();
                self.live_epoch = Some(epoch);
                self.pending.clear();
                if let Some(one) = self.stage_one.take() {
                    match Engine::new(one, self.judge.take(), self.config()) {
                        Ok(mut engine) => {
                            engine.reset();
                            self.engine = Some(engine);
                        }
                        // 导入时已经查过能不能接上，这里不该走到
                        Err(e) => self.fail(e),
                    }
                }
            }
            Cmd::Samples { epoch, samples } => {
                if self.live_epoch == Some(epoch) {
                    self.live(samples, epoch);
                }
            }
            Cmd::Open {
                epoch,
                path,
                channel,
            } => {
                self.reclaim();
                self.live_epoch = None;
                self.open(epoch, path, channel);
            }
            Cmd::Thresholds { t1, t2 } => {
                self.t1 = t1;
                self.t2 = t2;
                let config = self.config();
                if let Some(engine) = &mut self.engine {
                    engine.detection.set_config(config);
                    if let Some(j) = &mut engine.judge {
                        j.threshold = t2;
                    }
                }
                if let Some(j) = &mut self.judge {
                    j.threshold = t2;
                }
                if self.file.as_ref().is_some_and(|f| f.analysis.is_some()) {
                    self.redetect();
                }
            }
        }
    }

    fn models(&self) {
        let (one, two) = match &self.engine {
            Some(e) => (
                Some(e.info().clone()),
                e.judge.as_ref().map(|j| j.model.info().clone()),
            ),
            None => (
                self.stage_one.as_ref().map(|m| m.info().clone()),
                self.judge.as_ref().map(|j| j.model.info().clone()),
            ),
        };
        self.reply(Reply::Models { one, two });
    }

    fn import(&mut self, path: PathBuf) {
        let scratch = std::env::temp_dir().join("kwscope-import");
        let models = match adapter::import(&path, &scratch) {
            Ok(m) => m,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        self.reclaim();
        let mut one = None;
        let mut two = None;
        for m in models {
            match m {
                Model::StageOne(m) => one = Some(m),
                Model::StageTwo(m) => two = Some(m),
            }
        }
        // 新来的和留着的那一级要接得上，接不上就整次不收
        let one_info = one
            .as_ref()
            .or(self.stage_one.as_ref())
            .map(|m| m.info().clone());
        let two_ref = two
            .as_deref()
            .or(self.judge.as_ref().map(|j| j.model.as_ref()));
        if let (Some(info), Some(t)) = (&one_info, two_ref) {
            if let Err(e) = engine::check_pair(info, t) {
                self.models();
                return self.fail(format!("{}: {e}", path.display()));
            }
        }
        if let Some(m) = one {
            self.stage_one = Some(m);
        }
        if let Some(m) = two {
            self.judge = Some(Judge::new(m, self.t2));
        }
        self.models();
    }

    fn live(&mut self, samples: Vec<f32>, epoch: u64) {
        let Some(engine) = &mut self.engine else {
            return;
        };
        let step = engine::step_samples(engine.info().sample_rate, engine::CHUNK_MS);
        self.pending.extend(samples);
        let mut probs = vec![];
        let mut triggers = vec![];
        let began = Instant::now();
        let mut at = 0;
        let mut error = None;
        while self.pending.len() - at >= step {
            match engine.push(&self.pending[at..at + step]) {
                Ok(p) => {
                    probs.extend(p.scores.probs.iter().map(|&v| v as f32));
                    triggers.extend(p.triggers);
                }
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
            at += step;
        }
        self.pending.drain(..at);
        // 直播不重放：记住的分只会越攒越多
        if let Some(j) = &mut engine.judge {
            j.cache.clear();
        }
        if at > 0 {
            let seconds = began.elapsed().as_secs_f64();
            self.reply(Reply::Scores {
                epoch,
                probs,
                triggers,
                seconds,
            });
        }
        if let Some(e) = error {
            self.fail(format!("{e:#}"));
        }
    }

    fn open(&mut self, epoch: u64, path: PathBuf, channel: usize) {
        self.file = None;
        // 记住的分按样本位置索引，只对同一段音频成立
        if let Some(j) = &mut self.judge {
            j.cache.clear();
        }
        let current = || self.epoch.load(Relaxed) == epoch;
        let audio = match media::decode(&path) {
            Ok(a) => a,
            Err(e) => return self.fail(format!("{e:#}")),
        };
        let channels = audio.channels.len();
        let channel = channel.min(channels - 1);
        let source_rate = audio.sample_rate;
        let rate = self
            .stage_one
            .as_ref()
            .map_or(16_000, |m| m.info().sample_rate);
        let samples = media::resample(&audio.channels[channel], source_rate, rate);
        drop(audio);
        let mut spec = Spectrogram::new(rate);
        spec.push(&samples);
        let mut analysis = None;
        if let Some(one) = &mut self.stage_one {
            let step = engine::step_samples(rate, engine::CHUNK_MS);
            let total = samples.len().max(1);
            let (out, wake) = (&self.out, &self.wake);
            let mut shown = 0.0;
            let run = Analysis::run(one.as_mut(), &samples, step, |done| {
                let done = (done as f32 / total as f32).min(1.0);
                if done - shown >= 0.01 {
                    shown = done;
                    let _ = out.send(Reply::Progress { epoch, done });
                    wake();
                }
                current()
            });
            match run {
                Ok(Some(a)) => analysis = Some(a),
                Ok(None) => return,
                Err(e) => return self.fail(format!("{e:#}")),
            }
        }
        let triggers = match &analysis {
            Some(a) => match a.detect(self.config(), &samples, self.judge.as_mut()) {
                Ok(t) => t,
                Err(e) => return self.fail(format!("{e:#}")),
            },
            None => vec![],
        };
        let probs = analysis
            .as_ref()
            .map(|a| a.scores.probs.iter().map(|&v| v as f32).collect())
            .unwrap_or_default();
        let file = FileView {
            channels,
            channel,
            source_rate,
            samples: samples.len(),
            spec,
            probs,
            triggers,
        };
        self.file = Some(OpenFile {
            epoch,
            samples,
            analysis,
        });
        self.reply(Reply::File {
            epoch,
            file: Box::new(file),
        });
    }

    fn redetect(&mut self) {
        let config = self.config();
        let Some(f) = &self.file else { return };
        let Some(a) = &f.analysis else { return };
        match a.detect(config, &f.samples, self.judge.as_mut()) {
            Ok(triggers) => self.reply(Reply::Triggers {
                epoch: f.epoch,
                triggers,
            }),
            Err(e) => self.fail(format!("{e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::Path;
    use std::time::Duration;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    fn want() -> Vec<(u64, bool)> {
        std::fs::read_to_string(fixtures().join("triggers_t1_0.553.jsonl"))
            .unwrap()
            .lines()
            .map(|l| {
                let v: Value = serde_json::from_str(l).unwrap();
                (
                    v["decision_frame"].as_u64().unwrap(),
                    v["accepted"].as_bool().unwrap(),
                )
            })
            .collect()
    }

    fn next(h: &Handle) -> Reply {
        match h
            .rx
            .recv_timeout(Duration::from_secs(60))
            .expect("worker replies")
        {
            Reply::Error(e) => panic!("{e}"),
            r => r,
        }
    }

    fn loaded(h: &Handle) -> StageOneInfo {
        let mut info = None;
        for dir in ["stage_one", "stage_two"] {
            h.send(Cmd::Import(fixtures().join("bundles").join(dir)));
            let Reply::Models { one, .. } = next(h) else {
                panic!("expected models")
            };
            info = one;
        }
        info.unwrap()
    }

    #[test]
    fn live_pushes_in_any_chunking_give_kws_stream_triggers() {
        let h = spawn(|| {}, 0.553, 0.455);
        let info = loaded(&h);
        let epoch = h.bump();
        h.send(Cmd::Live { epoch });
        let samples = media::decode(&fixtures().join("thchs30_clip.wav"))
            .unwrap()
            .channels
            .remove(0);
        // 麦克风回调的块和 100 ms 的推送对不齐
        for chunk in samples.chunks(777) {
            h.send(Cmd::Samples {
                epoch,
                samples: chunk.to_vec(),
            });
        }
        // 推满的那些 100 ms 里凑齐的帧
        let pushed = samples.len() / 1600 * 1600;
        let expect = (pushed - info.n_fft) / info.hop + 1;
        let (mut frames, mut got) = (0, vec![]);
        while frames < expect {
            if let Reply::Scores {
                probs, triggers, ..
            } = next(&h)
            {
                frames += probs.len();
                got.extend(
                    triggers
                        .iter()
                        .map(|t| (t.decision_frame as u64, t.accepted)),
                );
            }
        }
        // 直播不 flush：文件末尾还开着的窗不判
        let want = want();
        assert!(got.len() >= want.len() - 1, "{got:?}");
        assert_eq!(got[..], want[..got.len()]);
    }

    #[test]
    fn opening_a_file_replays_with_new_thresholds() {
        let h = spawn(|| {}, 0.553, 0.455);
        loaded(&h);
        let epoch = h.bump();
        h.send(Cmd::Open {
            epoch,
            path: fixtures().join("thchs30_clip.wav"),
            channel: 0,
        });
        let file = loop {
            if let Reply::File { file, .. } = next(&h) {
                break file;
            }
        };
        let got: Vec<_> = file
            .triggers
            .iter()
            .map(|t| (t.decision_frame as u64, t.accepted))
            .collect();
        assert_eq!(got, want());
        assert!(file.triggers.iter().all(|t| t.stage_two_seconds.is_some()));

        // t2 抬到最高：全被拒；拒绝不锁定，会多出新触发，已判过的那些分从缓存取
        h.send(Cmd::Thresholds { t1: 0.553, t2: 1e9 });
        let Reply::Triggers { triggers, .. } = next(&h) else {
            panic!("expected triggers")
        };
        assert!(triggers.len() >= got.len());
        assert!(triggers.iter().all(|t| !t.accepted));
        let seen: Vec<_> = got.iter().map(|&(frame, _)| frame).collect();
        assert!(triggers
            .iter()
            .filter(|t| seen.contains(&(t.decision_frame as u64)))
            .all(|t| t.stage_two_seconds.is_none()));
    }

    fn key(triggers: &[Trigger]) -> Vec<(usize, bool, usize)> {
        triggers
            .iter()
            .map(|t| (t.decision_frame, t.accepted, t.end_sample))
            .collect()
    }

    fn file(h: &Handle, epoch: u64) -> Vec<Trigger> {
        loop {
            match next(h) {
                Reply::File { epoch: e, file } if e == epoch => break file.triggers,
                _ => {}
            }
        }
    }

    /// 默认阈值下，界面打开文件、改阈值再改回、重导一级、再开同一个文件，触发都和
    /// `kwscope stream`（`Engine::run`）的一样，一份也不会叠上去。
    #[test]
    fn replaying_a_file_gives_kwscope_stream_triggers() {
        use crate::app::{DEFAULT_T1, DEFAULT_T2};
        let scratch = std::env::temp_dir().join("kwscope-import");
        let mut models = adapter::import(&fixtures().join("bundles/stage_one"), &scratch).unwrap();
        models.extend(adapter::import(&fixtures().join("bundles/stage_two"), &scratch).unwrap());
        let (mut one, mut two) = (None, None);
        for m in models {
            match m {
                Model::StageOne(m) => one = Some(m),
                Model::StageTwo(m) => two = Some(m),
            }
        }
        let one = one.unwrap();
        let rate = one.info().sample_rate;
        let mut engine = Engine::new(
            one,
            Some(Judge::new(two.unwrap(), DEFAULT_T2)),
            DetectorConfig::det_v8(DEFAULT_T1),
        )
        .unwrap();
        let clip = fixtures().join("thchs30_clip.wav");
        let samples = media::decode(&clip).unwrap().channels.remove(0);
        let want = key(&engine
            .run(&samples, engine::step_samples(rate, engine::CHUNK_MS))
            .unwrap());
        // 随机初始化的二级在 t2 = 1.2 下全拒，拒绝不锁定：比只有一级时多
        assert!(!want.is_empty() && want.iter().all(|&(_, accepted, _)| !accepted));

        let h = spawn(|| {}, DEFAULT_T1, DEFAULT_T2);
        loaded(&h);
        let open = |h: &Handle| {
            let epoch = h.bump();
            h.send(Cmd::Open {
                epoch,
                path: clip.clone(),
                channel: 0,
            });
            file(h, epoch)
        };
        assert_eq!(key(&open(&h)), want);

        h.send(Cmd::Thresholds {
            t1: 0.553,
            t2: DEFAULT_T2,
        });
        let Reply::Triggers { .. } = next(&h) else {
            panic!("expected triggers")
        };
        h.send(Cmd::Thresholds {
            t1: DEFAULT_T1,
            t2: DEFAULT_T2,
        });
        let Reply::Triggers { triggers, .. } = next(&h) else {
            panic!("expected triggers")
        };
        assert_eq!(key(&triggers), want);

        h.send(Cmd::Import(fixtures().join("bundles/stage_one")));
        let Reply::Models { .. } = next(&h) else {
            panic!("expected models")
        };
        assert_eq!(key(&open(&h)), want);
        assert_eq!(key(&open(&h)), want);
    }
}

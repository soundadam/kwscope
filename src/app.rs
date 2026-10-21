use crate::audio::{self, Input};
use crate::axis::Scale;
use crate::colormap::Colormap;
use crate::spectrogram::{Layout, Resolution, Spectrogram, MAX_ROWS};
use crate::strip::{self, Strip};
use crate::worker::{self, Cmd, FileView, Handle, Reply};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Slider,
    Stroke,
};
use kwscope::kws::adapter::{StageOneInfo, StageTwoInfo};
use kwscope::kws::detector::DetectorConfig;
use kwscope::kws::engine::Trigger;
use kwscope::kws::{jsonl, media::Resampler};
use std::path::PathBuf;
use std::time::Duration;

/// kws-train 台账上一级 12 FA/h 的阈值与二级的现役阈值。
pub const DEFAULT_T1: f64 = 0.33140625;
pub const DEFAULT_T2: f64 = 1.2;
/// 直播留多久的图、分与触发。
const LIVE_KEEP_S: f64 = 300.0;
const COPYRIGHT_YEAR: u32 = 2026;
/// 直播时的重画间隔：麦克风环形缓冲按这个节奏取。
const LIVE_REPAINT: Duration = Duration::from_millis(33);
const MAX_TEXTURE_W: usize = 4096;

const BG: Color32 = Color32::from_gray(12);
const AMBER: Color32 = Color32::from_rgb(255, 214, 102);
const CYAN: Color32 = Color32::from_rgb(120, 200, 255);
const GREEN: Color32 = Color32::from_rgb(90, 230, 120);
const RED: Color32 = Color32::from_rgb(255, 90, 90);
const SCORE_H: f32 = 140.0;
const AXIS_H: f32 = 20.0;
const HZ_GUTTER: f32 = 44.0;
const TIME_STEPS: [f64; 12] = [
    0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
];

/// 一条时间轴上的一切：语谱、一级的分、触发。直播与离线共用。
struct Timeline {
    sample_rate: u32,
    spec: Spectrogram,
    /// 开机以来的样本数。
    samples: usize,
    keywords: Vec<String>,
    hop: usize,
    n_fft: usize,
    probs: Vec<f32>,
    /// `probs` 第一帧是开机以来第几帧。
    first_frame: usize,
    triggers: Vec<Trigger>,
    /// 直播暂停又继续处的样本号：停着的那段剪掉了，前后拼在一起。
    splices: Vec<usize>,
}

impl Timeline {
    fn new(sample_rate: u32, one: Option<&StageOneInfo>) -> Self {
        Self {
            sample_rate,
            spec: Spectrogram::new(sample_rate),
            samples: 0,
            keywords: one.map(|i| i.keywords.clone()).unwrap_or_default(),
            hop: one.map_or(160, |i| i.hop),
            n_fft: one.map_or(400, |i| i.n_fft),
            probs: vec![],
            first_frame: 0,
            triggers: vec![],
            splices: vec![],
        }
    }

    fn from_file(file: FileView, one: Option<&StageOneInfo>) -> Self {
        let mut t = Self::new(file.spec.sample_rate, one);
        t.samples = file.samples;
        t.spec = file.spec;
        t.probs = file.probs;
        t.triggers = file.triggers;
        t
    }

    fn duration(&self) -> f64 {
        self.samples as f64 / self.sample_rate as f64
    }

    fn seconds(&self, sample: usize) -> f64 {
        sample as f64 / self.sample_rate as f64
    }

    fn k(&self) -> usize {
        self.keywords.len().max(1)
    }

    fn frame_end(&self) -> usize {
        self.first_frame + self.probs.len() / self.k()
    }

    /// 第 `frame` 帧的分在它最后一个样本处才有：画在那里，和触发的终点对齐。
    fn frame_time(&self, frame: usize) -> f64 {
        self.seconds(frame * self.hop + self.n_fft)
    }

    fn prob(&self, frame: usize, index: usize) -> Option<f32> {
        let j = frame.checked_sub(self.first_frame)?;
        self.probs.get(j * self.k() + index).copied()
    }

    fn trim(&mut self) {
        let keep_cols = (LIVE_KEEP_S / crate::spectrogram::HOP_S) as usize;
        self.spec.trim(keep_cols);
        let k = self.k();
        let keep_frames = (LIVE_KEEP_S * self.sample_rate as f64 / self.hop as f64) as usize;
        let have = self.probs.len() / k;
        if have > keep_frames + keep_frames / 4 {
            let drop = have - keep_frames;
            self.probs.drain(..drop * k);
            self.first_frame += drop;
        }
        let horizon = self
            .samples
            .saturating_sub((LIVE_KEEP_S * self.sample_rate as f64) as usize);
        self.triggers.retain(|t| t.end_sample >= horizon);
        self.splices.retain(|&s| s >= horizon);
    }
}

/// 看哪一段：`end` 秒往前 `span` 秒。直播跟着最新样本走，拖动后停住（麦克风照录）。
#[derive(Clone, Copy)]
struct View {
    end: f64,
    span: f64,
    follow: bool,
}

enum Source {
    /// 暂停时麦克风关着，继续时接着同一条流（检测器的历史不清）。
    Live { paused: bool },
    File {
        path: PathBuf,
        channel: usize,
        channels: usize,
        source_rate: u32,
        /// 还在跑一级时的进度。
        progress: Option<f32>,
    },
}

#[derive(Default)]
struct Timing {
    stage_two_n: usize,
    stage_two_sum: f64,
    stage_two_max: f64,
    /// 最近一次直播推送：一级加二级花的秒数、推了多少秒的音频。
    push: Option<(f64, f64)>,
}

impl Timing {
    fn add(&mut self, triggers: &[Trigger]) {
        for s in triggers.iter().filter_map(|t| t.stage_two_seconds) {
            self.stage_two_n += 1;
            self.stage_two_sum += s;
            self.stage_two_max = self.stage_two_max.max(s);
        }
    }
}

pub struct App {
    worker: Handle,
    epoch: u64,
    one: Option<StageOneInfo>,
    two: Option<StageTwoInfo>,
    error: Option<String>,
    busy: bool,

    devices: Vec<String>,
    /// 选中的设备；`None` 是系统默认。只记名字，直播在听时才开。
    device: Option<String>,
    /// 只在直播在听时有：持有即录音。
    input: Option<Input>,
    resampler: Option<Resampler>,
    popped: Vec<f32>,

    source: Source,
    timeline: Timeline,
    view: View,
    timing: Timing,
    t1: f64,
    t2: f64,

    db_lo: f32,
    db_hi: f32,
    colormap: Colormap,
    lut: [Color32; 256],
    scale: Scale,
    resolution: Resolution,
    strip: Strip,
    selected: Option<usize>,
    /// 这条直播流里已经关掉的设备丢了多少样本。
    dropped: u64,

    updater: appkit::ui::Updater,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        identity: appkit::App,
        bundles: Vec<PathBuf>,
        audio: Option<PathBuf>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let worker = worker::spawn(move || ctx.request_repaint(), DEFAULT_T1, DEFAULT_T2);
        let colormap = Colormap::Magma;
        let mut app = Self {
            worker,
            epoch: 0,
            one: None,
            two: None,
            error: None,
            busy: false,
            devices: audio::input_device_names(),
            device: None,
            input: None,
            resampler: None,
            popped: vec![],
            source: Source::Live { paused: false },
            timeline: Timeline::new(16_000, None),
            view: View {
                end: 0.0,
                span: 8.0,
                follow: true,
            },
            timing: Timing::default(),
            t1: DEFAULT_T1,
            t2: DEFAULT_T2,
            db_lo: -100.0,
            db_hi: -20.0,
            colormap,
            lut: colormap.lut(),
            scale: Scale::Mel,
            resolution: Resolution::Multi,
            strip: Strip::default(),
            selected: None,
            dropped: 0,
            updater: appkit::ui::Updater::new(identity, &cc.egui_ctx),
        };
        for path in bundles {
            app.import(path);
        }
        match audio {
            Some(path) => app.open_file(path, 0),
            None => app.start_live(true),
        }
        app
    }

    fn rate(&self) -> u32 {
        self.one.as_ref().map_or(16_000, |i| i.sample_rate)
    }

    fn import(&mut self, path: PathBuf) {
        self.busy = true;
        self.worker.send(Cmd::Import(path));
    }

    /// 从开机重来一条直播流；`listen` 为假时停在暂停上，不开麦克风。
    fn start_live(&mut self, listen: bool) {
        self.epoch = self.worker.bump();
        self.worker.send(Cmd::Live { epoch: self.epoch });
        self.source = Source::Live { paused: true };
        self.timeline = Timeline::new(self.rate(), self.one.as_ref());
        self.timing = Timing::default();
        self.selected = None;
        self.view.follow = true;
        self.view.span = self.view.span.min(30.0);
        // 重开设备：缓冲里的旧样本不是这条流的
        self.close_input();
        self.dropped = 0;
        if listen {
            self.resume();
        }
    }

    /// 关麦克风，直播停在当前时刻。
    fn pause(&mut self) {
        self.close_input();
        if let Source::Live { paused } = &mut self.source {
            *paused = true;
        }
    }

    /// 开麦克风接着这条流；停着的那段不进时间轴，记一个拼接点。
    fn resume(&mut self) {
        if !self.open_input() {
            return;
        }
        if let Source::Live { paused } = &mut self.source {
            *paused = false;
        }
        if self.timeline.samples > 0 {
            self.timeline.splices.push(self.timeline.samples);
        }
    }

    fn open_input(&mut self) -> bool {
        if self.input.is_some() {
            return true;
        }
        match audio::open(self.device.as_deref()) {
            Ok(input) => {
                self.device = Some(input.device_name.clone());
                self.resampler = Some(Resampler::new(input.sample_rate, self.rate()));
                self.input = Some(input);
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn close_input(&mut self) {
        if let Some(input) = self.input.take() {
            self.dropped += input.stats.dropped();
        }
        self.resampler = None;
    }

    fn open_file(&mut self, path: PathBuf, channel: usize) {
        self.close_input();
        self.epoch = self.worker.bump();
        self.worker.send(Cmd::Open {
            epoch: self.epoch,
            path: path.clone(),
            channel,
        });
        self.source = Source::File {
            path,
            channel,
            channels: 1,
            source_rate: 0,
            progress: Some(0.0),
        };
        self.timeline = Timeline::new(self.rate(), self.one.as_ref());
        self.timing = Timing::default();
        self.selected = None;
    }

    /// 换了模型：当前的流从头再来。
    fn restart(&mut self) {
        match &self.source {
            Source::Live { paused } => self.start_live(!paused),
            Source::File { path, channel, .. } => self.open_file(path.clone(), *channel),
        }
    }

    /// 选了设备就是要听它：从开机重来一条直播流。
    fn switch_device(&mut self, name: Option<String>) {
        self.device = name;
        self.start_live(true);
    }

    fn drain_replies(&mut self) {
        while let Ok(reply) = self.worker.rx.try_recv() {
            match reply {
                Reply::Models { one, two } => {
                    self.busy = false;
                    self.error = None;
                    self.one = one;
                    self.two = two;
                    self.restart();
                }
                Reply::Error(e) => {
                    self.busy = false;
                    self.error = Some(e);
                    if let Source::File { progress, .. } = &mut self.source {
                        *progress = None;
                    }
                }
                Reply::Scores {
                    epoch,
                    probs,
                    triggers,
                    seconds,
                } if epoch == self.epoch => {
                    let k = self.timeline.k();
                    let pushed = probs.len() / k * self.timeline.hop;
                    self.timing.push = Some((seconds, self.timeline.seconds(pushed)));
                    self.timing.add(&triggers);
                    self.timeline.probs.extend(probs);
                    self.timeline.triggers.extend(triggers);
                }
                Reply::Progress { epoch, done } if epoch == self.epoch => {
                    if let Source::File { progress, .. } = &mut self.source {
                        *progress = Some(done);
                    }
                }
                Reply::File { epoch, file } if epoch == self.epoch => {
                    if let Source::File {
                        channels,
                        channel,
                        source_rate,
                        progress,
                        ..
                    } = &mut self.source
                    {
                        *channels = file.channels;
                        *channel = file.channel;
                        *source_rate = file.source_rate;
                        *progress = None;
                    }
                    self.timing.add(&file.triggers);
                    self.timeline = Timeline::from_file(*file, self.one.as_ref());
                    let d = self.timeline.duration();
                    self.view = View {
                        span: d.clamp(1.0, 20.0),
                        end: d.clamp(1.0, 20.0),
                        follow: false,
                    };
                }
                Reply::Triggers { epoch, triggers } if epoch == self.epoch => {
                    self.timing.add(&triggers);
                    self.timeline.triggers = triggers;
                    self.selected = None;
                }
                _ => {}
            }
        }
    }

    fn pump_audio(&mut self) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        self.popped.clear();
        while let Ok(s) = input.consumer.pop() {
            self.popped.push(s);
        }
        if !matches!(self.source, Source::Live { paused: false }) || self.popped.is_empty() {
            return;
        }
        let Some(resampler) = &mut self.resampler else {
            return;
        };
        let samples = resampler.push(&self.popped);
        self.timeline.samples += samples.len();
        self.timeline.spec.push(&samples);
        if self.one.is_some() {
            self.worker.send(Cmd::Samples {
                epoch: self.epoch,
                samples,
            });
        }
        self.timeline.trim();
    }

    fn toggle_pause(&mut self) {
        match self.source {
            Source::Live { paused: true } => self.resume(),
            Source::Live { paused: false } => self.pause(),
            Source::File { .. } => {}
        }
    }

    fn set_thresholds(&mut self) {
        self.worker.send(Cmd::Thresholds {
            t1: self.t1,
            t2: self.t2,
        });
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let live = matches!(self.source, Source::Live { .. });
                let paused = matches!(self.source, Source::Live { paused: true });
                let mut chosen = None;
                egui::ComboBox::from_id_salt("device")
                    .width(220.0)
                    .selected_text(self.device.as_deref().unwrap_or("Default input"))
                    .show_ui(ui, |ui| {
                        for name in &self.devices {
                            if ui
                                .selectable_label(self.device.as_ref() == Some(name), name)
                                .clicked()
                            {
                                chosen = Some(name.clone());
                            }
                        }
                    });
                if ui.button("⟳").on_hover_text("Rescan devices").clicked() {
                    self.devices = audio::input_device_names();
                }
                if chosen.is_some() && (chosen != self.device || !live) {
                    self.switch_device(chosen);
                }
                if live {
                    let (label, tip) = if paused {
                        (
                            "▶ Resume",
                            "Space · Opens the microphone and continues this stream: \
                             the paused stretch is cut out and marked on the timeline, \
                             and the detector keeps its history",
                        )
                    } else {
                        (
                            "⏸ Pause",
                            "Space · Closes the microphone; nothing is recorded or scored \
                             until you resume",
                        )
                    };
                    if ui.button(label).on_hover_text(tip).clicked() {
                        self.toggle_pause();
                    }
                    if ui
                        .button("⟲ Restart")
                        .on_hover_text("Start the stream again from boot")
                        .clicked()
                    {
                        self.start_live(true);
                    }
                    if !self.view.follow
                        && ui
                            .button("⇥ Latest")
                            .on_hover_text(
                                "Scroll back to the newest audio and follow it (double-click)",
                            )
                            .clicked()
                    {
                        self.view.follow = true;
                    }
                } else if ui.button("🎤 Live").clicked() {
                    self.start_live(true);
                }
                if ui.button("📂 Open audio…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Audio", &["wav", "wave", "flac"])
                        .pick_file()
                    {
                        self.open_file(path, 0);
                    }
                }
                ui.separator();
                self.counts(ui);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.updater.top_bar(ui);
                    if let Some(e) = &self.error {
                        ui.label(RichText::new(e).color(Color32::LIGHT_RED));
                    } else if self.busy {
                        ui.spinner();
                        ui.label("Importing…");
                    } else if let Source::File {
                        progress: Some(p), ..
                    } = self.source
                    {
                        ui.add(
                            egui::ProgressBar::new(p)
                                .desired_width(160.0)
                                .show_percentage(),
                        );
                        ui.label("Stage one");
                    } else if paused {
                        ui.label(
                            RichText::new("PAUSED · mic off")
                                .color(Color32::LIGHT_RED)
                                .strong(),
                        );
                    }
                });
            });
        });
    }

    fn counts(&self, ui: &mut egui::Ui) {
        let t = &self.timeline.triggers;
        let accepted = t.iter().filter(|t| t.accepted).count();
        ui.label(
            RichText::new(format!("Triggers {}", t.len()))
                .monospace()
                .size(16.0)
                .color(AMBER),
        );
        if self.two.is_some() {
            ui.label(
                RichText::new(format!("Accepted {accepted}"))
                    .monospace()
                    .size(16.0)
                    .color(GREEN),
            );
            ui.label(
                RichText::new(format!("Rejected {}", t.len() - accepted))
                    .monospace()
                    .size(16.0)
                    .color(RED),
            );
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("settings")
            .resizable(false)
            .exact_width(270.0)
            .show(ctx, |ui| {
                egui::TopBottomPanel::bottom("credit")
                    .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 6.0)))
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.hyperlink_to(
                                format!("© {COPYRIGHT_YEAR} {}", env!("CARGO_PKG_AUTHORS")),
                                env!("CARGO_PKG_REPOSITORY"),
                            );
                            ui.label(
                                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                                    .small()
                                    .weak(),
                            );
                        });
                    });
                egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui));
            });
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Bundles");
        ui.horizontal(|ui| {
            if ui.button("Import folder…").clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.import(dir);
                }
            }
            if ui.button("Import zip…").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("Bundle", &["zip", "json"])
                    .pick_file()
                {
                    self.import(file);
                }
            }
        });
        ui.label(
            RichText::new("or drop a folder or .zip on the window")
                .small()
                .weak(),
        );
        let mut unload = (false, false);
        match &self.one {
            Some(one) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stage one").strong());
                    if ui.small_button("✖").on_hover_text("Unload").clicked() {
                        unload.0 = true;
                    }
                });
                ui.label(RichText::new(&one.format).monospace().small());
                ui.label(format!(
                    "{}  ·  {} Hz  ·  hop {}  ·  RF {}",
                    one.keywords.join(", "),
                    one.sample_rate,
                    one.hop,
                    one.receptive_field
                ))
                .on_hover_text(one.root.display().to_string());
            }
            None => {
                ui.label(
                    RichText::new("No stage one: the view shows the spectrogram only.").weak(),
                );
            }
        }
        match &self.two {
            Some(two) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Stage two").strong());
                    if ui.small_button("✖").on_hover_text("Unload").clicked() {
                        unload.1 = true;
                    }
                });
                ui.label(RichText::new(&two.format).monospace().small());
                ui.label(format!(
                    "{} member{}  ·  reads {:.2} s back",
                    two.members,
                    if two.members == 1 { "" } else { "s" },
                    two.reach as f64 / two.sample_rate as f64
                ))
                .on_hover_text(two.root.display().to_string());
            }
            None if self.one.is_some() => {
                ui.label(RichText::new("No stage two: every trigger is accepted.").weak());
            }
            None => {}
        }
        if unload.0 || unload.1 {
            self.busy = true;
            self.worker.send(Cmd::Unload {
                stage_one: unload.0,
                stage_two: unload.1,
            });
        }

        ui.separator();
        ui.heading("Detection");
        let (t1, t2) = (self.t1, self.t2);
        ui.add(
            Slider::new(&mut self.t1, 0.01..=0.99)
                .text("t1")
                .fixed_decimals(4)
                .clamping(egui::SliderClamping::Always),
        )
        .on_hover_text("Stage one arms when its score reaches this");
        ui.add_enabled(
            self.two.is_some(),
            Slider::new(&mut self.t2, -5.0..=10.0)
                .text("t2")
                .fixed_decimals(2),
        )
        .on_hover_text("Stage two accepts a trigger whose logit is at or above this");
        ui.horizontal(|ui| {
            if ui.small_button("Defaults").clicked() {
                self.t1 = DEFAULT_T1;
                self.t2 = DEFAULT_T2;
            }
        });
        if (self.t1, self.t2) != (t1, t2) {
            self.set_thresholds();
        }
        let c = DetectorConfig::det_v8(self.t1);
        ui.label(
            RichText::new(format!(
                "window {} · lockout {} · warm-up {} frames",
                c.window, c.lockout, c.warmup
            ))
            .small()
            .weak(),
        );
        if matches!(self.source, Source::Live { .. }) {
            ui.label(
                RichText::new("Live: a new threshold applies from now on.")
                    .small()
                    .weak(),
            );
        }

        ui.separator();
        ui.heading("Timing");
        egui::Grid::new("timing").num_columns(2).show(ui, |ui| {
            if let Some((s, audio)) = self.timing.push {
                ui.label("Last push");
                ui.label(format!("{:.1} ms for {:.0} ms", s * 1e3, audio * 1e3));
                ui.end_row();
            }
            let t = &self.timing;
            ui.label("Stage two");
            if t.stage_two_n > 0 {
                ui.label(format!(
                    "{} runs, mean {:.1} ms, max {:.1}",
                    t.stage_two_n,
                    t.stage_two_sum / t.stage_two_n as f64 * 1e3,
                    t.stage_two_max * 1e3
                ));
            } else {
                ui.label("—");
            }
            ui.end_row();
            if let Source::Live { .. } = self.source {
                let dropped = self.dropped + self.input.as_ref().map_or(0, |i| i.stats.dropped());
                if dropped > 0 {
                    ui.label(RichText::new("Dropped").color(Color32::LIGHT_RED));
                    ui.label(
                        RichText::new(format!("{dropped} mic samples")).color(Color32::LIGHT_RED),
                    )
                    .on_hover_text(
                        "The window was not drawn for a while; the stream has a gap there",
                    );
                    ui.end_row();
                }
            }
        });

        if let Source::File {
            path,
            channel,
            channels,
            source_rate,
            progress,
        } = &self.source
        {
            let (path, mut ch, channels, source_rate, done) = (
                path.clone(),
                *channel,
                *channels,
                *source_rate,
                progress.is_none(),
            );
            ui.separator();
            ui.heading("File");
            ui.label(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
            .on_hover_text(path.display().to_string());
            if done {
                ui.label(format!(
                    "{:.1} s · {} Hz{}",
                    self.timeline.duration(),
                    source_rate,
                    if source_rate != self.timeline.sample_rate {
                        format!(" → {} Hz", self.timeline.sample_rate)
                    } else {
                        String::new()
                    }
                ));
            }
            if channels > 1 {
                egui::ComboBox::from_label("Channel")
                    .selected_text(ch.to_string())
                    .show_ui(ui, |ui| {
                        for c in 0..channels {
                            ui.selectable_value(&mut ch, c, c.to_string());
                        }
                    });
                if ch != *channel {
                    self.open_file(path.clone(), ch);
                }
            }
            if ui
                .add_enabled(
                    done && self.one.is_some(),
                    egui::Button::new("Export JSONL…"),
                )
                .on_hover_text("One line per trigger, as `kws stream` writes them")
                .clicked()
            {
                self.export(&path, channels, ch);
            }
        }

        ui.separator();
        ui.heading("View");
        ui.add(
            Slider::new(&mut self.view.span, 1.0..=120.0)
                .text("seconds")
                .logarithmic(true)
                .fixed_decimals(1),
        );
        ui.add(Slider::new(&mut self.db_lo, -160.0..=-40.0).text("dB floor"));
        ui.add(Slider::new(&mut self.db_hi, -60.0..=0.0).text("dB ceiling"));
        self.db_hi = self.db_hi.max(self.db_lo + 10.0);
        let before = self.colormap;
        egui::ComboBox::from_label("Colormap")
            .selected_text(self.colormap.name())
            .show_ui(ui, |ui| {
                for c in Colormap::ALL {
                    ui.selectable_value(&mut self.colormap, c, c.name());
                }
            });
        if self.colormap != before {
            self.lut = self.colormap.lut();
        }
        ui.horizontal(|ui| {
            ui.label("Frequency");
            for s in Scale::ALL {
                ui.selectable_value(&mut self.scale, s, s.name());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Resolution");
            for r in Resolution::ALL {
                ui.selectable_value(&mut self.resolution, r, r.name());
            }
        })
        .response
        .on_hover_text(
            "Multi: a 96 ms window below 500 Hz, 32 ms up to 2 kHz, 16 ms above, \
             cross-faded over a third of an octave at each edge",
        );

        self.trigger_list(ui);

        ui.separator();
        self.updater.settings(ui);
        ui.separator();
        ui.label(
            RichText::new(
                "Drag to pan · scroll to zoom · double-click to follow · Space pause / resume",
            )
            .small()
            .weak(),
        );
    }

    fn trigger_list(&mut self, ui: &mut egui::Ui) {
        if self.timeline.triggers.is_empty() {
            return;
        }
        ui.separator();
        ui.heading("Triggers");
        let mut go = None;
        egui::ScrollArea::vertical()
            .id_salt("triggers")
            .max_height(220.0)
            .show(ui, |ui| {
                for (i, t) in self.timeline.triggers.iter().enumerate().rev() {
                    let mark = if t.accepted { "✔" } else { "✖" };
                    let logit = t
                        .stage_two_logit
                        .map(|l| format!("{l:+.2}"))
                        .unwrap_or_default();
                    let text = RichText::new(format!(
                        "{mark} {:>8.2} s  {:.3}  {logit}",
                        self.timeline.seconds(t.end_sample),
                        t.score
                    ))
                    .monospace()
                    .color(if t.accepted { GREEN } else { RED });
                    if ui
                        .selectable_label(self.selected == Some(i), text)
                        .clicked()
                    {
                        go = Some(i);
                    }
                }
            });
        if let Some(i) = go {
            self.selected = Some(i);
            let t = &self.timeline.triggers[i];
            let mid = self.timeline.seconds((t.start_sample + t.end_sample) / 2);
            self.view.end = mid + self.view.span / 2.0;
            self.view.follow = false;
        }
    }

    fn export(&mut self, path: &std::path::Path, channels: usize, channel: usize) {
        let name = format!(
            "{}.triggers.jsonl",
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let Some(out) = rfd::FileDialog::new()
            .set_file_name(name)
            .add_filter("JSON lines", &["jsonl"])
            .save_file()
        else {
            return;
        };
        let wav = path.display().to_string();
        // `kws stream` 只在点了声道时写 channel
        let channel = (channels > 1).then_some(channel);
        let mut text = String::new();
        for t in &self.timeline.triggers {
            text += &jsonl::line(&wav, t, channel);
            text.push('\n');
        }
        if let Err(e) = std::fs::write(&out, text) {
            self.error = Some(format!("{}: {e}", out.display()));
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        for path in dropped {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if matches!(ext.as_str(), "wav" | "wave" | "flac") {
                self.open_file(path, 0);
            } else {
                self.import(path);
            }
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                let full = ui.available_rect_before_wrap();
                let plot = Rect::from_min_max(pos2(full.left() + HZ_GUTTER, full.top()), full.max);
                let spec = Rect::from_min_max(
                    plot.min,
                    pos2(plot.right(), plot.bottom() - SCORE_H - AXIS_H),
                );
                let score = Rect::from_min_max(
                    pos2(plot.left(), spec.bottom() + 4.0),
                    pos2(plot.right(), plot.bottom() - AXIS_H),
                );
                let axis = Rect::from_min_max(pos2(plot.left(), score.bottom()), plot.max);

                let response = ui.allocate_rect(plot, Sense::click_and_drag());
                self.navigate(ui, &response, plot);

                // 一行一个物理像素
                let rows = (spec.height() * ctx.pixels_per_point()).round() as usize;
                self.timeline.spec.set_layout(Layout {
                    scale: self.scale,
                    rows: rows.clamp(16, MAX_ROWS),
                    resolution: self.resolution,
                });

                let painter = ui.painter_at(full);
                let (t0, t1) = self.window();
                if let Some(frame) = self.update_texture(ctx, spec.width()) {
                    let x = |t: f64| spec.left() + ((t - t0) / (t1 - t0)) as f32 * spec.width();
                    let s = &self.timeline.spec;
                    // 纹理列 j 盖住列 [j·step, (j+1)·step)，每列以自己的中心为中
                    let edge = |j: usize| x(s.time_of((j * frame.step) as f64 - 0.5));
                    for seg in &frame.segments {
                        let (u0, u1) = self.strip.uv(seg);
                        painter.with_clip_rect(spec).image(
                            frame.texture,
                            Rect::from_x_y_ranges(
                                edge(seg.first)..=edge(seg.first + seg.len),
                                spec.y_range(),
                            ),
                            Rect::from_min_max(pos2(u0, 0.0), pos2(u1, 1.0)),
                            Color32::WHITE,
                        );
                    }
                }
                self.draw_freq_ticks(&painter, spec);
                self.draw_triggers(&painter.with_clip_rect(spec), spec);
                self.draw_scores(&painter, score);
                self.draw_splices(&painter, spec.union(score));
                self.draw_time_axis(&painter, axis, spec);
                if let Some(p) = response.hover_pos() {
                    self.draw_hover(&painter, spec, score, p);
                }
            });
    }

    /// 看得见的 [t0, t1] 秒。
    fn window(&self) -> (f64, f64) {
        (self.view.end - self.view.span, self.view.end)
    }

    fn navigate(&mut self, ui: &egui::Ui, response: &egui::Response, plot: Rect) {
        let live = matches!(self.source, Source::Live { .. });
        let duration = self.timeline.duration();
        if self.view.follow && live {
            self.view.end = duration.max(self.view.span);
        }
        let per_px = self.view.span / plot.width().max(1.0) as f64;
        if response.dragged() {
            self.view.end -= response.drag_delta().x as f64 * per_px;
            self.view.follow = false;
        }
        if response.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if scroll.x != 0.0 {
                self.view.end -= scroll.x as f64 * per_px;
                self.view.follow = false;
            }
            let factor = (-(scroll.y as f64) * 0.004).exp() / zoom as f64;
            if factor != 1.0 {
                let at = response.hover_pos().map_or(self.view.end, |p| {
                    self.view.end - (plot.right() - p.x) as f64 * per_px
                });
                let span = (self.view.span * factor).clamp(0.5, 600.0);
                // 指针下的那一刻不动
                self.view.end = at + (self.view.end - at) * span / self.view.span;
                self.view.span = span;
                if !live || !self.view.follow {
                    self.view.follow = false;
                }
            }
        }
        if response.double_clicked() && live {
            self.view.follow = true;
        }
        let lo = self.view.span.min(duration.max(self.view.span));
        let hi = duration.max(self.view.span);
        self.view.end = self.view.end.clamp(lo, hi);
    }

    /// 只着色看得见、还没画过的列。
    fn update_texture(&mut self, ctx: &egui::Context, width: f32) -> Option<strip::Frame> {
        let s = &self.timeline.spec;
        let (t0, t1) = self.window();
        let lo = s.column_at(t0).floor().max(0.0) as usize;
        let hi = (s.column_at(t1).ceil().max(0.0) as usize + 1).max(lo + 1);
        let w = ((width as usize).max(1) * 2).min(MAX_TEXTURE_W);
        let span_cols = (self.view.span / crate::spectrogram::HOP_S).ceil() as usize + 1;
        self.strip.update(
            ctx,
            s,
            &self.lut,
            self.colormap,
            (self.db_lo, self.db_hi),
            (lo, hi),
            span_cols,
            w,
        )
    }

    fn time_x(&self, rect: Rect, t: f64) -> f32 {
        let (t0, t1) = self.window();
        rect.left() + ((t - t0) / (t1 - t0)) as f32 * rect.width()
    }

    fn x_time(&self, rect: Rect, x: f32) -> f64 {
        let (t0, t1) = self.window();
        t0 + ((x - rect.left()) / rect.width()) as f64 * (t1 - t0)
    }

    fn draw_freq_ticks(&self, painter: &Painter, spec: Rect) {
        let axis = self.timeline.spec.axis();
        for f in axis.ticks(spec.height()) {
            let y = spec.bottom() - axis.unit_of(f) * spec.height();
            painter.line_segment(
                [pos2(spec.left() - 4.0, y), pos2(spec.left(), y)],
                Stroke::new(1.0f32, Color32::GRAY),
            );
            let label = if f >= 1e3 {
                format!("{}k", f / 1e3)
            } else {
                format!("{f}")
            };
            painter.text(
                pos2(spec.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                label,
                FontId::proportional(11.0),
                Color32::GRAY,
            );
        }
    }

    fn draw_triggers(&self, painter: &Painter, spec: Rect) {
        let tl = &self.timeline;
        let (t0, t1) = self.window();
        for (i, t) in tl.triggers.iter().enumerate() {
            let (a, b) = (tl.seconds(t.start_sample), tl.seconds(t.end_sample));
            let lead = t.stage_two_from.map(|s| tl.seconds(s)).unwrap_or(a);
            if b < t0 || lead.min(a) > t1 {
                continue;
            }
            let (xa, xb) = (self.time_x(spec, a), self.time_x(spec, b));
            let body = Rect::from_x_y_ranges(xa..=xb, spec.y_range());
            let color = if t.accepted { GREEN } else { RED };
            if t.accepted {
                if lead < a {
                    let r = Rect::from_x_y_ranges(self.time_x(spec, lead)..=xa, spec.y_range());
                    painter.rect_filled(r, 0.0, GREEN.gamma_multiply(0.12));
                }
                painter.rect_filled(body, 0.0, GREEN.gamma_multiply(0.32));
            } else {
                let r = body.shrink(1.0);
                let corners = [
                    r.left_top(),
                    r.right_top(),
                    r.right_bottom(),
                    r.left_bottom(),
                    r.left_top(),
                ];
                painter.extend(Shape::dashed_line(
                    &corners,
                    Stroke::new(2.0f32, RED),
                    8.0,
                    5.0,
                ));
            }
            if self.selected == Some(i) {
                painter.rect_stroke(body, 0.0, Stroke::new(1.5f32, Color32::WHITE));
            }
            let label = match t.stage_two_logit {
                Some(l) => format!("{l:+.2}"),
                None => format!("{:.2}", t.score),
            };
            let at = pos2((xa + xb) / 2.0, spec.top() + 4.0);
            let galley = painter.layout_no_wrap(label, FontId::monospace(13.0), color);
            let bg = Rect::from_center_size(
                at + vec2(0.0, galley.size().y / 2.0),
                galley.size() + vec2(6.0, 2.0),
            );
            painter.rect_filled(bg, 3.0, Color32::from_black_alpha(170));
            painter.galley(bg.min + vec2(3.0, 1.0), galley, color);
        }
    }

    /// 暂停处：两边的音频在时间轴上挨着，其实中间停过。
    fn draw_splices(&self, painter: &Painter, rect: Rect) {
        let (t0, t1) = self.window();
        for &s in &self.timeline.splices {
            let t = self.timeline.seconds(s);
            if t < t0 || t > t1 {
                continue;
            }
            let x = self.time_x(rect, t);
            painter.extend(Shape::dashed_line(
                &[pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.5f32, Color32::WHITE),
                4.0,
                4.0,
            ));
            painter.text(
                pos2(x + 3.0, rect.bottom() - 3.0),
                Align2::LEFT_BOTTOM,
                "paused",
                FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }

    fn draw_scores(&self, painter: &Painter, score: Rect) {
        painter.rect_filled(score, 0.0, Color32::from_gray(20));
        let y = |p: f32| score.bottom() - p.clamp(0.0, 1.0) * score.height();
        let tl = &self.timeline;
        // 触发的区间在分数图上也淡淡地画出来
        for t in &tl.triggers {
            let xa = self.time_x(score, tl.seconds(t.start_sample));
            let xb = self.time_x(score, tl.seconds(t.end_sample));
            let color = if t.accepted { GREEN } else { RED };
            painter.with_clip_rect(score).rect_filled(
                Rect::from_x_y_ranges(xa..=xb, score.y_range()),
                0.0,
                color.gamma_multiply(0.15),
            );
        }
        if tl.keywords.is_empty() {
            painter.text(
                score.center(),
                Align2::CENTER_CENTER,
                "Import a stage-one bundle to see its score",
                FontId::proportional(13.0),
                Color32::GRAY,
            );
            return;
        }
        let t1y = y(self.t1 as f32);
        painter.extend(Shape::dashed_line(
            &[pos2(score.left(), t1y), pos2(score.right(), t1y)],
            Stroke::new(1.0f32, AMBER),
            6.0,
            4.0,
        ));
        painter.text(
            pos2(score.left() - 6.0, t1y),
            Align2::RIGHT_CENTER,
            format!("t1 {:.2}", self.t1),
            FontId::proportional(10.0),
            AMBER,
        );
        for p in [0.0, 1.0] {
            painter.text(
                pos2(score.left() - 6.0, y(p)),
                Align2::RIGHT_CENTER,
                format!("{p:.0}"),
                FontId::proportional(10.0),
                Color32::GRAY,
            );
        }
        // 每个像素至多一个点：区间里取最大，峰不会被抽掉
        let (t0, t1) = self.window();
        let frame_at = |t: f64| (t * tl.sample_rate as f64 - tl.n_fft as f64) / tl.hop as f64;
        let lo = frame_at(t0).floor().max(tl.first_frame as f64) as usize;
        let hi = (frame_at(t1).ceil().max(0.0) as usize + 1).min(tl.frame_end());
        if hi <= lo {
            return;
        }
        let per_px = ((hi - lo) as f32 / score.width()).max(1.0) as usize;
        let colors = [
            CYAN,
            Color32::from_rgb(255, 140, 220),
            Color32::from_rgb(180, 255, 120),
        ];
        for index in 0..tl.keywords.len() {
            let mut points = Vec::with_capacity((hi - lo) / per_px + 1);
            let mut f = lo;
            while f < hi {
                let end = (f + per_px).min(hi);
                let p = (f..end)
                    .filter_map(|j| tl.prob(j, index))
                    .fold(0.0f32, f32::max);
                points.push(pos2(self.time_x(score, tl.frame_time(f)), y(p)));
                f = end;
            }
            painter.with_clip_rect(score).add(Shape::line(
                points,
                Stroke::new(1.5f32, colors[index % colors.len()]),
            ));
        }
        if tl.keywords.len() > 1 {
            for (index, k) in tl.keywords.iter().enumerate() {
                painter.text(
                    score.right_top() + vec2(-6.0, 4.0 + 14.0 * index as f32),
                    Align2::RIGHT_TOP,
                    k,
                    FontId::proportional(11.0),
                    colors[index % colors.len()],
                );
            }
        }
    }

    fn draw_time_axis(&self, painter: &Painter, axis: Rect, spec: Rect) {
        let (t0, t1) = self.window();
        let px_per_s = axis.width() as f64 / (t1 - t0);
        let step = TIME_STEPS
            .into_iter()
            .find(|s| s * px_per_s >= 90.0)
            .unwrap_or(600.0);
        let mut t = (t0 / step).ceil() * step;
        while t <= t1 {
            let x = self.time_x(axis, t);
            painter.line_segment(
                [pos2(x, spec.top()), pos2(x, axis.top())],
                Stroke::new(1.0f32, Color32::from_white_alpha(14)),
            );
            let label = if step < 1.0 {
                format!("{}:{:04.1}", (t / 60.0).floor(), t % 60.0)
            } else {
                format!("{}:{:02.0}", (t / 60.0).floor(), (t % 60.0).floor())
            };
            painter.text(
                pos2(x, axis.top() + 3.0),
                Align2::CENTER_TOP,
                label,
                FontId::proportional(11.0),
                Color32::GRAY,
            );
            t += step;
        }
    }

    fn draw_hover(&self, painter: &Painter, spec: Rect, score: Rect, p: Pos2) {
        let tl = &self.timeline;
        let t = self.x_time(spec, p.x);
        painter.line_segment(
            [pos2(p.x, spec.top()), pos2(p.x, score.bottom())],
            Stroke::new(1.0f32, Color32::from_white_alpha(60)),
        );
        let mut lines = vec![format!("{t:.3} s")];
        if spec.contains(p) {
            let f = tl
                .spec
                .axis()
                .freq_at((spec.bottom() - p.y) / spec.height());
            lines.push(format!("{f:.0} Hz"));
        }
        if !tl.keywords.is_empty() {
            let frame = ((t * tl.sample_rate as f64 - tl.n_fft as f64) / tl.hop as f64).round();
            if frame >= 0.0 {
                for (i, k) in tl.keywords.iter().enumerate() {
                    if let Some(v) = tl.prob(frame as usize, i) {
                        lines.push(format!("{k} {v:.3}"));
                    }
                }
            }
        }
        let sample = (t.max(0.0) * tl.sample_rate as f64) as usize;
        for tr in tl
            .triggers
            .iter()
            .filter(|tr| tr.start_sample <= sample && sample <= tr.end_sample)
        {
            lines.push(format!(
                "{} {} · frame {} · score {:.3}{}",
                if tr.accepted { "accepted" } else { "rejected" },
                tr.keyword,
                tr.decision_frame,
                tr.score,
                tr.stage_two_logit
                    .map(|l| format!(" · logit {l:+.3}"))
                    .unwrap_or_default()
            ));
        }
        let text = lines.join("\n");
        let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), Color32::WHITE);
        let mut at = p + vec2(14.0, 14.0);
        if at.x + galley.size().x > spec.right() {
            at.x = p.x - 14.0 - galley.size().x;
        }
        let bg = Rect::from_min_size(at, galley.size()).expand(4.0);
        painter.rect_filled(bg, 4.0, Color32::from_black_alpha(200));
        painter.galley(at, galley, Color32::WHITE);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 吃掉这个键，免得刚点过的按钮再被它按一次；正在输入时让给输入框
        if !ctx.wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space))
        {
            self.toggle_pause();
        }
        self.drain_replies();
        self.pump_audio();
        self.handle_drops(ctx);
        self.top_bar(ctx);
        self.sidebar(ctx);
        self.central(ctx);
        if self.input.is_some() {
            ctx.request_repaint_after(LIVE_REPAINT);
        }
    }
}

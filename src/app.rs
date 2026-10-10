use crate::audio::{self, Input};
use crate::axis::LogAxis;
use crate::colormap::Colormap;
use crate::dsp::{self, Stft};
use crate::music::{self, Scale, NOTE_NAMES};
use crate::perf::{Latency, Perf};
use crate::pitch::Yin;
use crate::waterfall::{RowMap, Waterfall};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Sense, Shape, Slider,
    Stroke, TextureHandle, TextureOptions,
};
use std::time::Instant;

// 纹理分辨率与窗口无关，GPU 线性缩放到绘制区域
const ROWS: usize = 600;
const COLS: usize = 1024;
/// 横向要绕回，见 `central` 里的 uv。
const TEXTURE: TextureOptions = TextureOptions::LINEAR_REPEAT;
const FFT_SIZES: [usize; 5] = [1024, 2048, 4096, 8192, 16384];
const HOPS: [usize; 5] = [128, 256, 512, 1024, 2048];
const FREQ_TICKS: [f32; 15] = [
    20.0, 30.0, 50.0, 100.0, 200.0, 300.0, 500.0, 1e3, 2e3, 3e3, 5e3, 1e4, 1.5e4, 2e4, 3e4,
];
const MAX_HARMONIC: u32 = 8;

// 覆盖人声和多数旋律乐器的基频
const PITCH_MIN: f32 = 40.0;
const PITCH_MAX: f32 = 2_000.0;
// 相邻两列音高差超过这么多半音就断开轨迹，不画竖直连线
const MAX_TRACK_JUMP: f32 = 2.0;

const COPYRIGHT_YEAR: u32 = 2026;

const HZ_GUTTER: f32 = 44.0;
const NOTE_GUTTER: f32 = 44.0;
const LIVE_W: f32 = 160.0;
// 相邻音名标签的最小间距（像素），更密时只画线不写字
const LABEL_SPACING: f32 = 11.0;

const BG: Color32 = Color32::from_gray(12);
const AMBER: Color32 = Color32::from_rgb(255, 214, 102);
const CYAN: Color32 = Color32::from_rgb(120, 200, 255);
const GREEN: Color32 = Color32::from_rgb(110, 255, 140);

#[derive(Clone, Copy, PartialEq)]
struct Analysis {
    fft_size: usize,
    hop: usize,
}

pub struct App {
    devices: Vec<String>,
    device: Option<String>,
    input: Option<Input>,
    error: Option<String>,
    sample_rate: f32,
    samples: Vec<f32>,

    analysis: Analysis,
    stft: Stft,
    yin: Yin,
    waterfall: Waterfall,
    peak: Option<(f32, f32)>,
    paused: bool,
    perf: Perf,

    axis: LogAxis,
    db_lo: f32,
    db_hi: f32,
    colormap: Colormap,
    lut: [Color32; 256],
    texture: Option<TextureHandle>,
    /// 纹理现有像素用的色表与 dB 范围；变了就整张重新着色。
    painted: Option<(Colormap, f32, f32)>,

    scale: Option<Scale>,
    tonic: u8,
    a4: f32,
    level_gate: f32,
    show_pitch: bool,
    harmonics: bool,
}

/// 音高窗长：两倍最长周期向上取 2 的幂，且不超过 FFT 帧长（取帧中央那一段，
/// 与瀑布图该列对齐）。
fn pitch_window(fft_size: usize, sample_rate: f32) -> usize {
    let want = ((2.0 * sample_rate / PITCH_MIN).ceil() as usize).next_power_of_two();
    want.min(fft_size)
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let analysis = Analysis {
            fft_size: 4096,
            hop: 256,
        };
        let axis = LogAxis {
            f_min: 50.0,
            f_max: 8_000.0,
        };
        let sample_rate = 48_000.0;
        let stft = Stft::new(analysis.fft_size, analysis.hop);
        let colormap = Colormap::Magma;
        let mut app = Self {
            devices: audio::input_device_names(),
            device: None,
            input: None,
            error: None,
            sample_rate,
            samples: Vec::new(),
            waterfall: Waterfall::new(
                COLS,
                RowMap::new(
                    axis,
                    ROWS,
                    stft.bins(),
                    sample_rate / analysis.fft_size as f32,
                ),
            ),
            yin: Yin::new(pitch_window(analysis.fft_size, sample_rate), sample_rate),
            stft,
            analysis,
            peak: None,
            paused: false,
            perf: Perf::default(),
            axis,
            db_lo: -100.0,
            db_hi: -20.0,
            colormap,
            lut: colormap.lut(),
            texture: None,
            painted: None,
            scale: Some(Scale::Chromatic),
            tonic: 0,
            a4: 440.0,
            level_gate: -60.0,
            show_pitch: true,
            harmonics: true,
        };
        app.switch_device(None);
        app
    }

    fn bin_hz(&self) -> f32 {
        self.sample_rate / self.analysis.fft_size as f32
    }

    fn row_map(&self) -> RowMap {
        RowMap::new(self.axis, ROWS, self.stft.bins(), self.bin_hz())
    }

    fn reset_analysis(&mut self) {
        self.stft = Stft::new(self.analysis.fft_size, self.analysis.hop);
        self.yin = Yin::new(
            pitch_window(self.analysis.fft_size, self.sample_rate),
            self.sample_rate,
        );
        self.waterfall = Waterfall::new(COLS, self.row_map());
        self.peak = None;
    }

    fn switch_device(&mut self, name: Option<String>) {
        // 先停旧流，有的后端不允许同一设备开两条
        self.input = None;
        match audio::open(name.as_deref()) {
            Ok(input) => {
                self.sample_rate = input.sample_rate as f32;
                self.device = Some(input.device_name.clone());
                self.input = Some(input);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.axis.f_max = self.axis.f_max.min(self.sample_rate / 2.0);
        self.reset_analysis();
    }

    fn pump_audio(&mut self) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        self.samples.clear();
        while let Ok(s) = input.consumer.pop() {
            self.samples.push(s);
        }
        self.perf.popped.add(self.samples.len() as f32);
        // 暂停时照样取走样本，恢复后直接接上实时
        if self.paused {
            self.perf.dsp.add(0.0);
            self.perf.cols.add(0.0);
            return;
        }
        let wf = &mut self.waterfall;
        let yin = &mut self.yin;
        let gate = self.level_gate;
        let mut cols = 0;
        let t0 = Instant::now();
        self.stft.process(&self.samples, |spec, frame| {
            cols += 1;
            let n = yin.len();
            let start = (frame.len() - n) / 2;
            let x = &frame[start..start + n];
            let pitch = if dsp::rms_db(x) >= gate {
                yin.detect(x, PITCH_MIN, PITCH_MAX)
            } else {
                None
            };
            wf.push(spec, pitch);
        });
        self.perf.dsp.add(t0.elapsed().as_secs_f32());
        self.perf.cols.add(cols as f32);
        self.perf.unframed.add(self.stft.unframed() as f32);
        if let Some(latest) = self.waterfall.latest_raw() {
            self.peak = dsp::peak(latest, self.bin_hz(), self.axis.f_min, self.axis.f_max);
        }
    }

    fn freq_y(&self, rect: Rect, f: f32) -> f32 {
        rect.bottom() - self.axis.unit_of(f) * rect.height()
    }

    fn y_freq(&self, rect: Rect, y: f32) -> f32 {
        self.axis.freq_at((rect.bottom() - y) / rect.height())
    }

    fn gated_peak(&self) -> Option<(f32, f32)> {
        self.peak.filter(|&(_, db)| db >= self.level_gate)
    }

    fn note_text(&self, f: f32) -> String {
        let (m, cents) = music::nearest_note(f, self.a4);
        format!("{:<4} {:+4.0} ct", music::note_name(m), cents)
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let mut chosen = None;
                egui::ComboBox::from_label("")
                    .width(240.0)
                    .selected_text(self.device.as_deref().unwrap_or("No input"))
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
                if chosen.is_some() && chosen != self.device {
                    self.switch_device(chosen);
                }

                let label = if self.paused {
                    "▶ Resume"
                } else {
                    "⏸ Pause"
                };
                if ui.button(label).clicked() {
                    self.paused = !self.paused;
                }
                ui.separator();

                let latest = self.waterfall.pitch_at(self.waterfall.cols() - 1);
                let readout = match latest {
                    Some(f) => format!("Pitch {}  {:7.1} Hz", self.note_text(f), f),
                    None => "Pitch —".into(),
                };
                ui.label(RichText::new(readout).monospace().size(18.0).color(GREEN));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(e) = &self.error {
                        ui.label(RichText::new(e).color(Color32::LIGHT_RED));
                    } else if self.paused {
                        ui.label(RichText::new("PAUSED").color(Color32::LIGHT_RED).strong());
                    } else {
                        ui.label(format!("{:.0} Hz", self.sample_rate));
                    }
                });
            });
        });
    }

    fn settings_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("settings")
            .resizable(false)
            .exact_width(240.0)
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
        ui.add_space(4.0);
        ui.heading("Analysis");
        egui::ComboBox::from_label("FFT size")
            .selected_text(self.analysis.fft_size.to_string())
            .show_ui(ui, |ui| {
                for n in FFT_SIZES {
                    ui.selectable_value(&mut self.analysis.fft_size, n, n.to_string());
                }
            });
        let fft_size = self.analysis.fft_size;
        self.analysis.hop = self.analysis.hop.min(fft_size);
        egui::ComboBox::from_label("Hop")
            .selected_text(self.analysis.hop.to_string())
            .show_ui(ui, |ui| {
                for h in HOPS.into_iter().filter(|&h| h <= fft_size) {
                    ui.selectable_value(&mut self.analysis.hop, h, h.to_string());
                }
            });
        let dt = self.analysis.hop as f32 / self.sample_rate;
        ui.label(
                    RichText::new(format!(
                        "Δf {:.2} Hz  ·  Δt {:.1} ms\nwindow {:.0} ms  ·  history {:.1} s\nlowest pitch ≈ {:.0} Hz",
                        self.bin_hz(),
                        dt * 1e3,
                        fft_size as f32 / self.sample_rate * 1e3,
                        dt * COLS as f32,
                        self.yin.lowest_freq().max(PITCH_MIN),
                    ))
                    .small()
                    .weak(),
                );

        ui.separator();
        ui.heading("Display");
        let nyquist = self.sample_rate / 2.0;
        ui.add(
            Slider::new(&mut self.axis.f_min, 20.0..=2_000.0)
                .logarithmic(true)
                .suffix(" Hz")
                .text("f min"),
        );
        ui.add(
            Slider::new(&mut self.axis.f_max, 200.0..=nyquist)
                .logarithmic(true)
                .suffix(" Hz")
                .text("f max"),
        );
        // 至少留一个八度
        if self.axis.f_max < self.axis.f_min * 2.0 {
            self.axis.f_max = (self.axis.f_min * 2.0).min(nyquist);
            self.axis.f_min = self.axis.f_min.min(self.axis.f_max / 2.0);
        }
        ui.add(
            Slider::new(&mut self.db_hi, -80.0..=0.0)
                .suffix(" dB")
                .text("top"),
        );
        ui.add(
            Slider::new(&mut self.db_lo, -140.0..=-20.0)
                .suffix(" dB")
                .text("floor"),
        );
        if self.db_hi < self.db_lo + 10.0 {
            self.db_lo = self.db_hi - 10.0;
        }
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

        ui.separator();
        ui.heading("Pitch & scale");
        ui.add(
            Slider::new(&mut self.level_gate, -120.0..=0.0)
                .suffix(" dB")
                .text("level gate"),
        )
        .on_hover_text("Frames below this level get no pitch and no peak marker");
        ui.checkbox(&mut self.show_pitch, "Pitch track");
        egui::ComboBox::from_label("Scale")
            .selected_text(self.scale.map_or("Off", Scale::name))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.scale, None, "Off");
                for s in Scale::ALL {
                    ui.selectable_value(&mut self.scale, Some(s), s.name());
                }
            });
        egui::ComboBox::from_label("Tonic")
            .selected_text(NOTE_NAMES[self.tonic as usize])
            .show_ui(ui, |ui| {
                for (i, n) in NOTE_NAMES.iter().enumerate() {
                    ui.selectable_value(&mut self.tonic, i as u8, *n);
                }
            });
        ui.add(
            Slider::new(&mut self.a4, 400.0..=480.0)
                .step_by(0.5)
                .suffix(" Hz")
                .text("A4"),
        );
        ui.checkbox(&mut self.harmonics, "Harmonics on hover");

        ui.separator();
        self.perf_section(ui);

        ui.separator();
        ui.label(RichText::new("Space  pause / resume").small().weak());
    }

    fn latency(&self) -> Latency {
        let sr = self.sample_rate;
        let stats = self.input.as_ref().map(|i| &i.stats);
        let device = match stats {
            Some(st) if st.capture_us() > 0 => st.capture_us() as f32 * 1e-6,
            Some(st) => st.block() as f32 / sr,
            None => 0.0,
        };
        Latency {
            device,
            queue: 0.5 * self.perf.popped.get() / sr,
            window: (0.5 * self.analysis.fft_size as f32 + self.perf.unframed.get()) / sr,
            display: self.perf.frame.get(),
        }
    }

    fn perf_section(&self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(RichText::new("Performance").heading())
            .default_open(true)
            .show(ui, |ui| {
                let p = &self.perf;
                let ms = |s: f32| format!("{:5.1} ms", s * 1e3);
                let frame = p.frame.get();
                let audio_per_frame = p.popped.get() / self.sample_rate;
                let pct = |num: f32, den: f32| {
                    if den > 0.0 {
                        format!("{:4.1} %", 100.0 * num / den)
                    } else {
                        "—".into()
                    }
                };
                let lat = self.latency();
                let block = self.input.as_ref().map_or(0, |i| i.stats.block());
                let dropped = self.input.as_ref().map_or(0, |i| i.stats.dropped());
                let rows: [(&str, String, String, &str); 10] = [
                    (
                        "fps",
                        format!("{:5.1}", if frame > 0.0 { 1.0 / frame } else { 0.0 }),
                        ms(frame),
                        "Frames per second and frame interval",
                    ),
                    (
                        "frame cpu",
                        ms(p.cpu.get()),
                        pct(p.cpu.get(), frame),
                        "UI thread time per frame (update + paint, excluding vsync wait) \
                         and its share of the frame interval",
                    ),
                    (
                        "  colorize",
                        ms(p.colorize.get()),
                        String::new(),
                        "Colouring new waterfall columns and uploading them to the texture; \
                         a colormap or dB range change repaints the whole image once",
                    ),
                    (
                        "  dsp",
                        ms(p.dsp.get()),
                        format!("{:3.0} col", p.cols.get()),
                        "STFT + YIN + waterfall write per frame, and columns produced",
                    ),
                    (
                        "dsp load",
                        pct(p.dsp.get(), audio_per_frame),
                        String::new(),
                        "DSP time as a share of the audio it processed (100 % = cannot keep up)",
                    ),
                    (
                        "latency ≈",
                        ms(lat.total()),
                        String::new(),
                        "Estimated sound-to-screen delay to the centre of the newest column",
                    ),
                    (
                        "  device",
                        ms(lat.device),
                        format!("{block} smp"),
                        "Capture to callback as reported by the driver, and the callback block size",
                    ),
                    (
                        "  queue",
                        ms(lat.queue),
                        String::new(),
                        "Average wait in the ring buffer until the next UI frame picks samples up",
                    ),
                    (
                        "  window",
                        ms(lat.window),
                        String::new(),
                        "Half the FFT window plus samples not yet in a column; \
                         shrink FFT size to cut it",
                    ),
                    (
                        "  display",
                        ms(lat.display),
                        String::new(),
                        "One frame from paint to scan-out",
                    ),
                ];
                egui::Grid::new("perf")
                    .num_columns(3)
                    .spacing(vec2(8.0, 1.0))
                    .show(ui, |ui| {
                        let mono = |t: &str| RichText::new(t).monospace().small();
                        for (name, value, extra, tip) in &rows {
                            ui.label(mono(name).weak()).on_hover_text(*tip);
                            ui.label(mono(value));
                            ui.label(mono(extra).weak());
                            ui.end_row();
                        }
                        let dropped_text = mono(&dropped.to_string());
                        ui.label(mono("dropped").weak())
                            .on_hover_text("Samples lost because the ring buffer was full");
                        ui.label(if dropped > 0 {
                            dropped_text.color(Color32::LIGHT_RED)
                        } else {
                            dropped_text
                        });
                        ui.end_row();
                    });
            });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                let full = ui.available_rect_before_wrap();
                let resp = ui.allocate_rect(full, Sense::hover());
                let spec = Rect::from_min_max(
                    pos2(full.left() + HZ_GUTTER, full.top() + 8.0),
                    pos2(full.right() - NOTE_GUTTER - LIVE_W, full.bottom() - 8.0),
                );
                let live = Rect::from_min_max(
                    pos2(full.right() - LIVE_W, spec.top()),
                    pos2(full.right() - 8.0, spec.bottom()),
                );
                if spec.width() < 50.0 || spec.height() < 50.0 {
                    return;
                }
                let painter = ui.painter_at(full);

                let tex_id = self.update_texture(ctx);
                // 纹理按槽位存列，从最老的槽位起绕回一圈（REPEAT）。两端各收半个
                // 纹素，线性插值不会把最新一列和最老一列混在一起。
                let cols = self.waterfall.cols() as f32;
                let head = self.waterfall.head() as f32;
                let uv = Rect::from_min_max(
                    pos2((head + 0.5) / cols, 0.0),
                    pos2((head + cols - 0.5) / cols, 1.0),
                );
                painter.image(tex_id, spec, uv, Color32::WHITE);

                self.draw_freq_ticks(&painter, spec);
                self.draw_scale(&painter, spec);
                if self.show_pitch {
                    self.draw_pitch(&painter, spec);
                }
                self.draw_live(&painter, live);
                if let Some(p) = resp.hover_pos().filter(|p| spec.contains(*p)) {
                    self.draw_hover(&painter, spec, p);
                }
            });
    }

    /// 只给新写入的列着色并局部上传；色表或 dB 范围变了才整张重来。
    fn update_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        let t0 = Instant::now();
        let style = (self.colormap, self.db_lo, self.db_hi);
        let mut dirty = self.waterfall.take_dirty();
        if self.texture.is_none() || self.painted != Some(style) {
            dirty = [0..self.waterfall.cols(), 0..0];
            self.painted = Some(style);
        }
        for slots in dirty.into_iter().filter(|r| !r.is_empty()) {
            let start = slots.start;
            let image = self
                .waterfall
                .render(&self.lut, self.db_lo, self.db_hi, slots);
            match &mut self.texture {
                Some(t) => t.set_partial([start, 0], image, TEXTURE),
                None => self.texture = Some(ctx.load_texture("waterfall", image, TEXTURE)),
            }
        }
        self.perf.colorize.add(t0.elapsed().as_secs_f32());
        self.texture.as_ref().expect("painted above").id()
    }

    fn draw_freq_ticks(&self, painter: &Painter, spec: Rect) {
        let stroke = Stroke::new(1.0, Color32::from_gray(140));
        for f in FREQ_TICKS {
            if f < self.axis.f_min || f > self.axis.f_max {
                continue;
            }
            let y = self.freq_y(spec, f);
            painter.line_segment([pos2(spec.left() - 5.0, y), pos2(spec.left(), y)], stroke);
            let label = if f >= 1e3 {
                format!("{}k", f / 1e3)
            } else {
                format!("{f}")
            };
            painter.text(
                pos2(spec.left() - 7.0, y),
                Align2::RIGHT_CENTER,
                label,
                FontId::monospace(11.0),
                Color32::from_gray(170),
            );
        }
    }

    fn draw_scale(&self, painter: &Painter, spec: Rect) {
        let Some(scale) = self.scale else {
            return;
        };
        let lines =
            music::scale_lines(scale, self.tonic, self.a4, self.axis.f_min, self.axis.f_max);
        for l in &lines {
            let y = self.freq_y(spec, l.freq);
            let color = if l.is_tonic {
                Color32::from_rgba_unmultiplied(255, 214, 102, 130)
            } else {
                Color32::from_rgba_unmultiplied(200, 220, 255, 45)
            };
            painter.line_segment(
                [pos2(spec.left(), y), pos2(spec.right(), y)],
                Stroke::new(1.0, color),
            );
        }
        // 主音标签优先占位，其余音名在不挤的地方补上
        let mut taken: Vec<f32> = Vec::new();
        for tonic_pass in [true, false] {
            for l in lines.iter().filter(|l| l.is_tonic == tonic_pass) {
                let y = self.freq_y(spec, l.freq);
                if taken.iter().any(|t| (t - y).abs() < LABEL_SPACING) {
                    continue;
                }
                taken.push(y);
                painter.text(
                    pos2(spec.right() + 5.0, y),
                    Align2::LEFT_CENTER,
                    music::note_name(l.midi),
                    FontId::monospace(10.5),
                    if l.is_tonic {
                        AMBER
                    } else {
                        Color32::from_gray(170)
                    },
                );
            }
        }
    }

    fn draw_pitch(&self, painter: &Painter, spec: Rect) {
        let cols = self.waterfall.cols();
        let mut track: Vec<Pos2> = Vec::new();
        let mut prev: Option<f32> = None;
        for x in 0..cols {
            let f = self
                .waterfall
                .pitch_at(x)
                .filter(|f| (self.axis.f_min..=self.axis.f_max).contains(f));
            let jump = matches!((prev, f), (Some(a), Some(b)) if 12.0 * (b / a).log2().abs() > MAX_TRACK_JUMP);
            if f.is_none() || jump {
                stroke_track(painter, &mut track);
            }
            if let Some(f) = f {
                let px = spec.left() + (x as f32 + 0.5) / cols as f32 * spec.width();
                track.push(pos2(px, self.freq_y(spec, f)));
            }
            prev = f;
        }
        stroke_track(painter, &mut track);
    }

    fn draw_live(&self, painter: &Painter, live: Rect) {
        painter.rect_filled(live, 2.0, Color32::from_gray(20));
        let col = self.waterfall.mapped_column(self.waterfall.cols() - 1);
        let rows = self.waterfall.rows() as f32;
        let span = self.db_hi - self.db_lo;
        let points: Vec<Pos2> = col
            .iter()
            .enumerate()
            .map(|(r, &db)| {
                let t = ((db - self.db_lo) / span).clamp(0.0, 1.0);
                pos2(
                    live.left() + t * live.width(),
                    live.top() + (r as f32 + 0.5) / rows * live.height(),
                )
            })
            .collect();
        painter.add(Shape::line(points, Stroke::new(1.0, CYAN)));

        // 琥珀：最强频率；绿：音高。泛音比基音响时两者会差一个或几个八度
        if let Some((f, _)) = self.gated_peak() {
            let y = self.freq_y(live, f);
            painter.line_segment(
                [pos2(live.left(), y), pos2(live.right(), y)],
                Stroke::new(1.0, AMBER),
            );
        }
        if let Some(f) = self.waterfall.pitch_at(self.waterfall.cols() - 1) {
            let y = self.freq_y(live, f);
            painter.line_segment(
                [pos2(live.left(), y), pos2(live.right(), y)],
                Stroke::new(1.5, GREEN),
            );
        }
    }

    fn draw_hover(&self, painter: &Painter, spec: Rect, p: Pos2) {
        let f = self.y_freq(spec, p.y);
        let cross = Stroke::new(1.0, Color32::from_white_alpha(70));
        painter.line_segment([pos2(spec.left(), p.y), pos2(spec.right(), p.y)], cross);
        painter.line_segment([pos2(p.x, spec.top()), pos2(p.x, spec.bottom())], cross);

        if self.harmonics {
            let stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 140, 60, 150));
            for k in 2..=MAX_HARMONIC {
                let fk = f * k as f32;
                if fk > self.axis.f_max {
                    break;
                }
                let y = self.freq_y(spec, fk);
                painter.extend(Shape::dashed_line(
                    &[pos2(spec.left(), y), pos2(spec.right(), y)],
                    stroke,
                    6.0,
                    4.0,
                ));
                painter.text(
                    pos2(p.x + 4.0, y - 1.0),
                    Align2::LEFT_BOTTOM,
                    format!("×{k}"),
                    FontId::monospace(10.0),
                    stroke.color,
                );
            }
        }

        let cols = self.waterfall.cols();
        let rows = self.waterfall.rows();
        let x = (((p.x - spec.left()) / spec.width() * cols as f32) as usize).min(cols - 1);
        let r = (((p.y - spec.top()) / spec.height() * rows as f32) as usize).min(rows - 1);
        let db = self.waterfall.mapped_column(x)[r];
        let age = (cols - 1 - x) as f32 * self.analysis.hop as f32 / self.sample_rate;
        let pitch = match self.waterfall.pitch_at(x) {
            Some(pf) => format!("Pitch {}", self.note_text(pf)),
            None => "Pitch —".into(),
        };
        let text = format!(
            "{f:8.1} Hz\n{}\n{db:8.1} dB\n{age:8.2} s ago\n{pitch}",
            self.note_text(f)
        );

        let galley = painter.layout_no_wrap(text, FontId::monospace(12.0), Color32::WHITE);
        let size = galley.size();
        let mut pos = p + vec2(14.0, 14.0);
        if pos.x + size.x > spec.right() {
            pos.x = p.x - 14.0 - size.x;
        }
        if pos.y + size.y > spec.bottom() {
            pos.y = p.y - 14.0 - size.y;
        }
        painter.rect_filled(
            Rect::from_min_size(pos, size).expand(5.0),
            4.0,
            Color32::from_black_alpha(210),
        );
        painter.galley(pos, galley, Color32::WHITE);
    }
}

/// 画一段音高轨迹（深色描边打底，任何色表上都看得清），然后清空。
fn stroke_track(painter: &Painter, track: &mut Vec<Pos2>) {
    match track.len() {
        0 => {}
        1 => {
            painter.circle_filled(track[0], 1.5, GREEN);
        }
        _ => {
            painter.add(Shape::line(
                track.clone(),
                Stroke::new(4.0, Color32::from_black_alpha(160)),
            ));
            painter.add(Shape::line(track.clone(), Stroke::new(2.0, GREEN)));
        }
    }
    track.clear();
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.perf.tick(frame.info().cpu_usage);
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.paused = !self.paused;
        }
        self.pump_audio();

        let (analysis, axis) = (self.analysis, self.axis);
        self.top_bar(ctx);
        self.settings_panel(ctx);
        if self.analysis != analysis {
            self.reset_analysis();
        } else if self.axis != axis {
            self.waterfall.set_map(self.row_map());
        }

        self.central(ctx);
        ctx.request_repaint();
    }
}

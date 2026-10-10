//! 运行时自测：帧率、各阶段耗时与端到端延迟估计，供侧栏 Performance 区显示。

use std::time::Instant;

/// 指数滑动平均系数：60 Hz 下约半秒的时间常数，读数不抖又跟得上设置变化。
const SMOOTH: f32 = 1.0 / 30.0;

#[derive(Default, Clone, Copy)]
pub struct Ema(Option<f32>);

impl Ema {
    pub fn add(&mut self, x: f32) {
        self.0 = Some(match self.0 {
            Some(m) => m + (x - m) * SMOOTH,
            None => x,
        });
    }

    pub fn get(self) -> f32 {
        self.0.unwrap_or(0.0)
    }
}

/// 每帧一次采样，全部取滑动平均。时间单位为秒，样本数为单声道样本。
#[derive(Default)]
pub struct Perf {
    last_frame: Option<Instant>,
    /// 相邻两次 `update` 的间隔。
    pub frame: Ema,
    /// eframe 报告的上一帧 `update` + 绘制耗时，不含等 vsync。
    pub cpu: Ema,
    /// STFT + YIN + 写入瀑布图。
    pub dsp: Ema,
    /// 每帧产出的瀑布图列数。
    pub cols: Ema,
    /// 整张瀑布图按色表着色并交给纹理。
    pub colorize: Ema,
    /// 每帧从环形缓冲取走的样本数。
    pub popped: Ema,
    /// 处理完后最新一列之后、尚未进入任何一列的样本数。
    pub unframed: Ema,
}

impl Perf {
    pub fn tick(&mut self, cpu_usage: Option<f32>) {
        let now = Instant::now();
        if let Some(prev) = self.last_frame.replace(now) {
            self.frame.add((now - prev).as_secs_f32());
        }
        if let Some(s) = cpu_usage {
            self.cpu.add(s);
        }
    }
}

/// 一个声音事件从进麦克风到出现在屏幕最新一列中心的延迟估计，各段单位秒。
pub struct Latency {
    /// 采集到回调：设备缓冲 + 驱动；后端不报时间戳时取一个缓冲的时长。
    pub device: f32,
    /// 样本在环形缓冲里等 UI 线程来取的平均时间（均匀到达，平均等半个取样间隔）。
    pub queue: f32,
    /// 最新一列的时间中心落后最新样本的量：半个 FFT 窗 + 还没凑够一帧的样本。
    pub window: f32,
    /// 画完到上屏，按一帧计。
    pub display: f32,
}

impl Latency {
    pub fn total(&self) -> f32 {
        self.device + self.queue + self.window + self.display
    }
}

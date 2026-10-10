//! 麦克风输入：cpal 回调里只做下混成单声道并推进无锁环形缓冲，UI 线程每帧取走。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

/// 回调线程写、UI 线程读的计数，只用原子量，不让音频线程碰锁。
#[derive(Default)]
pub struct Stats {
    /// 最近一次回调的帧数（每声道样本数），即设备缓冲大小。
    block: AtomicU32,
    /// 最近一次回调里，首个样本的采集时刻到回调开始的间隔（µs）；后端不给时间戳时为 0。
    capture_us: AtomicU32,
    /// 环形缓冲满时丢掉的样本累计数。
    dropped: AtomicU64,
}

impl Stats {
    pub fn block(&self) -> u32 {
        self.block.load(Relaxed)
    }

    pub fn capture_us(&self) -> u32 {
        self.capture_us.load(Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Relaxed)
    }
}

pub struct Input {
    // 持有即在录；drop 即停
    _stream: cpal::Stream,
    pub consumer: Consumer<f32>,
    pub stats: Arc<Stats>,
    pub sample_rate: u32,
    pub device_name: String,
}

pub fn input_device_names() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// `name` 为 `None` 时用系统默认输入设备。
pub fn open(name: Option<&str>) -> Result<Input, String> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().is_ok_and(|x| x == n))
            .ok_or_else(|| format!("input device not found: {n}"))?,
        None => host
            .default_input_device()
            .ok_or("no default input device")?,
    };
    let device_name = device.name().unwrap_or_else(|_| "?".into());
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    // 两秒余量：UI 卡顿时不丢样本；真满了丢新样本而不是阻塞音频线程
    let (producer, consumer) = RingBuffer::new(sample_rate as usize * 2);
    let stats = Arc::new(Stats::default());
    let s = stats.clone();
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, channels, producer, s),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, channels, producer, s),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, channels, producer, s),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, channels, producer, s),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(Input {
        _stream: stream,
        consumer,
        stats,
        sample_rate,
        device_name,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut producer: Producer<f32>,
    stats: Arc<Stats>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    let gain = 1.0 / channels as f32;
    device.build_input_stream(
        config,
        move |data: &[T], info: &cpal::InputCallbackInfo| {
            let ts = info.timestamp();
            let capture = ts.callback.duration_since(&ts.capture).unwrap_or_default();
            stats.capture_us.store(capture.as_micros() as u32, Relaxed);
            stats.block.store((data.len() / channels) as u32, Relaxed);
            let mut dropped = 0;
            for frame in data.chunks_exact(channels) {
                let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                if producer.push(sum * gain).is_err() {
                    dropped += 1;
                }
            }
            if dropped > 0 {
                stats.dropped.fetch_add(dropped, Relaxed);
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )
}

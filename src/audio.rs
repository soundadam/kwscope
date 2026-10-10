//! 麦克风输入：cpal 回调里只做下混成单声道并推进无锁环形缓冲，UI 线程每帧取走。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample};
use rtrb::{Consumer, Producer, RingBuffer};

pub struct Input {
    // 持有即在录；drop 即停
    _stream: cpal::Stream,
    pub consumer: Consumer<f32>,
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
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, &config, channels, producer),
        cpal::SampleFormat::I16 => build::<i16>(&device, &config, channels, producer),
        cpal::SampleFormat::U16 => build::<u16>(&device, &config, channels, producer),
        cpal::SampleFormat::I32 => build::<i32>(&device, &config, channels, producer),
        other => return Err(format!("unsupported sample format: {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    Ok(Input {
        _stream: stream,
        consumer,
        sample_rate,
        device_name,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    mut producer: Producer<f32>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    let gain = 1.0 / channels as f32;
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            for frame in data.chunks_exact(channels) {
                let sum: f32 = frame.iter().map(|&s| f32::from_sample(s)).sum();
                let _ = producer.push(sum * gain);
            }
        },
        |err| eprintln!("audio stream error: {err}"),
        None,
    )
}

# Tonescope

实时对数频谱（瀑布图）+ 音阶参考线。Rust · cpal · realfft · egui，单个可执行文件。

## 下载

推 `v*` tag 时，[release workflow](.github/workflows/release.yml) 构建 macOS（通用二进制 `.app`）、Windows（x86_64）、Linux（x86_64）三份产物，发布到 [Releases](https://github.com/soundadam/tonescope/releases)。tag 必须与 `Cargo.toml` 的版本一致。

- macOS：`.app` 只有 ad-hoc 签名、未公证，首次打开要在 Finder 里右键 →「打开」。首次运行会请求麦克风权限。
- Linux：需要系统里有 ALSA（`libasound2`）；PulseAudio / PipeWire 通过 ALSA 兼容层接入。

## 从源码运行

```bash
cargo run --release
```

Linux 构建前先装 ALSA 头文件（Debian / Ubuntu：`sudo apt install libasound2-dev`）。

macOS 下用 `cargo run` 时，麦克风权限记在启动它的那个程序（Terminal / iTerm / IDE）名下。

本地打包与 CI 用同一个脚本，产物放在 `dist/`：

```bash
scripts/package.sh
```

界面为英文，只用 egui 自带字体。

## 现有功能

- 默认输入设备实时采集，顶栏可切换设备；多声道下混为单声道
- 流式 STFT（Hann 窗），FFT 1024–16384、hop 128–2048 可选；幅度为 dBFS（满幅正弦 ≈ 0 dB）
- 对数频率轴瀑布图：低频行在相邻 bin 间插值，高频行取区间最大值；频率范围、dB 上下限、色表（Magma / Viridis / Gray）可调；每帧只给新列着色并局部更新纹理
- 改频率范围时从保留的原始谱重映射历史，暂停后仍可缩放已有画面
- 音阶参考线：半音阶、大调、三种小调、五种调式、大/小五声、布鲁斯、全音阶；主音 12 选 1；A4 400–480 Hz 可调；主音高亮，标签按间距自动避让
- 音高跟踪（YIN，40–2000 Hz）：每列瀑布图对应一个音高，在图上画成绿色轨迹；顶栏读数为当前音名 + 音分偏差 + Hz。
  基音弱于泛音时仍报基音，不跳八度。音高窗取自同一 FFT 帧的中央，FFT 越小可测最低音越高（侧栏有显示）
- 电平门限：帧 RMS 低于门限不测音高；右侧频谱上同时标出最强频率（琥珀）与音高（绿）
- 鼠标悬停：频率、最近音名与音分、该点 dB、距今时间、该列音高；可选显示 2–8 次泛音线
- 右侧实时频谱曲线，与瀑布图共用频率轴
- 空格暂停 / 继续
- 侧栏 Performance：帧率、每帧 CPU（着色 / DSP 分项）、DSP 占实时比例、声音到屏幕的延迟估计（设备 / 排队 / 分析窗 / 显示分项）、丢样本计数；各项悬停有说明

## 待做

- [ ] 打开 WAV / FLAC 文件离线分析，可拖动时间轴
- [ ] 导出当前画面 PNG、录制最近 N 秒为 WAV
- [ ] 输入电平表与削波提示
- [ ] 非十二平均律：纯律、五度相生律、自定义 cents 表
- [ ] 可选 CQT / 重分配谱，提升低频分辨率

## 许可证

[MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)，任选其一。

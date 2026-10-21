# Kwscope

唤醒词（KWS）模型的测试台：导入 ONNX bundle，对着麦克风说或打开一段录音，在频谱上看一级在哪儿触发、
二级收下还是拒掉。本身不带任何模型。Rust · cpal · realfft · egui · ONNX Runtime，单个可执行文件。

## 安装

macOS 与 Linux（x86_64）用 Homebrew：

```bash
brew trust --tap soundadam/tap
brew install --cask soundadam/tap/kwscope
```

Windows 从 [Releases](https://github.com/soundadam/kwscope/releases) 下载 zip。

- macOS：`.app` 只有 ad-hoc 签名、未公证。Homebrew 装的直接能打开；从 Releases 下载的 zip 首次打开会被拦下，
  到「系统设置 → 隐私与安全性」里点「仍要打开」，或者自己去掉隔离属性：

  ```bash
  xattr -dr com.apple.quarantine <解压出来的 Kwscope.app>
  ```

  首次运行会请求麦克风权限。
- macOS：Apple Silicon、13.4 及以上。ONNX Runtime 已不出 Intel Mac 的库，所以没有 x86_64 版。
- Linux：glibc 2.38 及以上（Ubuntu 24.04、Debian 13 起）；需要系统里有 ALSA（`libasound2`），PulseAudio / PipeWire 通过 ALSA 兼容层接入。

ONNX Runtime 静态链接进可执行文件，不用另装。

## 导入 bundle

一级 bundle 必须有，二级可选；只导一级时每个触发都算收下。三种导法，效果相同：

- 侧栏 **Bundles → Import folder…** / **Import zip…**；
- 把文件夹、`.zip` 或 bundle 的 json 拖进窗口（拖进来的 `.wav` / `.flac` 则是打开录音）；
- 启动时带上：`kwscope --import export/stage_one --import export/stage_two`。

文件夹与 zip 里往下找两层，找到哪个认哪个，所以 `kws export` 的整个输出目录打个 zip 直接拖进来就行。
再导一次同一级就替换旧的；侧栏 ✖ 卸下。两级的关键词或采样率对不上时整次导入作废，原来的模型不动。

导入后：

- 麦克风或录音先重采样到 bundle 的采样率（通常 16 kHz），模型和频谱都读这一份；
- 一级逐帧分数画在频谱下方，琥珀虚线是 t1；
- 每个触发在频谱上标 `[start_sample, end_sample)`：收下的是绿色实底，二级往前多读的那段（导程）是更淡的绿；
  拒掉的是红色虚线框；框上写二级 logit（没有二级时写一级分数）；
- 侧栏 t1 / t2 滑块改阈值（默认 0.33140625 与 1.2），检测参数是 det.v8 的 window 50 / lockout 80 / warmup 150 帧。
  打开的录音改阈值即时重判，一级分数与二级 logit 都有缓存。

频谱是显示用的，和模型的前端无关（模型读什么特征只看 bundle）：

- 侧栏 **Frequency** 选纵轴：Mel（默认，0 Hz 起）、Log（60 Hz 起，每个八度等高）、Linear；刻度跟着轴换；
- 侧栏 **Resolution** 默认 Multi：每 10 ms 一列同时算三个窗，中心对齐在同一时刻，所以列和一级的帧、
  触发的框还在同一条时间轴上——500 Hz 以下用 96 ms 窗（看得清基频与低次谐波），500 Hz–2 kHz 用 32 ms，
  2 kHz 以上用 16 ms（辅音起止更利落），交界处在 dB 上做 1/3 八度的交叉渐变，同一个正弦在哪一段读数都一样。
  Single 32 ms 只用中间那个窗；
- 行数跟着频谱在屏幕上的物理像素高度走（HiDPI 下更密，最多 1024 行），改窗口大小、换轴、换分辨率都不用重算，
  直播时每帧只给新进来的几列着色。

录音可拖动时间轴、滚轮缩放、双击回到全貌；多声道文件在侧栏选声道。**Export JSONL** 写出和 `kws stream`
一样的行（多声道时带 `channel`）。

## bundle 格式

bundle 是一个目录，里面一个 json 加它点名的文件；json 的 `format` 字段决定用哪个 adapter
（`src/kws/adapter.rs` 的 `ADAPTERS`）。两种格式都由 kws-train 的 `kws export stage-one` / `stage-two` 写出，
契约以那边 `docs/stage-one-runtime.md` 为准，这里只列 kwscope 读的。

### `kws_stage_one.v1`（`stage_one.json`）

| 字段 | 含义 |
| --- | --- |
| `sample_rate`、`n_fft`、`hop_length` | 帧网格：第 `t` 帧是从 `t·hop` 起的 `n_fft` 个样本（不居中），最后一个样本到了这帧就出 |
| `keywords` | 关键词列表，`K` 个 |
| `feature_dim`、`log_offset` | 网络输入维数；取对数前加的偏移 |
| `receptive_field` | 感受野帧数；起点偏移头的单位 |
| `cache_size` | 流式缓存长度 |

- `frontend.npz`：`window`（补零到 `n_fft`）、`filterbank`（bands × (n_fft/2+1)）、`dct`；可选 `c0_weights`
  （`c0` 减去自己的因果滑动均值）与 `post`（之后乘的矩阵，log-mel 前端是逆 DCT）。运行时只用这几张表，不自己推公式。
- `stage_one.onnx`：输入 `feats` (1, T, feature_dim) 与 `cache_in` (1, 1, cache_size)，输出 `logits` (1, T, 2K)
  与 `cache_out`。前 `K` 列过 sigmoid 是分数，后 `K` 列乘 `receptive_field` 是词起点在判定帧之前几帧。

一个触发的 `end_sample = decision_frame·hop + n_fft`，`start_sample = round((decision_frame − offset)·hop)`，
offset 截在 `[0, receptive_field]`。

### `kws_stage_two.v2`（`stage_two.json`）

| 字段 | 含义 |
| --- | --- |
| `sample_rate`、`keywords` | 须和一级一致（二级的关键词要包含一级的） |
| `members[]` | 一个或多个检查器（集成取均值），每个有 `model`（onnx）、`frontend`（npz）、`n_fft`、`hop_length`、`feature_dim`、`log_offset`、`cmn`、`leads`、`fixed`、`min_samples` |
| `scoring` | `disc`、`phone`、`phones`、`blank`、`floor`：logit = `disc`·判别 logit + `phone`·音素路径分 |

每个成员对每个导程 `lead` 读一段 `[start_sample − lead, end_sample)`（`fixed` 非空时改为 `end_sample − fixed` 起），
开机处截住、至少 `min_samples`；算 log-mel，`cmn` 时减每带均值；`model` 输入 `feats` (1, T, dim)，
输出 `logits`（每关键词一个），`phone ≠ 0` 时还要 `phones`。成员内对导程取均值，成员间再取均值，
`logit ≥ t2` 就收下。

## 命令行与更新

```bash
kwscope                                    # 打开界面，听默认麦克风
kwscope --import <bundle> --open <wav>     # 启动时导入并打开录音
kwscope stream <wav>... --bundle <stage_one> [--stage-two <stage_two>] [--threshold 0.33140625] [--t2 1.2] [--channel N]
kwscope update                             # 更新到最新版
kwscope update --check                     # 只看有没有新版
kwscope update --auto on|off               # 打开或关掉自动检查
kwscope --version
```

`kwscope stream` 不开窗口，每个触发打一行 JSON 到 stdout，字段、顺序和浮点写法都同 kws-train 的 `kws stream`；
差别只在它会先把录音重采样到 bundle 的采样率。

Homebrew 装的升级交给 brew，从 Releases 下载的 `.app` 或可执行文件原地替换自己。界面每天至多查一次新版，有新版时顶栏右侧出现 Update 按钮。怎么判断安装方式、会发哪些网络请求见 [appkit](https://github.com/soundadam/appkit#更新)。

## 从源码运行

```bash
cargo run --release
```

Linux 构建前先装 ALSA 头文件（Debian / Ubuntu：`sudo apt install libasound2-dev`）。首次构建时 `ort` 会下载 ONNX Runtime 预编译库。

macOS 下用 `cargo run` 时，麦克风权限记在启动它的那个程序（Terminal / iTerm / IDE）名下。

`cargo test` 里的 `tests/golden.rs` 拿 kws-train 运行时写下的特征、logit 与触发逐帧对，来源见
[`tests/fixtures/README.md`](tests/fixtures/README.md)。

本地打包与 CI 用同一个脚本，产物放在 `dist/`：

```bash
scripts/package.sh
```

界面为英文，只用 egui 自带字体。

## 发布

产物命名、Cask 与命令行照 [appkit](https://github.com/soundadam/appkit) 的约定。推 `v*` tag（必须与 `Cargo.toml` 的版本一致），[release workflow](.github/workflows/release.yml) 构建 macOS（Apple Silicon 的 `.app`，文件名仍是 appkit 约定的 `macos-universal.zip`）、Windows（x86_64）、Linux（x86_64）三份产物发到 Releases，再用 [`packaging/homebrew/kwscope.rb`](packaging/homebrew/kwscope.rb) 填好版本与 SHA-256 推到 [soundadam/homebrew-tap](https://github.com/soundadam/homebrew-tap)。带 `-` 的预发布 tag 不更新 Cask。

[`site/`](site/) 是 [soundadam.com/projects/kwscope](https://soundadam.com/projects/kwscope/) 的文案与截图，站点以 Hugo module 引入；main 上 `site/` 一变，[site workflow](.github/workflows/site.yml) 就通知站点重新发布。

## 许可证

[MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)，任选其一。

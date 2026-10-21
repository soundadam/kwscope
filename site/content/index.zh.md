---
title: Kwscope
excerpt: 唤醒词模型的测试台。导入 ONNX bundle，对着麦克风说或打开一段录音，每个触发都标在频谱上。
weight: 40
presentation:
  category: 语音 / macOS + Linux + Windows
  label: 用 Homebrew 安装
  command:
    - brew trust --tap soundadam/tap
    - brew install --cask soundadam/tap/kwscope
  note: 单个可执行文件。不带模型。从 GitHub Releases 自动更新。
registry:
  github: soundadam/kwscope
  homebrew: soundadam/tap/kwscope
  docs: https://github.com/soundadam/kwscope#readme
release:
  version: "0.1.0"
  license: MIT OR Apache-2.0
modules:
  - type: promo
    description: 拖进一个一级唤醒词 bundle，愿意的话再加一个二级检查器，然后对着麦克风说，或者打开一段 wav / flac。{title} 照训练侧自己的运行时跑两级，把每个触发标在对数频率的频谱上，下方是逐帧分数。
    line: "{title} {version} 以 {license} 开源，用 Rust 写成，ONNX Runtime 静态链接，macOS、Linux、Windows 各一个原生可执行文件。"
    actions:
      - label: 文档
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: 和训练侧一样的数
    body: "{title} 读 kws-train 导出的 bundle，复现它的流式运行时：前端按 bundle 自带的表算，网络带缓存逐块前向，检测器和二级读的区间都一样。测试拿一段录音，把特征、logit 和触发逐帧对那边的运行时。"
    note: 用哪个模型看 bundle json 的 format 字段，新的模型族就是多一个 adapter。
    metrics:
      - value: "2"
        label: bundle 格式：一级、二级
      - value: "100 ms"
        label: 每次推入，同 kws stream
      - value: "16 kHz"
        label: 输入重采样到 bundle 的采样率
      - value: "{version}"
        label: "当前版本 · {license}"

  - type: section
    title: 每个触发落在哪儿
    index: 1/3
    lede: 收下的触发是一条绿底，盖住一级说是唤醒词的那段样本，二级往前多读的部分是更淡的绿；拒掉的是红色虚线框。每个都标着二级 logit，下方的分数曲线上能看到它越过的 t1 线。

  - type: media
    image: triggers.webp
    alt: Kwscope 打开一段 42 秒的朗读录音：收下的触发是绿色阴影，拒掉的是红色虚线框，上方标着二级 logit，下方是一级分数曲线和阈值线
    caption: "在 Linux 上的 {title} {version} 里导入测试用的两个 bundle（权重未训练，触发本身没有意义）、打开 42 秒 THCHS-30 朗读录音：收下九个，拒掉两个。"

  - type: section
    title: 拖阈值，马上看结果
    index: 2/3
    lede: 打开的录音上拖 t1 / t2 滑块立即重判，一级分数和二级 logit 都有缓存。时间轴可拖动、滚轮缩放，多声道文件可选声道，触发可导出成和 kws stream 一样的 JSON 行。

  - type: section
    title: 实时，或者不开窗口
    index: 3/3
    lede: 接麦克风时频谱滚动，触发一出就标上，侧栏显示二级的耗时。不开窗口时用 kwscope stream 跑一批文件，每个触发打一行 JSON，可以直接和训练侧的输出比对。
---

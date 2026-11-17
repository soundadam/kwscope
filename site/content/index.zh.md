---
title: Kwscope
excerpt: 语音唤醒模型的测试台：导入 ONNX 模型，对着麦克风说或打开录音，每次触发都标在频谱上。
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
  version: "0.2.0"
  license: MIT OR Apache-2.0
modules:
  - type: promo
    description: "{title} 是语音唤醒（KWS）模型的测试台。导入一级模型和可选的二级复核器，对着麦克风说或打开一段 wav / flac，每次触发都标在频谱上，下方是逐帧分数。"
    line: "{title} {version} 以 {license} 开源，用 Rust 写成，macOS、Linux、Windows 各一个可执行文件。"
    actions:
      - label: 文档
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: 和训练侧逐帧一致
    body: "{title} 复现训练侧的流式运行时：同样的前端、带缓存的逐块前向、同样的检测器和二级读取区间。测试用一段录音逐帧比对特征、logit 和触发。"
    note: "{title} 本身不带模型，唤醒模型的权重目前尚未开源。"
    metrics:
      - value: 两级
        label: 一级听词，二级复核
      - value: 逐帧
        label: 和训练侧的运行时对齐
      - value: "{version}"
        label: "当前版本 · {license}"

  - type: section
    title: 触发标在哪里
    index: 1/3
    lede: 绿色是收下的唤醒，红色虚线框是二级拦下的误报，上面的数字是二级的把握。下方是一级的分数曲线和阈值线。

  - type: media
    image: triggers.webp
    alt: Kwscope 在 MacBook Pro 上接麦克风：频谱上三段绿色阴影是收下的唤醒，一个红色虚线框是拦下的触发，上方标着二级 logit，下方是一级分数曲线和阈值线
    caption: "{title} {version} 在 MacBook Pro 上接麦克风实测：四次触发，收下三次，二级拦下一次。"

  - type: section
    title: 拖动阈值，即时重判
    index: 2/3
    lede: 拖动 t1、t2，打开的录音立即重判，可以看到哪些触发多出、哪些消失。

  - type: section
    title: 实时，或批量
    index: 3/3
    lede: 接上麦克风时频谱随声音滚动，触发一出就标上。批量跑录音用 `kwscope stream`，每个触发一行 JSON。
---

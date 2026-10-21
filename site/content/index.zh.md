---
title: Kwscope
excerpt: 语音唤醒模型的测试台。对着麦克风说一句唤醒词，看模型在哪一刻醒、为什么醒。
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
    description: 说一句“嘿 Siri”，手机就醒了，这一步叫[语音唤醒](https://machinelearning.apple.com/research/voice-trigger)（KWS）。{title} 是唤醒模型的测试台：对着麦克风说，或者打开一段录音，模型每一次醒来，都落在频谱上。
    line: "{title} {version} 以 {license} 开源，眼下用在我们内部的唤醒模型测试上。不远的将来，我们会放出一个开源权重的唤醒模型，装上就能试。"
    actions:
      - label: 文档
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: 看到的，就是模型真正的判断
    body: 唤醒模型好不好，最后都落在一个个具体的时刻：该醒的时候醒没醒，不该醒的时候有没有误醒。{title} 和训练时用的运行时逐帧对齐，屏幕上的每一次触发，都是模型上线后真的会做出的判断，不是近似。
    note: "{title} 本身不带模型。开源权重的唤醒模型发布后，会在这里给出下载。"
    metrics:
      - value: 两级
        label: 一级听词，二级复核
      - value: 逐帧
        label: 和训练侧的运行时对齐
      - value: "{version}"
        label: "当前版本 · {license}"

  - type: section
    title: 每一次醒来都有位置
    index: 1/3
    lede: 绿色是收下的唤醒，红色虚线框是二级拦下的误报，上面的数字是二级的把握。下方是一级的分数曲线，越过黄色虚线的那一刻，模型醒了。

  - type: media
    image: triggers.webp
    alt: Kwscope 在 MacBook Pro 上接麦克风：频谱上三段绿色阴影是收下的唤醒，一个红色虚线框是拦下的触发，上方标着二级 logit，下方是一级分数曲线和阈值线
    caption: "{title} {version} 在 MacBook Pro 上接麦克风实测：四次触发，收下三次，二级拦下一次。"

  - type: section
    title: 拖一下阈值，就知道该定在哪儿
    index: 2/3
    lede: 唤醒永远在两种失败之间取舍：太灵敏会误醒，太迟钝会叫不应。拖动 t1、t2，打开的录音马上重判一遍，哪一次多出来、哪一次消失了，一眼就能看到。

  - type: section
    title: 在麦克风前，也在流水线里
    index: 3/3
    lede: 最好的测试是自己开口说。接上麦克风，频谱随声音滚动，唤醒一出就标上。要成批跑录音时，用 `kwscope stream`，每个触发一行 JSON，直接接进评测。
---

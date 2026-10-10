---
title: Tonescope
excerpt: 实时频谱瀑布图，频率轴按音高取对数，音阶参考线和音高轨迹直接画在声音上。
weight: 40
presentation:
  category: 声学 / macOS + Linux + Windows
  label: 用 Homebrew 安装
  command: brew install --cask soundadam/tap/tonescope
  note: 单个可执行文件。不要账号。不联网。
registry:
  github: soundadam/tonescope
  homebrew: soundadam/tap/tonescope
  docs: https://github.com/soundadam/tonescope#readme
release:
  version: "0.1.1"
  license: MIT OR Apache-2.0
modules:
  - type: promo
    description: 对着麦克风唱、弹或者说话，声音在对数频率轴上滚成瀑布图：每个八度一样高，每个音落在自己的音阶线上。{title} 把听到的音高画成一条线穿过画面，同时读出音名和音分偏差。
    line: "{title} {version} 以 {license} 开源，用 Rust 写成，macOS、Linux、Windows 各一个原生可执行文件。"
    actions:
      - label: 文档
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: 为什么用对数频率轴
    body: 线性频谱图把一半高度给了最高的那个八度，人声和乐器真正发声的频段被挤成底下一条。音高本来就是对数的，所以 {title} 给每个八度同样的高度：低频行在相邻 FFT bin 之间插值，高频行取该像素行内的最大值。所选调式的音阶线和行对齐，读音名就像在键盘上找键。
    note: 音高跟踪用 YIN，测量范围 40–2000 Hz。FFT 越小，能测的最低音越高；侧栏显示当前下限。
    metrics:
      - value: "14"
        label: 种音阶与调式，主音 12 选 1
      - value: "40–2000 Hz"
        label: 音高跟踪范围
      - value: "1024–16384"
        label: FFT 长度，hop 128–2048
      - value: "{version}"
        label: "当前版本 · {license}"

  - type: section
    title: 看到的是音，不只是频率
    index: 1/3
    lede: 选好主音和音阶（大调、三种小调、五种调式、五声、布鲁斯、全音阶），{title} 把音阶线横贯瀑布图，主音高亮，A4 可在 400–480 Hz 间调。鼠标悬停任一点，显示频率、最近音名与音分、该点 dB、距今多久、该列音高；打开泛音线，还能看到该点 2–8 次泛音落在哪里。

  - type: media
    image: waterfall.webp
    alt: Tonescope 画出一段旋律：音符排成阶梯，上方是各次泛音，绿色音高轨迹沿基音走，右侧是带音名的半音阶参考线和实时频谱曲线
    caption: "一段带 8 次泛音的合成旋律，经虚拟麦克风输入 Linux 上的 {title} {version}。绿线是跟踪到的音高，悬停框读的是光标下那一点。"

  - type: section
    title: 报基音，不报最响的泛音
    index: 2/3
    lede: 泛音比基音还响时，音高读数仍停在基音上，不跳八度。电平低于门限的帧不测音高，静音不会变成一条乱跑的线。右侧实时频谱同时标出最强频率和检测到的音高，两者不一致时一眼可见。

  - type: section
    title: 快到可以跟着唱
    index: 3/3
    lede: 每帧只给新列着色，就地更新纹理。帧率跟随显示器，可限 60 或 30 fps，输入持续安静时自动降帧。Performance 面板拆出每帧 CPU，并估算声音到屏幕的延迟，分为设备、排队、分析窗、显示四段。空格暂停，暂停后的画面仍可改频率范围、缩放。
---

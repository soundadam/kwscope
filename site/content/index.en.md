---
title: Kwscope
excerpt: A test bench for wake-word models. Say the word into the microphone and see the moment the model wakes, and why.
weight: 40
presentation:
  category: Speech / macOS + Linux + Windows
  label: Install with Homebrew
  command:
    - brew trust --tap soundadam/tap
    - brew install --cask soundadam/tap/kwscope
  note: One executable. No models bundled. Updates itself from GitHub Releases.
registry:
  github: soundadam/kwscope
  homebrew: soundadam/tap/kwscope
  docs: https://github.com/soundadam/kwscope#readme
release:
  version: "0.2.0"
  license: MIT OR Apache-2.0
modules:
  - type: promo
    description: Say "Hey Siri" and the phone wakes up. That step is [keyword spotting](https://machinelearning.apple.com/research/voice-trigger), or KWS. {title} is a test bench for wake-word models. Talk into the microphone or open a recording, and every time the model wakes, you see it on the spectrogram.
    line: "{title} {version} is open source under {license}. Today it is what we use to test our wake-word models in house. Soon we will release an open-weight wake-word model you can load and try."
    actions:
      - label: Docs (Chinese)
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: What you see is what the model decides
    body: A wake-word model is judged one moment at a time. Did it wake when it should have? Did it stay quiet when it should have? {title} matches the training-side runtime frame by frame, so every trigger on screen is a decision the model would really make once it ships, not an approximation.
    note: "{title} ships without a model. When the open-weight wake-word model is out, it will be linked from here."
    metrics:
      - value: Two stages
        label: Stage one listens, stage two double-checks
      - value: Frame by frame
        label: Matches the training-side runtime
      - value: "{version}"
        label: "Current release · {license}"

  - type: section
    title: Every wake has a place
    index: 1/3
    lede: Green is a wake that was accepted. A red dashed box is a false alarm stage two caught. The number above each is stage two's confidence. Below runs stage one's score; the moment it crosses the yellow dashed line, the model wakes.

  - type: media
    image: triggers.webp
    alt: Kwscope on a MacBook Pro listening to the microphone, with three green shaded accepted triggers and one red dashed rejected trigger on the spectrogram, stage-two logits above each, and the stage-one score curve with its threshold line below
    caption: "{title} {version} on a MacBook Pro, listening to the microphone: four triggers, three accepted, one caught by stage two."

  - type: section
    title: Move a threshold, find where it belongs
    index: 2/3
    lede: Wake words always trade one failure for another. Too eager and it wakes on its own; too cautious and it ignores you. Drag t1 or t2 and the open recording is judged again at once, so you see which wakes appear and which ones vanish.

  - type: section
    title: At the microphone, and in the pipeline
    index: 3/3
    lede: The best test is to speak. With the microphone on, the spectrogram scrolls as you talk and each wake is marked as it fires. For a batch of recordings, `kwscope stream` prints one JSON line per trigger, ready for your evaluation.
---

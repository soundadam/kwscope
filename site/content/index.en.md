---
title: Kwscope
excerpt: "A test bench for wake-word models: import ONNX bundles, speak or open a recording, and see each trigger marked on the spectrogram."
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
  license: Apache-2.0
modules:
  - type: promo
    description: "{title} is a test bench for keyword-spotting (KWS) models. Import a stage-one model and an optional stage-two checker, talk into the microphone or open a wav or flac file, and each trigger is marked on the spectrogram above the frame-by-frame score."
    line: "{title} {version} is open source under {license}: one native executable for macOS, Linux and Windows, written in Rust."
    actions:
      - label: Docs (Chinese)
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: The same numbers as the training side
    body: "{title} reproduces the training side's streaming runtime: the same front end, the same cached streaming forward pass, the same detector and stage-two spans. Its tests compare features, logits and triggers frame by frame on a recorded clip."
    note: "{title} ships without a model; the wake-word model weights are not open yet."
    metrics:
      - value: Two stages
        label: Stage one listens, stage two double-checks
      - value: Frame by frame
        label: Matches the training-side runtime
      - value: "{version}"
        label: "Current release · {license}"

  - type: section
    title: Where each trigger lands
    index: 1/3
    lede: Green is a wake that was accepted. A red dashed box is a false alarm stage two caught. The number above each is stage two's confidence. Below runs stage one's score with its threshold line.

  - type: media
    image: triggers.webp
    alt: Kwscope on a MacBook Pro listening to the microphone, with three green shaded accepted triggers and one red dashed rejected trigger on the spectrogram, stage-two logits above each, and the stage-one score curve with its threshold line below
    caption: "{title} {version} on a MacBook Pro, listening to the microphone: four triggers, three accepted, one caught by stage two."

  - type: section
    title: Move a threshold, re-run at once
    index: 2/3
    lede: Drag t1 or t2 and the open recording is judged again at once, so you see which triggers appear and which vanish.

  - type: section
    title: Live, or in batch
    index: 3/3
    lede: With the microphone on, the spectrogram scrolls and each trigger is marked as it fires. For a batch of recordings, `kwscope stream` prints one JSON line per trigger.
---

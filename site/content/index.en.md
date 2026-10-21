---
title: Kwscope
excerpt: A test bench for wake-word models. Import ONNX bundles, speak or open a recording, and see every trigger shaded on the spectrogram.
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
    description: Drop in a stage-one keyword-spotting bundle, and optionally a stage-two checker, then talk into the microphone or open a wav or flac file. {title} runs both stages the way the training side's own runtime does and marks each trigger on a log-frequency spectrogram, with the frame-by-frame score underneath.
    line: "{title} {version} is open source under {license}, a single native executable for macOS, Linux and Windows written in Rust, with ONNX Runtime linked in."
    actions:
      - label: Docs (Chinese)
        url: "{docs}"
      - label: GitHub
        url: "https://github.com/{github}"

  - type: snapshot
    title: The same numbers as the training side
    body: "{title} reads the bundles kws-train exports and reproduces its streaming runtime: the same front end from the bundle's own tables, the same cached streaming forward pass, the same detector and the same stage-two spans. Its tests compare features, logits and triggers frame by frame against that runtime on a recorded clip."
    note: Bundles are chosen by the format field in their json, so a new model family is one more adapter.
    metrics:
      - value: "2"
        label: Bundle formats, stage one and stage two
      - value: "100 ms"
        label: Pushes, as in kws stream
      - value: "16 kHz"
        label: Input resampled to the bundle's rate
      - value: "{version}"
        label: "Current release · {license}"

  - type: section
    title: Every trigger where it happened
    index: 1/3
    lede: An accepted trigger is a green band over the samples stage one says hold the keyword, with a lighter band for the extra audio stage two reads before it. A rejected one is a red dashed box. Each is labelled with stage two's logit, and the score curve below shows the t1 line it crossed.

  - type: media
    image: triggers.webp
    alt: Kwscope showing a 42-second read-speech recording with green shaded accepted triggers, a red dashed rejected trigger, stage-two logits above each, and the stage-one score curve with its threshold line below
    caption: "{title} {version} on Linux with the test bundles imported (untrained weights, so the triggers mean nothing) and 42 seconds of THCHS-30 read speech opened: nine triggers accepted, two rejected."

  - type: section
    title: Move the thresholds, see the answer
    index: 2/3
    lede: The t1 and t2 sliders re-run detection on an open recording at once, from cached scores and cached stage-two logits. Drag the time axis, zoom with the wheel, pick a channel of a multichannel file, and export the triggers as JSON lines in the same format as kws stream.

  - type: section
    title: Live, or headless
    index: 3/3
    lede: On the microphone the spectrogram scrolls and triggers appear as they fire, with stage two's run time in the sidebar. Without a window, kwscope stream runs the bundles over files and prints one JSON line per trigger, ready to diff against the training side's output.
---

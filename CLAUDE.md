# Tonescope

本地门槛（每个 commit 前）：

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

模块：`audio` 采集 → `dsp` STFT/峰值 → `waterfall` 对数行映射与着色 → `app` UI；`pitch` YIN 音高，`music` 音名与音阶，`axis` 对数轴，`fonts` 系统中文字体。

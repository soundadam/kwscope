# Kwscope

本地门槛（每个 commit 前）：

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

只改 `site/` 或 Markdown 的 commit 不用跑；CI 也只跑 `check`，跳过三平台测试。

库（`src/lib.rs` → `kws`）：`adapter` 按 bundle json 的 `format` 选模型 → `stage_one` / `stage_two` 各一个格式，
`frontend` 前端、`onnx` 会话、`npy` 读表 → `detector` det 检测器 → `engine` 两级串起来（流式 `Engine`、离线 `Analysis`）
→ `jsonl` 写 `kws stream` 的行；`media` 解码 wav/flac。

可执行文件：`audio` 采集 → `dsp` 重采样 / STFT → `spectrogram` 对数行映射与着色 → `worker` 线程里跑模型 → `app` UI；
`cli` 是 `kwscope stream`。

这里的数要和 kws-train 的 `kws_infer` 逐帧一致：改 `kws/` 之后 `tests/golden.rs` 必须还过，
不放宽容差；金标准怎么来的见 `tests/fixtures/README.md`。

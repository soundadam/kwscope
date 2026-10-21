//! KWS 运行时：`kws_infer` 的 Rust 移植，加上导入层。

pub mod adapter;
pub mod detector;
pub mod engine;
pub mod frontend;
pub mod jsonl;
pub mod media;
pub mod npy;
pub mod onnx;
pub mod stage_one;
pub mod stage_two;

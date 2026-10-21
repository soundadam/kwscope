//! ONNX Runtime 会话：和 `kws_infer` 同样的单线程设置，跑同一张图同样的数。

use anyhow::{anyhow, Context, Result};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::OnceLock;

/// 一个进程只许一个 ONNX Runtime 环境，建一次留到退出。
fn environment() -> Result<&'static Environment> {
    static ENV: OnceLock<Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("kwscope")
            .build()
            .map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| anyhow!("onnxruntime: {e}"))
}

pub fn session(path: &Path) -> Result<Session> {
    // 一次推送就几帧：多开线程只多出交接
    let fail = |e: &dyn std::fmt::Display| anyhow!("onnxruntime: {e}");
    let mut builder = Session::builder(environment()?)
        .map_err(|e| fail(&e))?
        .with_intra_threads(1)
        .map_err(|e| fail(&e))?
        .with_inter_threads(1)
        .map_err(|e| fail(&e))?;
    builder
        .commit_from_file(path)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("load {}", path.display()))
}

pub fn tensor(shape: &[usize], data: Vec<f32>) -> Result<Tensor<f32>> {
    let shape: Vec<i64> = shape.iter().map(|&d| d as i64).collect();
    Tensor::from_array((shape, data)).map_err(|e| anyhow!("{e}"))
}

/// 会话的输出名，按图里的顺序。
pub fn output_names(session: &Session) -> Vec<String> {
    session
        .outputs()
        .iter()
        .map(|o| o.name().to_owned())
        .collect()
}

//! 金标准：kws-train 自己的运行时（`kws_infer`、`kws stream`）在同一段 THCHS-30 录音上
//! 写下的数，这里逐帧逐触发对。来源与重做见 `tests/fixtures/README.md`。

use kwscope::kws::adapter::{self, Model, StageOneModel};
use kwscope::kws::detector::DetectorConfig;
use kwscope::kws::engine::{self, Analysis, Engine, Judge, Trigger};
use kwscope::kws::stage_one::KwsStageOneV1;
use kwscope::kws::stage_two::KwsStageTwoV2;
use kwscope::kws::{jsonl, media, npy};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 特征：f64 算、转 f32，两边只差在求和次序上。
const FEATURE_TOL: f64 = 1e-4;
/// 网络输出：同一张图，不同的 ORT 构建。
const LOGIT_TOL: f64 = 1e-3;
const SCORE_TOL: f64 = 1e-5;
const T2: f64 = 0.455;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn clip() -> Vec<f32> {
    let audio = media::decode(&fixtures().join("thchs30_clip.wav")).unwrap();
    assert_eq!((audio.sample_rate, audio.channels.len()), (16_000, 1));
    audio.channels.into_iter().next().unwrap()
}

fn stage_one() -> KwsStageOneV1 {
    KwsStageOneV1::open(&fixtures().join("bundles/stage_one")).unwrap()
}

fn stage_two() -> KwsStageTwoV2 {
    KwsStageTwoV2::open(&fixtures().join("bundles/stage_two")).unwrap()
}

fn max_diff(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

fn as_f64(v: &[f32]) -> Vec<f64> {
    v.iter().map(|&x| x as f64).collect()
}

#[test]
fn stage_one_features_and_logits_match_kws_infer() {
    let samples = clip();
    let mut model = stage_one();
    let step = engine::step_samples(16_000, engine::CHUNK_MS);
    let want_feats = npy::load(&fixtures().join("stage_one_feats.npy")).unwrap();
    let want_logits = npy::load(&fixtures().join("stage_one_logits.npy")).unwrap();
    let dim = want_feats.shape[1];

    let mut feats = vec![];
    let mut logits = vec![];
    for chunk in samples.chunks(step) {
        let f = model.frontend.push(chunk);
        logits.extend(model.forward(&f).unwrap());
        feats.extend(f);
    }
    assert_eq!(feats.len() / dim, want_feats.shape[0], "frame count");
    let d = max_diff(&as_f64(&feats), &want_feats.data);
    assert!(d < FEATURE_TOL, "features differ by {d}");
    let d = max_diff(&as_f64(&logits), &want_logits.data);
    assert!(d < LOGIT_TOL, "logits differ by {d}");

    // 同一串 Python 特征喂网络：把网络和前端分开钉
    model.reset();
    let rows: Vec<f32> = want_feats.data.iter().map(|&v| v as f32).collect();
    let mut from_python = vec![];
    let mut at = 0;
    for chunk in samples.chunks(step) {
        let n = model.frontend.push(chunk).len() / dim;
        from_python.extend(model.forward(&rows[at * dim..(at + n) * dim]).unwrap());
        at += n;
    }
    let d = max_diff(&as_f64(&from_python), &want_logits.data);
    assert!(d < LOGIT_TOL, "logits on Python's features differ by {d}");
}

#[test]
fn stage_two_spans_match_kws_infer() {
    let samples = clip();
    let mut model = stage_two();
    let spans: Value =
        serde_json::from_slice(&std::fs::read(fixtures().join("stage_two_spans.json")).unwrap())
            .unwrap();
    for span in spans.as_array().unwrap() {
        let i = span["index"].as_u64().unwrap();
        let (lo, hi) = (
            span["lo"].as_u64().unwrap() as usize,
            span["hi"].as_u64().unwrap() as usize,
        );
        let want = npy::load(&fixtures().join(format!("stage_two_feats_{i}.npy"))).unwrap();
        let feats = model.members[0].features(&samples[lo..hi]);
        let d = max_diff(&as_f64(&feats), &want.data);
        assert!(d < FEATURE_TOL, "span {i}: features differ by {d}");
        let logit = model.span_score(0, 0, "NIHAO_KAKA", feats).unwrap();
        let want_logit = span["logits"][0].as_f64().unwrap();
        assert!(
            (logit - want_logit).abs() < LOGIT_TOL,
            "span {i}: logit {logit} against {want_logit}"
        );
    }
}

fn expected(name: &str) -> Vec<Value> {
    std::fs::read_to_string(fixtures().join(name))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn assert_triggers(got: &[Trigger], want: &[Value], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: trigger count");
    for (g, w) in got.iter().zip(want) {
        let line: Value = serde_json::from_str(&jsonl::line("thchs30_clip.wav", g, None)).unwrap();
        let at = format!("{what}, decision {}", g.decision_frame);
        for key in [
            "wav",
            "keyword",
            "arm_frame",
            "decision_frame",
            "end_sample",
            "accepted",
        ] {
            assert_eq!(line[key], w[key], "{at}: {key}");
        }
        let close = |key: &str, tol: f64| {
            let (a, b) = (line[key].as_f64().unwrap(), w[key].as_f64().unwrap());
            assert!((a - b).abs() < tol, "{at}: {key} {a} against {b}");
        };
        // 起点由网络回归的偏移取整而来：偏移差在 LOGIT_TOL 内，取整后至多差一个样本
        let (a, b) = (
            line["start_sample"].as_i64().unwrap(),
            w["start_sample"].as_i64().unwrap(),
        );
        assert!((a - b).abs() <= 1, "{at}: start_sample {a} against {b}");
        close("score", SCORE_TOL);
        close("start_offset_frames", LOGIT_TOL * 131.0);
        close("stage_two_logit", LOGIT_TOL);
        let keys: Vec<_> = line.as_object().unwrap().keys().collect();
        let want_keys: Vec<_> = w.as_object().unwrap().keys().collect();
        assert_eq!(keys, want_keys, "{at}: fields");
    }
}

#[test]
fn triggers_match_kws_stream() {
    let samples = clip();
    let step = engine::step_samples(16_000, engine::CHUNK_MS);
    for (t1, file) in [
        (0.553, "triggers_t1_0.553.jsonl"),
        (0.603, "triggers_t1_0.603.jsonl"),
    ] {
        let want = expected(file);
        let config = DetectorConfig::det_v8(t1);

        let mut streaming = Engine::new(
            Box::new(stage_one()),
            Some(Judge::new(Box::new(stage_two()), T2)),
            config,
        )
        .unwrap();
        let got = streaming.run(&samples, step).unwrap();
        assert_triggers(&got, &want, &format!("{file} streamed"));

        let mut one = stage_one();
        let analysis = Analysis::run(&mut one, &samples, step, |_| true)
            .unwrap()
            .unwrap();
        let mut judge = Judge::new(Box::new(stage_two()), T2);
        let got = analysis.detect(config, &samples, Some(&mut judge)).unwrap();
        assert_triggers(&got, &want, &format!("{file} offline"));
        // 第二遍全从缓存取，结果不变
        let again = analysis.detect(config, &samples, Some(&mut judge)).unwrap();
        assert_eq!(got.len(), again.len());
        assert!(again.iter().all(|t| t.stage_two_seconds.is_none()));
    }
}

#[test]
fn stage_one_alone_accepts_every_trigger() {
    let samples = clip();
    let mut engine =
        Engine::new(Box::new(stage_one()), None, DetectorConfig::det_v8(0.553)).unwrap();
    let got = engine.run(&samples, 1600).unwrap();
    assert!(!got.is_empty());
    assert!(got
        .iter()
        .all(|t| t.accepted && t.stage_two_logit.is_none()));
    // 没有二级时拒绝不会改耳聋时长：头一个触发同 t1=0.553 的金标准
    let want = &expected("triggers_t1_0.553.jsonl")[0];
    assert_eq!(
        got[0].decision_frame as u64,
        want["decision_frame"].as_u64().unwrap()
    );
}

#[test]
fn imports_both_bundles_from_a_zip() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("golden-import");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let zip_path = scratch.join("bundles.zip");
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for dir in ["stage_one", "stage_two"] {
            for entry in std::fs::read_dir(fixtures().join("bundles").join(dir)).unwrap() {
                let path = entry.unwrap().path();
                let name = format!(
                    "export/{dir}/{}",
                    path.file_name().unwrap().to_string_lossy()
                );
                zip.start_file(name, options).unwrap();
                std::io::copy(&mut std::fs::File::open(&path).unwrap(), &mut zip).unwrap();
            }
        }
        zip.finish().unwrap();
    }
    let models = adapter::import(&zip_path, &scratch.join("unzipped")).unwrap();
    let mut formats: Vec<_> = models
        .iter()
        .map(|m| match m {
            Model::StageOne(m) => m.info().format.clone(),
            Model::StageTwo(m) => m.info().format.clone(),
        })
        .collect();
    formats.sort();
    assert_eq!(formats, ["kws_stage_one.v1", "kws_stage_two.v2"]);

    // 单拖一级的目录也行
    let models = adapter::import(&fixtures().join("bundles/stage_one"), &scratch).unwrap();
    assert!(matches!(models.as_slice(), [Model::StageOne(_)]));
    // 两层以内的都找得到
    assert_eq!(adapter::import(&fixtures(), &scratch).unwrap().len(), 2);
    // 什么都没有的目录要说清楚
    let empty = scratch.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let err = adapter::import(&empty, &scratch).err().unwrap().to_string();
    assert!(err.contains("kws_stage_one.v1"), "{err}");
}

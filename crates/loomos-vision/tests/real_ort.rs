//! 真视觉引擎回归（ort feature 门控）：
//! - YuNet（板同款 2023mar onnx）在真实合影上检测多张人脸；
//! - SFace 嵌入：同人自匹配 cos=1.0、不同人低于阈值（预处理正确性的经验判据）。
//!
//! 运行条件（缺任一 → skip 并 PASS，不 fail）：
//! - LOOMOS_ORT_DLL=onnxruntime.dll 路径（NuGet 包解出）
//! - LOOMOS_YUNET_MODEL、LOOMOS_SFACE_MODEL
//! - LOOMOS_SELFIE_JPG（真实合影图）
//!
//! 通过后读数回填 docs/dev/EVIDENCE.md。

#![cfg(feature = "ort")]
// ⚠️ 本机现象：独立 onnxruntime.dll（NuGet 各版本）在本 Insider 系统上
// DllMain/GetApi 写崩溃（sherpa 内嵌 ORT 与 cv2 dnn 均正常）——故本测试 #[ignore]，
// 板端/Linux 环境显式 `cargo test -- --ignored` 跑。cv2 侧真验证见 EVIDENCE.md。
#![cfg(feature = "ort")]

use loomos_ort_sys::OrtSysError;
use loomos_vision::ort_adapter::{OrtSFaceEmbedder, OrtVisionConfig, OrtYuNetDetector};
use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

fn config() -> Option<(OrtVisionConfig, PathBuf)> {
    let dll = env_path("LOOMOS_ORT_DLL")?;
    let yunet = env_path("LOOMOS_YUNET_MODEL")?;
    let sface = env_path("LOOMOS_SFACE_MODEL")?;
    let selfie = env_path("LOOMOS_SELFIE_JPG")?;
    Some((
        OrtVisionConfig {
            dll,
            yunet_model: yunet,
            sface_model: sface,
            num_threads: 2,
        },
        selfie,
    ))
}

fn frame_from_jpg(path: &PathBuf) -> Result<loomos_vision::frame::Frame, String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    Ok(loomos_vision::frame::Frame {
        id: 0,
        token: "real".into(),
        session: "real-test".into(),
        captured_ms: 0,
        width: w,
        height: h,
        format: loomos_vision::frame::Format::Rgb888,
        data: rgb.into_raw(),
    })
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na * nb < 1e-12 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[test]
#[ignore = "ORT dll 本机崩溃（见文件头注记）；板端/Linux 显式 --ignored 跑"]
fn real_yunet_detects_many_faces() {
    let Some((cfg, selfie)) = config() else {
        eprintln!("skip: LOOMOS_ORT_DLL / 模型 / 合影图 未就位（Assumed, 不 fail）");
        return;
    };
    let inner = loomos_vision::ort_adapter::Sessions::load(&cfg)
        .expect("真 YuNet session 加载失败（模型在但加载错——这是 fail）");
    let detector = OrtYuNetDetector::new(inner.clone());
    let frame = frame_from_jpg(&selfie).expect("合影解码失败");
    eprintln!("合影尺寸: {}×{}", frame.width, frame.height);
    let dets = detector
        .detect_full(&frame)
        .expect("真 YuNet 检测失败（这是 fail）");
    eprintln!("真 YuNet 检测人脸数: {}", dets.len());
    assert!(
        dets.len() >= 10,
        "百人合影应检出大量人脸（≥10），实际 {}",
        dets.len()
    );
    assert!(dets.iter().all(|d| d.landmarks.len() == 5));
}

#[test]
#[ignore = "同上"]
fn real_sface_same_person_matches() {
    let Some((cfg, selfie)) = config() else {
        eprintln!("skip: 模型/图未就位");
        return;
    };
    let inner = loomos_vision::ort_adapter::Sessions::load(&cfg).unwrap();
    let detector = OrtYuNetDetector::new(inner.clone());
    let embedder = OrtSFaceEmbedder::new(inner);
    let frame = frame_from_jpg(&selfie).unwrap();
    let dets = detector.detect_full(&frame).unwrap();
    // 取分数最高的两张不同人脸
    let mut best: Vec<loomos_vision::embed::Detection> =
        dets.iter().filter(|d| d.score >= 0.9).cloned().collect();
    best.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    best.dedup_by(|a, b| (a.bbox[0] as i32 - b.bbox[0] as i32).abs() < 8);
    if best.len() < 2 {
        eprintln!("skip: 高分人脸不足 2 张（{}）", best.len());
        return;
    }
    let to_f32 = |l: &[[u32; 2]; 5]| -> [[f32; 2]; 5] {
        let mut out = [[0.0f32; 2]; 5];
        for (i, p) in l.iter().enumerate() {
            out[i] = [p[0] as f32, p[1] as f32];
        }
        out
    };
    let e1 = embedder
        .embed_detection(&frame, &to_f32(&best[0].landmarks))
        .expect("嵌入失败（这是 fail）");
    let e2 = embedder
        .embed_detection(&frame, &to_f32(&best[1].landmarks))
        .expect("嵌入失败（这是 fail）");
    let self_cos = cosine(&e1.vector, &e1.vector);
    let cross = cosine(&e1.vector, &e2.vector);
    eprintln!(
        "真 SFace：同人 cos={self_cos:.4}，不同人 cos={cross:.4}，维度={}（zoo 阈值 0.363）",
        e1.dim
    );
    assert!((self_cos - 1.0).abs() < 1e-6);
    // 正确预处理的标志：不同人相似度显著低于同人；错误归一化会把跨人推到 0.9+
    assert!(cross < 0.9, "不同人 cos={cross} 异常偏高（疑似归一化错误）");
}

//! PAD 防伪（Presentation Attack Detection）。
//!
//! 契约（旧板 presentation_attack_detection 字段 + fmql30tai-sdi-npu-face-recognition）：
//! - 模型：MiniFASNetV2 + MiniFASNetV1SE，80×80 RGB 双模型取平均 → live_score；
//! - 状态值域固定：not_applicable / model_live / model_non_live / inconclusive /
//!   stale / not_evaluated；
//! - `security_calibrated=false`：PAD 分数**未经安全标定，不能作为开锁许可**
//!   （README 原文纪律）；loomos-home::door 的 warn/enforce 策略消费本模块输出。

use crate::embed::Detection;
use crate::frame::Frame;
use serde::{Deserialize, Serialize};

/// PAD 状态值域（固定集合，与旧板字段一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadStatus {
    /// 没跑（无证据）。
    NotApplicable,
    /// 模型判定真人。
    ModelLive,
    /// 模型判定攻击（平面照片/重放）。
    ModelNonLive,
    /// 模型不可判（分数落在阈值带内）。
    Inconclusive,
    /// 帧过期（不得用于授权）。
    Stale,
    /// 超出 face_limit 等原因未评估。
    NotEvaluated,
}

impl PadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PadStatus::NotApplicable => "not_applicable",
            PadStatus::ModelLive => "model_live",
            PadStatus::ModelNonLive => "model_non_live",
            PadStatus::Inconclusive => "inconclusive",
            PadStatus::Stale => "stale",
            PadStatus::NotEvaluated => "not_evaluated",
        }
    }
}

/// 旧板 JSON 字段同形结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresentationAttackDetection {
    pub status: PadStatus,
    pub security_calibrated: bool,
    /// live_score 只记录，**不作为依据**（未标定）。
    pub live_score: Option<f64>,
    /// 判定原因（人读）。
    pub reason: Option<String>,
}

/// 双模型融合的默认 live 阈值（mock 档）。
pub const DEFAULT_PAD_LIVE_THRESHOLD: f64 = 0.5;

/// PAD 引擎 trait：对检测框内区域给 live_score。
pub trait PadEngine: Send {
    fn name(&self) -> &str;
    /// 是否已完成面向本摄像头/场景的安全标定（false 时调用方必须 warn/不得据此开锁）。
    fn security_calibrated(&self) -> bool;
    fn live_score(&self, frame: &Frame, det: &Detection) -> f64;
}

/// mock PAD：分析框内纹理方差——live 人脸有细纹理（像素抖动），平面照片攻击平坦。
/// 确定性、可在合成帧上回归。
#[derive(Debug)]
pub struct MockPad {
    pub live_threshold: f64,
    /// 是否已标定（mock 默认 false，对齐旧板现状：未标定不作开锁依据）。
    pub calibrated: bool,
}

impl Default for MockPad {
    fn default() -> Self {
        Self {
            live_threshold: DEFAULT_PAD_LIVE_THRESHOLD,
            calibrated: false,
        }
    }
}

impl MockPad {
    /// 从分数映射状态（值域映射与旧板一致；inconclusive 带 = 阈值 ±0.1）。
    pub fn score_to_status(score: f64, threshold: f64) -> PadStatus {
        if (score - threshold).abs() < 0.1 {
            PadStatus::Inconclusive
        } else if score >= threshold {
            PadStatus::ModelLive
        } else {
            PadStatus::ModelNonLive
        }
    }
}

impl PadEngine for MockPad {
    fn name(&self) -> &str {
        "mock-minifasnet"
    }

    fn security_calibrated(&self) -> bool {
        self.calibrated
    }

    fn live_score(&self, frame: &Frame, det: &Detection) -> f64 {
        // 只统计 blob 像素（v>96，排除框内背景）：
        // live 人脸有细纹理（stddev≈0.8/像素），平面照片攻击完全平坦（stddev=0）
        let [x0, y0, x1, y1] = det.bbox;
        let w = frame.width as usize;
        let mut sum = 0f64;
        let mut sum2 = 0f64;
        let mut n = 0u64;
        for yy in y0..=y1.min(frame.height - 1) {
            for xx in x0..=x1.min(frame.width - 1) {
                let v = frame.data[(yy as usize * w + xx as usize) * 3] as f64;
                if v > 96.0 {
                    sum += v;
                    sum2 += v * v;
                    n += 1;
                }
            }
        }
        if n == 0 {
            return 0.0;
        }
        let mean = sum / n as f64;
        let var = (sum2 / n as f64) - mean * mean;
        let stddev = var.max(0.0).sqrt();
        // 灵敏度：stddev ≥ 0.5/像素 即判 live 高分
        ((stddev * 2.0).min(1.0)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::FaceDetector;
    use crate::frame::{FaceSpec, FrameSource, MockFrameSource, SyntheticScene};

    fn clock() -> u64 {
        1_000_000
    }

    fn frame(signature: u8, spoof: bool) -> (Frame, Detection) {
        let mut src = MockFrameSource::new(160, 120, clock);
        src.set_scene(SyntheticScene {
            faces: vec![FaceSpec {
                signature,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof,
        });
        let f = src.latest().unwrap();
        let det = crate::embed::MockDetector.detect(&f).remove(0);
        (f, det)
    }

    #[test]
    fn live_face_scores_high_print_scores_low() {
        let (live_frame, live_det) = frame(200, false);
        let (spoof_frame, spoof_det) = frame(200, true);
        let pad = MockPad::default();
        let live = pad.live_score(&live_frame, &live_det);
        let spoof = pad.live_score(&spoof_frame, &spoof_det);
        assert!(live > 0.5, "live 纹理应高分，实际 {live}");
        assert!(spoof < 0.1, "平坦照片应低分，实际 {spoof}");
    }

    #[test]
    fn status_mapping_covers_value_domain() {
        assert_eq!(MockPad::score_to_status(0.9, 0.5), PadStatus::ModelLive);
        assert_eq!(MockPad::score_to_status(0.2, 0.5), PadStatus::ModelNonLive);
        assert_eq!(MockPad::score_to_status(0.55, 0.5), PadStatus::Inconclusive);
        // 值域字符串与旧板字段一致
        assert_eq!(PadStatus::NotApplicable.as_str(), "not_applicable");
        assert_eq!(PadStatus::ModelLive.as_str(), "model_live");
        assert_eq!(PadStatus::ModelNonLive.as_str(), "model_non_live");
        assert_eq!(PadStatus::Inconclusive.as_str(), "inconclusive");
        assert_eq!(PadStatus::Stale.as_str(), "stale");
        assert_eq!(PadStatus::NotEvaluated.as_str(), "not_evaluated");
    }

    #[test]
    fn mock_pad_reports_uncalibrated() {
        let pad = MockPad::default();
        assert!(!pad.security_calibrated(), "mock 档必须如实报告未标定");
    }
}

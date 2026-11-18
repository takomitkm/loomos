//! 识别流水线：帧 → 检测 → 嵌入 → 匹配 → PAD → 结果（8091 JSON 契约同形）。
//!
//! 旧板契约（HA4FPAI 实测）：
//! ```json
//! {"entry":{"result":{"identities":[{...,"name":...}],"faces":[],"face_count":N,
//!  "presentation_attack_detection":{"status":...,"security_calibrated":...,
//!  "reason":...,"model_result.live_score":...}}}}
//! ```
//! 帧过期 → PAD 状态 `stale`；无新鲜帧 → 上层 `video_no_frame`（503）。

use crate::embed::{FaceDetector, FaceEmbedder};
use crate::frame::FrameSource;
use crate::gallery::FaceGallery;
use crate::pad::{PadEngine, PadStatus, PresentationAttackDetection};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 识别一条（命中的人）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub score: f64,
}

/// 最多识别 4 张脸（旧板 MAX_RECOGNIZED_FACES=4；超出 → PAD not_evaluated/face_limit）。
pub const MAX_RECOGNIZED_FACES: usize = 4;

/// 一次识别的完整结果（8091 result 字段同形）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recognition {
    pub identities: Vec<Identity>,
    pub face_count: usize,
    pub presentation_attack_detection: PresentationAttackDetection,
    /// 使用的帧 id（审计：结果可追溯到帧）。
    pub frame_id: u64,
    /// 使用的帧会话（审计五元组之一）。
    pub frame_session: String,
}

/// 引擎集合（可拆卸点：换任何一个引擎只需换一个盒子）。
pub struct VisionStages {
    pub source: Box<dyn FrameSource>,
    pub detector: Box<dyn FaceDetector>,
    pub embedder: Box<dyn FaceEmbedder>,
    pub pad: Box<dyn PadEngine>,
    /// 帧最大可接受年龄（超过 → stale；旧板帧滞留语义）。
    pub max_frame_age: Duration,
}

/// 视频识别流水线（持有 gallery；线程安全由外部封装）。
pub struct VisionPipeline {
    pub stages: VisionStages,
    pub gallery: FaceGallery,
}

impl VisionPipeline {
    /// 对当前帧跑一次完整识别。
    ///
    /// 错误语义：
    /// - 帧源离线/无新鲜帧 → `VideoNoFrame`（上层映射 503 / video_no_frame）；
    /// - 帧过期 → 正常返回，但 PAD 状态 = `Stale`（**过期帧不得用于授权**，
    ///   identities 照常给出但调用方必须检查 PAD 状态）。
    pub fn recognize(&mut self, now_ms: u64) -> Result<Recognition, PipelineError> {
        let frame = self
            .stages
            .source
            .latest()
            .map_err(|_| PipelineError::VideoNoFrame)?;
        let stale = !frame.is_fresh(now_ms, self.stages.max_frame_age);
        let detections = self.stages.detector.detect(&frame);

        let mut identities = Vec::new();
        let mut first_score: Option<f64> = None;
        let mut pad_reason = String::new();
        for det in detections.iter().take(MAX_RECOGNIZED_FACES) {
            let emb = match self.stages.embedder.embed(&frame, det) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if let Ok(m) = self.gallery.match_embedding(&emb)
                && let Some(name) = &m.name
            {
                identities.push(Identity {
                    name: name.clone(),
                    score: m.best_score,
                });
            }
            let score = self.stages.pad.live_score(&frame, det);
            if first_score.is_none() {
                first_score = Some(score);
                pad_reason = if stale {
                    "frame stale".into()
                } else if detections.is_empty() {
                    "no face".into()
                } else {
                    "evaluated".into()
                };
            }
        }

        let pad_status = if stale {
            PadStatus::Stale
        } else if detections.is_empty() {
            PadStatus::NotApplicable
        } else if detections.len() > MAX_RECOGNIZED_FACES {
            // 超 face_limit：not_evaluated（旧板 reason=face_limit 契约）
            pad_reason = "face_limit".into();
            PadStatus::NotEvaluated
        } else {
            first_score
                .map(|s| {
                    crate::pad::MockPad::score_to_status(s, crate::pad::DEFAULT_PAD_LIVE_THRESHOLD)
                })
                .unwrap_or(PadStatus::NotEvaluated)
        };
        let live_score = if pad_status == PadStatus::NotApplicable {
            None
        } else {
            first_score
        };

        Ok(Recognition {
            identities,
            face_count: detections.len(),
            presentation_attack_detection: PresentationAttackDetection {
                status: pad_status,
                security_calibrated: self.stages.pad.security_calibrated(),
                live_score,
                reason: Some(pad_reason),
            },
            frame_id: frame.id,
            frame_session: frame.session.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PipelineError {
    #[error("video_no_frame（摄像头无新鲜画面 → 板端 503 契约）")]
    VideoNoFrame,
}

// ---------------- mock 检测器（在 embed.rs 的实现转发，保持模块内聚） ----------------

pub use crate::embed::MockDetector;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::frame::{FaceSpec, MockFrameSource, SyntheticScene};
    use crate::pad::MockPad;

    fn clock() -> u64 {
        1_000_000
    }

    fn pipeline_with(scene: SyntheticScene) -> VisionPipeline {
        VisionPipeline {
            stages: VisionStages {
                source: Box::new({
                    let mut s = MockFrameSource::new(160, 120, clock);
                    s.set_scene(scene);
                    s
                }),
                detector: Box::new(MockDetector),
                embedder: Box::new(MockEmbedder),
                pad: Box::new(MockPad::default()),
                max_frame_age: Duration::from_millis(5_000),
            },
            gallery: {
                let mut g = FaceGallery::new(crate::gallery::DEFAULT_FACE_MATCH_THRESHOLD);
                // 注册张三：签名 200、位置居中
                let mut src = MockFrameSource::new(160, 120, clock);
                src.set_scene(SyntheticScene {
                    faces: vec![FaceSpec {
                        signature: 200,
                        cx: 0.5,
                        cy: 0.5,
                        r: 0.2,
                    }],
                    spoof: false,
                });
                let f = src.latest().unwrap();
                let det = MockDetector.detect(&f).remove(0);
                let emb = MockEmbedder.embed(&f, &det).unwrap();
                g.enroll("张三", emb).unwrap();
                g
            },
        }
    }

    #[test]
    fn recognize_known_person_with_live_pad() {
        let mut p = pipeline_with(SyntheticScene {
            faces: vec![FaceSpec {
                signature: 200,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof: false,
        });
        let r = p.recognize(clock() + 100).unwrap();
        assert_eq!(r.face_count, 1);
        assert_eq!(r.identities.len(), 1);
        assert_eq!(r.identities[0].name, "张三");
        assert_eq!(r.presentation_attack_detection.status, PadStatus::ModelLive);
        assert!(!r.presentation_attack_detection.security_calibrated);
    }

    #[test]
    fn recognize_unknown_person_gives_no_identity() {
        let mut p = pipeline_with(SyntheticScene {
            faces: vec![FaceSpec {
                signature: 130,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof: false,
        });
        let r = p.recognize(clock() + 100).unwrap();
        assert_eq!(r.face_count, 1);
        assert!(r.identities.is_empty(), "未命中不应编造身份");
    }

    #[test]
    fn spoof_frame_flags_non_live() {
        let mut p = pipeline_with(SyntheticScene {
            faces: vec![FaceSpec {
                signature: 200,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof: true,
        });
        let r = p.recognize(clock() + 100).unwrap();
        assert_eq!(
            r.presentation_attack_detection.status,
            PadStatus::ModelNonLive
        );
    }

    #[test]
    fn stale_frame_marked_stale() {
        let mut p = pipeline_with(SyntheticScene {
            faces: vec![FaceSpec {
                signature: 200,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof: false,
        });
        p.stages.max_frame_age = Duration::from_millis(500);
        // 帧采集于 clock()，识别发生在 2s 后 → stale
        let r = p.recognize(clock() + 2_000).unwrap();
        assert_eq!(r.presentation_attack_detection.status, PadStatus::Stale);
    }

    #[test]
    fn offline_source_maps_to_video_no_frame() {
        let mut p = pipeline_with(SyntheticScene::default());
        // 把帧源切换为离线（模拟摄像头线路黑/无新鲜帧 → 板端 503 契约）
        // VisionStages.source 是 trait object；用 downcast 不可行，
        // 直接构造一个离线 pipeline 验证错误映射。
        let mut p2 = VisionPipeline {
            stages: VisionStages {
                source: Box::new({
                    let mut s = MockFrameSource::new(160, 120, clock);
                    s.set_online(false);
                    s
                }),
                detector: Box::new(MockDetector),
                embedder: Box::new(MockEmbedder),
                pad: Box::new(MockPad::default()),
                max_frame_age: Duration::from_millis(5_000),
            },
            gallery: FaceGallery::new(crate::gallery::DEFAULT_FACE_MATCH_THRESHOLD),
        };
        let _ = &mut p;
        assert!(matches!(
            p2.recognize(clock()),
            Err(PipelineError::VideoNoFrame)
        ));
    }
}

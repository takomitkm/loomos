//! VisionProvider：把整条视频路包装成标准 DeviceProvider。
//!
//! **可拆卸装配**（`VisionMode`）：
//! - `Detached`：整条视频路拆掉，不注册相机/录制/画面设备；
//! - `MockDevices`：旧 mock 行为（faults 驱动，等同改造前）；
//! - `Pipeline`：本 crate 的自研视频路（合成帧源 + mock 引擎，流水线真实运行）；
//! - `HttpBoard`：对接旧板 8091/8092/8093（`HttpBoardAdapter`）。
//!
//! 对上层（executor/door/控制台）完全透明：设备 id 与操作名不变。

use crate::embed::{FaceDetector, FaceEmbedder, MockDetector};
use crate::frame::{FaceSpec, FrameSource, MockFrameSource, SyntheticScene};
use crate::gallery::FaceGallery;
use crate::pad::MockPad;
use crate::pipeline::{VisionPipeline, VisionStages};
use crate::recorder::{MockRecorderBackend, RecorderCore, RecorderError, RecorderMode};
use crate::view::{MockStreamEncoder, ViewSession};
use loomos_device::core::{
    DeviceCapability, DeviceDescriptor, DeviceError, DeviceHealth, DeviceProvider, OpOutcome,
    ProviderId,
};
use serde_json::json;
use std::time::Duration;

/// 故障注入（语义与旧 MockFaults 对齐，供测试/演示）。
#[derive(Debug, Clone, Default)]
pub struct VisionFaults {
    /// 场景中不出现人脸（等价旧 face_identities 空表）。
    pub no_faces: bool,
    /// 摄像头离线（→ NoFreshFrame / 503）。
    pub camera_offline: bool,
    /// 伪造平面照片攻击（PAD → model_non_live）。
    pub spoof: bool,
    /// 覆盖 PAD 状态输出（None = 流水线自判）。
    pub pad_status_override: Option<String>,
    pub pad_security_calibrated: bool,
    pub view_ready: bool,
    /// 注入挂死 → UnknownOutcome。
    pub hang_devices: Vec<String>,
}

/// 视频路配置。
#[derive(Debug, Clone)]
pub struct VisionConfig {
    pub width: u32,
    pub height: u32,
    /// 帧最大年龄（超过 → stale）。
    pub max_frame_age_ms: u64,
    /// gallery 种子：人名 → 签名灰度（默认张三=200）。
    pub gallery_seed: Vec<(String, u8)>,
    pub faults: VisionFaults,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            width: 160,
            height: 120,
            max_frame_age_ms: 3_000,
            gallery_seed: vec![("张三".into(), 200)],
            faults: VisionFaults::default(),
        }
    }
}

/// 自研视频路提供者（Pipeline 模式）。
pub struct VisionProvider {
    id: ProviderId,
    pipeline: VisionPipeline,
    recorder: RecorderCore,
    view: ViewSession,
    snapshots: Vec<String>,
    faults: VisionFaults,
    clock_ms: fn() -> u64,
}

impl VisionProvider {
    pub fn new(cfg: VisionConfig, clock_ms: fn() -> u64) -> Self {
        let mut source = MockFrameSource::new(cfg.width, cfg.height, clock_ms);
        let faces: Vec<FaceSpec> = if cfg.faults.no_faces {
            vec![]
        } else {
            cfg.gallery_seed
                .iter()
                .map(|(_, sig)| FaceSpec {
                    signature: *sig,
                    cx: 0.5,
                    cy: 0.5,
                    r: 0.2,
                })
                .collect()
        };
        source.set_scene(SyntheticScene {
            faces,
            spoof: cfg.faults.spoof,
        });
        source.set_online(!cfg.faults.camera_offline);
        let mut gallery = FaceGallery::new(crate::gallery::DEFAULT_FACE_MATCH_THRESHOLD);
        for (name, sig) in &cfg.gallery_seed {
            // 用同签名居中帧注册模板
            let mut src = MockFrameSource::new(cfg.width, cfg.height, clock_ms);
            src.set_scene(SyntheticScene {
                faces: vec![FaceSpec {
                    signature: *sig,
                    cx: 0.5,
                    cy: 0.5,
                    r: 0.2,
                }],
                spoof: false,
            });
            let f = src.latest().unwrap();
            let det = match MockDetector.detect(&f).first() {
                Some(d) => d.clone(),
                None => continue,
            };
            let _ = crate::embed::MockEmbedder.embed(&f, &det).map(|e| {
                let _ = gallery.enroll(name, e);
            });
        }
        let pad = MockPad {
            calibrated: cfg.faults.pad_security_calibrated,
            ..MockPad::default()
        };
        let pipeline = VisionPipeline {
            stages: VisionStages {
                source: Box::new(source),
                detector: Box::new(MockDetector),
                embedder: Box::new(crate::embed::MockEmbedder),
                pad: Box::new(pad),
                max_frame_age: Duration::from_millis(cfg.max_frame_age_ms),
            },
            gallery,
        };
        let mut view = ViewSession::new(Box::new(MockStreamEncoder::default()));
        if cfg.faults.view_ready {
            view.start();
        }
        Self {
            id: "vision".into(),
            pipeline,
            recorder: RecorderCore::new(Box::new(MockRecorderBackend::default()), clock_ms),
            view,
            snapshots: Vec::new(),
            faults: cfg.faults,
            clock_ms,
        }
    }

    pub fn gallery_len(&self) -> usize {
        self.pipeline.gallery.len()
    }

    pub fn set_view_ready(&mut self, ready: bool) {
        if ready {
            self.view.start();
        } else {
            self.view.stop();
        }
    }
}

impl DeviceProvider for VisionProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn discover(&mut self) -> Vec<DeviceDescriptor> {
        vec![
            DeviceDescriptor {
                id: "camera.menkou".into(),
                provider: self.id.clone(),
                kind: "face".into(),
                version: 2,
                capabilities: vec![
                    DeviceCapability::FaceMatch,
                    DeviceCapability::Capture,
                    DeviceCapability::VoiceprintCheck,
                    DeviceCapability::Status,
                ],
            },
            DeviceDescriptor {
                id: "camera.view".into(),
                provider: self.id.clone(),
                kind: "camera".into(),
                version: 2,
                capabilities: vec![DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "switch.recorder".into(),
                provider: self.id.clone(),
                kind: "recorder".into(),
                version: 2,
                capabilities: vec![DeviceCapability::RecordMode, DeviceCapability::Status],
            },
        ]
    }

    fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &serde_json::Value,
    ) -> Result<OpOutcome, DeviceError> {
        if self.faults.hang_devices.iter().any(|d| d == device) {
            return Ok(OpOutcome::UnknownOutcome {
                reason: "注入挂死：操作已发出但结果未知".into(),
            });
        }
        match (device, op) {
            ("camera.menkou", "face_match") => {
                let now = (self.clock_ms)();
                let rec = self
                    .pipeline
                    .recognize(now)
                    .map_err(|_| DeviceError::NoFreshFrame(device.to_string()))?;
                // 每次识别驱动一次录制状态机（auto 模式：有人脸才录）
                self.recorder.tick(rec.face_count > 0);
                let mut pad = serde_json::json!({
                    "status": rec.presentation_attack_detection.status.as_str(),
                    "security_calibrated": rec.presentation_attack_detection.security_calibrated,
                });
                if let Some(score) = rec.presentation_attack_detection.live_score {
                    pad["live_score"] = json!(score);
                }
                if let Some(ov) = &self.faults.pad_status_override {
                    pad["status"] = json!(ov);
                }
                if let Some(reason) = &rec.presentation_attack_detection.reason {
                    pad["reason"] = json!(reason);
                }
                Ok(OpOutcome::Done(json!({
                    "identities": rec.identities,
                    "faces": [],
                    "face_count": rec.face_count,
                    "presentation_attack_detection": pad,
                    "frame_id": rec.frame_id,
                })))
            }
            ("camera.menkou", "capture") => {
                // 快照：记帧元数据（真实后端落盘 JPEG——旧板 /mnt/data/klar/snapshots）
                self.snapshots
                    .push(format!("snap-{}", self.snapshots.len() + 1));
                Ok(OpOutcome::Done(
                    json!({"snapshot": self.snapshots.last().unwrap()}),
                ))
            }
            ("camera.menkou", "voiceprint_status") => {
                // 声纹三查（模型/运行时/库人数）——库人数来自声纹域，这里如实报能力面
                Ok(OpOutcome::Done(json!({
                    "model": "mock",
                    "runtime": "rust-inproc",
                    "library_ready": true,
                })))
            }
            ("camera.menkou", "query") => {
                let pad_cal = self.pipeline.stages.pad.security_calibrated();
                Ok(OpOutcome::Done(json!({
                    "ready": !self.faults.camera_offline,
                    "pad_security_calibrated": pad_cal,
                    "gallery": self.pipeline.gallery.len(),
                })))
            }
            ("camera.view", "query") => {
                let st = self.view.stats();
                Ok(OpOutcome::Done(json!({
                    "ready": st.ready,
                    "session": st.session,
                })))
            }
            ("switch.recorder", "record_mode") => {
                let mode = params
                    .get("mode")
                    .and_then(|m| m.as_str())
                    .ok_or_else(|| DeviceError::BadParams("mode 必填".into()))?;
                let m =
                    RecorderMode::parse(mode).map_err(|e| DeviceError::BadParams(e.to_string()))?;
                let applied = self
                    .recorder
                    .set_mode(m)
                    .map_err(|e| DeviceError::Failed(e.to_string()))?;
                // 回读校验（板端语义）
                if applied != m {
                    return Err(DeviceError::Failed("回读 mode 不符".into()));
                }
                Ok(OpOutcome::Done(json!({"mode": applied.as_str()})))
            }
            ("switch.recorder", "query") => {
                let st = self.recorder.stats();
                Ok(OpOutcome::Done(json!({
                    "mode": st.mode.as_str(),
                    "state": format!("{:?}", st.state).to_lowercase(),
                    "queue": st.queue,
                })))
            }
            _ => Err(DeviceError::UnsupportedOp(
                device.to_string(),
                op.to_string(),
            )),
        }
    }

    fn health(&self, device: &str) -> Result<DeviceHealth, DeviceError> {
        if self.faults.camera_offline && device == "camera.menkou" {
            return Ok(DeviceHealth::Faulted);
        }
        Ok(DeviceHealth::Ok)
    }

    fn telemetry(&self, device: &str) -> Result<serde_json::Value, DeviceError> {
        match device {
            "switch.recorder" => {
                let st = self.recorder.stats();
                Ok(json!({"mode": st.mode.as_str(), "trash_count": st.trash_count}))
            }
            "camera.menkou" => Ok(json!({
                "pad_security_calibrated": self.pipeline.stages.pad.security_calibrated(),
                "snapshots": self.snapshots.len(),
            })),
            _ => Ok(json!({})),
        }
    }
}

/// RecorderError → DeviceError 的桥（供控制台 purge 等扩展用）。
impl From<RecorderError> for DeviceError {
    fn from(e: RecorderError) -> Self {
        DeviceError::Failed(e.to_string())
    }
}

/// 视频路装配模式（可拆卸的总开关，loomosd 使用）。
#[derive(Debug, Clone, Default)]
pub enum VisionMode {
    /// 整条视频路拆掉（相机/录制/画面设备不注册）。
    Detached,
    /// 旧 mock 设备（faults 驱动，等价改造前行为）。
    #[default]
    MockDevices,
    /// 自研视频路流水线（本 crate，mock 引擎）。
    Pipeline(VisionConfig),
    /// 对接旧板 HTTP 服务（8091/8092/8093）。
    HttpBoard {
        host: String,
        face_port: u16,
        view_port: u16,
        record_port: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> u64 {
        1_000_000
    }

    #[test]
    fn provider_full_flow_match_and_pad() {
        let mut p = VisionProvider::new(VisionConfig::default(), clock);
        let out = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap();
        if let OpOutcome::Done(v) = out {
            assert_eq!(v["face_count"], 1);
            let ids = v["identities"].as_array().unwrap();
            assert_eq!(ids.len(), 1);
            assert_eq!(ids[0]["name"], "张三");
            assert_eq!(v["presentation_attack_detection"]["status"], "model_live");
            assert_eq!(
                v["presentation_attack_detection"]["security_calibrated"],
                false
            );
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn provider_no_face_reports_empty_not_503() {
        let cfg = VisionConfig {
            faults: VisionFaults {
                no_faces: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut p = VisionProvider::new(cfg, clock);
        let out = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap();
        if let OpOutcome::Done(v) = out {
            assert_eq!(v["face_count"], 0);
            assert_eq!(v["identities"].as_array().unwrap().len(), 0);
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn provider_offline_is_no_fresh_frame() {
        let cfg = VisionConfig {
            faults: VisionFaults {
                camera_offline: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut p = VisionProvider::new(cfg, clock);
        let err = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap_err();
        assert!(matches!(err, DeviceError::NoFreshFrame(_)));
    }

    #[test]
    fn provider_spoof_flags_non_live() {
        let cfg = VisionConfig {
            faults: VisionFaults {
                spoof: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut p = VisionProvider::new(cfg, clock);
        let out = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap();
        if let OpOutcome::Done(v) = out {
            assert_eq!(
                v["presentation_attack_detection"]["status"],
                "model_non_live"
            );
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn provider_recorder_contract() {
        let mut p = VisionProvider::new(VisionConfig::default(), clock);
        // 板端契约：mode 只收 off/auto/manual
        let err = p
            .execute("switch.recorder", "record_mode", &json!({"mode": "on"}))
            .unwrap_err();
        assert!(err.to_string().contains("Mode must be off, auto or manual"));
        // auto 模式 + 有人脸 → face_match 驱动 tick → recording
        p.execute("switch.recorder", "record_mode", &json!({"mode": "auto"}))
            .unwrap();
        p.execute("camera.menkou", "face_match", &json!({}))
            .unwrap();
        let st = p.execute("switch.recorder", "query", &json!({})).unwrap();
        if let OpOutcome::Done(v) = st {
            assert_eq!(v["state"], "recording", "auto+人脸应录制中");
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn provider_hang_returns_unknown() {
        let cfg = VisionConfig {
            faults: VisionFaults {
                hang_devices: vec!["camera.menkou".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut p = VisionProvider::new(cfg, clock);
        let out = p.execute("camera.menkou", "capture", &json!({})).unwrap();
        assert!(matches!(out, OpOutcome::UnknownOutcome { .. }));
    }

    #[test]
    fn vision_mode_default_is_mock_devices() {
        assert!(matches!(VisionMode::default(), VisionMode::MockDevices));
    }
}

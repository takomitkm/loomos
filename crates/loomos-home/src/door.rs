//! 四道闸开门（旧 boards/door_unlock.py 的 Rust 版）：口令 → 人脸 → PAD 防伪 → 声纹。
//! 任一不过不开。PAD 策略 warn/enforce（旧 env KLAR_PAD_MODE）；
//! 未安全标定时附提醒但不阻断（warn 档），enforce 档必须 model_live。

use crate::voiceprint::MockVoiceprint;
use loomos_device::core::{DeviceError, DeviceRegistry, OpOutcome};
use loomos_nlu::grammar::normalize;
use loomos_runtime::capability::{CapabilityStore, Token};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 开门口令（旧 UNLOCK_PHRASES 10 条；双向包含匹配）。
pub const UNLOCK_PHRASES: [&str; 10] = [
    "开门",
    "我回来了",
    "我回来了开门",
    "我回家了",
    "我回家了开门",
    "开门吧",
    "帮我开门",
    "把门打开",
    "门开一下",
    "开一下门",
];

/// PAD 防伪策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadMode {
    /// 记录+播报提醒，不阻断（默认：security_calibrated=false 未标定）。
    Warn,
    /// 必须 model_live。
    Enforce,
}

/// 每道闸的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResult {
    pub gate: String,
    pub passed: bool,
    pub detail: serde_json::Value,
}

/// 开门结果（旧契约 `{"ok","steps":{phrase,face,pad,voiceprint},"speech"}`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockResult {
    pub ok: bool,
    pub steps: Vec<GateResult>,
    pub speech: String,
}

/// 口令闸：归一化后双向包含匹配。
pub fn is_unlock_phrase(text: &str) -> bool {
    let norm = normalize(text);
    UNLOCK_PHRASES
        .iter()
        .any(|p| norm.contains(p) || p.contains(&norm))
}

/// 四道闸全流程。
///
/// `face_result`/`voiceprint_result` 由调用方先查询设备得到——本模块做**策略**，
/// 不直接持有 registry，保持策略与执行的边界。
pub struct UnlockFlow<'a> {
    pub pad_mode: PadMode,
    pub voiceprint_threshold: f64,
    /// 声纹库；None = 无库（闸直接不过）。
    pub voiceprint: Option<&'a MockVoiceprint>,
    /// 当前待验证音频样本。
    pub samples: &'a [i16],
    pub rate: u32,
}

impl<'a> UnlockFlow<'a> {
    pub fn run(
        &self,
        text: &str,
        face: &Result<serde_json::Value, DeviceError>,
        registry: &mut DeviceRegistry,
        caps: &CapabilityStore,
        token: &Token,
    ) -> UnlockResult {
        let mut steps = Vec::new();

        // 闸 1：口令
        let phrase_ok = is_unlock_phrase(text);
        steps.push(GateResult {
            gate: "phrase".into(),
            passed: phrase_ok,
            detail: json!({"normalized": normalize(text)}),
        });
        if !phrase_ok {
            return UnlockResult {
                ok: false,
                steps,
                speech: "这不是开门指令。".into(),
            };
        }

        // 闸 2+3：人脸 + PAD（同一次识别，旧 check_face 语义）
        let (face_ok, face_name, pad_status, pad_calibrated) = match face {
            Ok(v) => {
                let identities: Vec<String> = v
                    .get("identities")
                    .and_then(|i| i.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| {
                                x.as_str().map(|s| s.to_string()).or_else(|| {
                                    x.get("name")
                                        .and_then(|n| n.as_str())
                                        .map(|s| s.to_string())
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let name = identities.first().cloned();
                let pad = v
                    .get("presentation_attack_detection")
                    .cloned()
                    .unwrap_or(json!({}));
                let status = pad
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("not_applicable")
                    .to_string();
                let calibrated = pad
                    .get("security_calibrated")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                (name.is_some(), name, status, calibrated)
            }
            Err(e) => {
                let detail = match e {
                    DeviceError::NoFreshFrame(_) => json!({"error": "video_no_frame"}),
                    other => json!({"error": other.to_string()}),
                };
                steps.push(GateResult {
                    gate: "face".into(),
                    passed: false,
                    detail: detail.clone(),
                });
                return UnlockResult {
                    ok: false,
                    steps,
                    speech: "不能开门：摄像头暂时没有可用画面。".into(),
                };
            }
        };
        steps.push(GateResult {
            gate: "face".into(),
            passed: face_ok,
            detail: json!({"name": face_name}),
        });
        if !face_ok {
            return UnlockResult {
                ok: false,
                steps,
                speech: "不能开门：人脸没认出来。".into(),
            };
        }

        let pad_pass = match (self.pad_mode, pad_status.as_str()) {
            (PadMode::Warn, _) => true, // warn 档记录不阻断
            (PadMode::Enforce, "model_live") => true,
            (PadMode::Enforce, _) => false,
        };
        steps.push(GateResult {
            gate: "pad".into(),
            passed: pad_pass,
            detail: json!({"status": pad_status, "security_calibrated": pad_calibrated}),
        });
        if !pad_pass {
            return UnlockResult {
                ok: false,
                steps,
                speech: "不能开门：防伪检测未通过。".into(),
            };
        }

        // 闸 4：声纹
        let vp_result = match self.voiceprint {
            Some(vp) => vp.verify(self.samples, self.rate, self.voiceprint_threshold),
            None => Err(crate::voiceprint::VoiceprintError::EmptyLibrary),
        };
        let vp_pass = matches!(
            &vp_result,
            Ok(r) if r.name.is_some()
        );
        let vp_detail = match &vp_result {
            Ok(r) => json!({"name": r.name, "score": r.score, "threshold": r.threshold}),
            Err(e) => json!({"error": e.to_string()}),
        };
        steps.push(GateResult {
            gate: "voiceprint".into(),
            passed: vp_pass,
            detail: vp_detail,
        });
        if !vp_pass {
            return UnlockResult {
                ok: false,
                steps,
                speech: "不能开门：声音不像登记的人。".into(),
            };
        }

        // 全过 → 执行解锁（actuate_unlock 占位：旧系统写事件文件；此处走设备操作）
        let actuate = registry.execute("lock.door", "unlock", &json!({}), "voice", caps, token);
        let (unlocked, speech) = match actuate {
            Ok(id) => {
                let rec = registry.operation(id).cloned();
                match rec.map(|r| r.state) {
                    Some(loomos_device::core::OperationState::Succeeded) => {
                        let mut s = "验证通过，门已打开。".to_string();
                        if !pad_calibrated && self.pad_mode == PadMode::Warn {
                            s.push_str("防伪未经安全标定，本次不依据其结论。");
                        }
                        (true, s)
                    }
                    Some(loomos_device::core::OperationState::Unknown) => {
                        (false, "开锁指令已发出但结果未知，请现场确认。".to_string())
                    }
                    _ => (false, "开锁执行失败。".to_string()),
                }
            }
            Err(_) => (false, "开锁执行失败。".to_string()),
        };
        if unlocked {
            tracing::warn!("UNLOCK EVENT: action=unlock (四道闸全过)");
        }
        UnlockResult {
            ok: unlocked,
            steps,
            speech,
        }
    }
}

/// 把设备 face_match 结果从 OpOutcome 解包（供 UnlockFlow 使用）。
pub fn face_result_to_value(
    r: &Result<OpOutcome, DeviceError>,
) -> Result<serde_json::Value, DeviceError> {
    match r {
        Ok(OpOutcome::Done(v)) => Ok(v.clone()),
        Ok(OpOutcome::UnknownOutcome { reason }) => {
            Err(DeviceError::Failed(format!("unknown: {reason}")))
        }
        Err(e) => Err(e.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voiceprint::DEFAULT_THRESHOLD;
    use loomos_device::core::{DeviceLifecycle, DeviceRegistry};
    use loomos_device::mock::{MockFaults, MockHomeProvider};
    use loomos_runtime::capability::{CapabilityStore, Rights};
    use loomos_runtime::event::EventLog;
    use serde_json::json;

    fn fixture_registry(
        face_ids: Option<Vec<String>>,
        pad: &str,
    ) -> (
        DeviceRegistry,
        CapabilityStore,
        loomos_runtime::capability::Token,
    ) {
        let prov = MockHomeProvider::new(MockFaults {
            face_identities: face_ids,
            pad_status: pad.to_string(),
            pad_security_calibrated: false,
            voiceprint_match: None,
            view_ready: true,
            ..Default::default()
        });
        let mut reg = DeviceRegistry::new(std::sync::Arc::new(std::sync::Mutex::new(
            EventLog::in_memory("test"),
        )));
        reg.register_provider(Box::new(prov)).unwrap();
        for d in [
            "light.keting",
            "light.woshi",
            "camera.menkou",
            "switch.recorder",
            "camera.view",
            "lock.door",
            "speaker.tts",
            "system.host",
        ] {
            reg.set_lifecycle(d, DeviceLifecycle::Active).unwrap();
        }
        let mut caps = CapabilityStore::new();
        let tok = caps
            .grant(
                "voice",
                Rights::DeviceCommand {
                    device: "lock.door".into(),
                },
            )
            .unwrap();
        (reg, caps, tok)
    }

    fn tone(samples_len: usize) -> Vec<i16> {
        (0..samples_len)
            .map(|i| {
                ((440.0 * (i as f64 / 16_000.0) * std::f64::consts::TAU).sin() * 20000.0) as i16
            })
            .collect()
    }

    #[test]
    fn all_gates_pass_unlocks() {
        let (mut reg, caps, tok) = fixture_registry(Some(vec!["张三".into()]), "model_live");
        let mut vp = MockVoiceprint::new();
        let samples = tone(16_000);
        vp.enroll("张三", &samples, 16_000).unwrap();

        // 人脸结果直接构造（闸策略测试重点不在设备 IO，见 no_face_frame_fails）
        let face = Ok(json!({
            "identities": [{"name": "张三"}],
            "face_count": 1,
            "presentation_attack_detection": {"status": "model_live", "security_calibrated": false},
        }));
        let flow = UnlockFlow {
            pad_mode: PadMode::Warn,
            voiceprint_threshold: DEFAULT_THRESHOLD,
            voiceprint: Some(&vp),
            samples: &samples,
            rate: 16_000,
        };
        let result = flow.run("我回来了，开门", &face, &mut reg, &caps, &tok);
        assert!(result.ok, "{result:?}");
        assert!(result.speech.contains("门已打开"));
        assert!(result.speech.contains("防伪未经安全标定"));
        assert_eq!(result.steps.len(), 4);
        assert!(result.steps.iter().all(|s| s.passed));
    }

    #[test]
    fn wrong_phrase_fails_gate1() {
        let (mut reg, caps, tok) = fixture_registry(Some(vec!["张三".into()]), "model_live");
        let mut vp = MockVoiceprint::new();
        let samples = tone(16_000);
        vp.enroll("张三", &samples, 16_000).unwrap();
        let face = Ok(
            json!({"identities": [{"name": "张三"}], "presentation_attack_detection": {"status": "model_live", "security_calibrated": true}}),
        );
        let flow = UnlockFlow {
            pad_mode: PadMode::Warn,
            voiceprint_threshold: DEFAULT_THRESHOLD,
            voiceprint: Some(&vp),
            samples: &samples,
            rate: 16_000,
        };
        let r = flow.run("今天天气不错", &face, &mut reg, &caps, &tok);
        assert!(!r.ok);
        assert_eq!(r.speech, "这不是开门指令。");
        let _ = (&mut reg, &caps, &tok);
    }

    #[test]
    fn no_face_frame_fails() {
        let (mut reg, caps, tok) = fixture_registry(None, "not_applicable");
        let mut vp = MockVoiceprint::new();
        let samples = tone(16_000);
        vp.enroll("张三", &samples, 16_000).unwrap();
        // 走真实设备查询：无新鲜画面 → NoFreshFrame → face_result_to_value 映射为 Err
        let op_result = reg.execute(
            "camera.menkou",
            "face_match",
            &json!({}),
            "voice",
            &caps,
            &tok,
        );
        let face_inner: Result<serde_json::Value, DeviceError> = match op_result {
            Ok(id) => {
                let rec = reg.operation(id).unwrap();
                if rec.state == loomos_device::core::OperationState::Succeeded {
                    Ok(rec.result.clone().unwrap_or(json!({})))
                } else {
                    Err(DeviceError::NoFreshFrame("camera.menkou".into()))
                }
            }
            Err(e) => Err(e),
        };
        let face = face_result_to_value(&face_inner.map(OpOutcome::Done).map_err(|e| e.clone()));
        let flow = UnlockFlow {
            pad_mode: PadMode::Warn,
            voiceprint_threshold: DEFAULT_THRESHOLD,
            voiceprint: Some(&vp),
            samples: &samples,
            rate: 16_000,
        };
        let r = flow.run("我回来了", &face, &mut reg, &caps, &tok);
        assert!(!r.ok);
        assert!(r.speech.contains("没有可用画面"));
    }

    #[test]
    fn enforce_mode_blocks_uncalibrated_pad() {
        let (mut reg, caps, tok) = fixture_registry(Some(vec!["张三".into()]), "model_non_live");
        let mut vp = MockVoiceprint::new();
        let samples = tone(16_000);
        vp.enroll("张三", &samples, 16_000).unwrap();
        let face = Ok(json!({
            "identities": [{"name": "张三"}],
            "presentation_attack_detection": {"status": "model_non_live", "security_calibrated": false},
        }));
        let flow = UnlockFlow {
            pad_mode: PadMode::Enforce,
            voiceprint_threshold: DEFAULT_THRESHOLD,
            voiceprint: Some(&vp),
            samples: &samples,
            rate: 16_000,
        };
        let r = flow.run("我回来了", &face, &mut reg, &caps, &tok);
        assert!(!r.ok);
        assert!(r.speech.contains("防伪检测未通过"));
    }

    #[test]
    fn voiceprint_mismatch_fails() {
        let (mut reg, caps, tok) = fixture_registry(Some(vec!["张三".into()]), "model_live");
        let mut vp = MockVoiceprint::new();
        let enrolled = tone(16_000);
        vp.enroll("张三", &enrolled, 16_000).unwrap();
        // 不同音频
        let samples: Vec<i16> = (0..16_000)
            .map(|i| if i % 2 == 0 { 20000 } else { -20000 })
            .collect();
        let face = Ok(json!({
            "identities": [{"name": "张三"}],
            "presentation_attack_detection": {"status": "model_live", "security_calibrated": true},
        }));
        let flow = UnlockFlow {
            pad_mode: PadMode::Warn,
            voiceprint_threshold: DEFAULT_THRESHOLD,
            voiceprint: Some(&vp),
            samples: &samples,
            rate: 16_000,
        };
        let r = flow.run("我回来了", &face, &mut reg, &caps, &tok);
        assert!(!r.ok);
        assert!(r.speech.contains("声音不像"));
    }

    #[test]
    fn is_unlock_phrase_bidirectional_contains() {
        assert!(is_unlock_phrase("小克，我回来了，开门"));
        assert!(is_unlock_phrase("门开一下"));
        assert!(!is_unlock_phrase("今天天气不错"));
    }
}

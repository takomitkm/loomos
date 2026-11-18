//! Mock 家居提供者：原型默认档（全自检）——设备是确定性状态机 + 可注入故障。
//!
//! 设备集对齐旧板语义：
//! - light.keting / light.woshi：开关灯；
//! - camera.menkou（8091 语义）：人脸识别（identities + PAD）、拍照、健康；
//! - switch.recorder（8093 语义）：mode off/auto/manual（"on" 不存在——8093 返回
//!   `Mode must be off, auto or manual`，本 mock 沿用同一错误契约）；
//! - camera.view（8092 语义）：直播 ready/session 状态；
//! - lock.door：四道闸通过后执行 unlock；
//! - speaker.tts：播报（板上无声卡，mock 记录 last_text）；
//! - system.host：时间/状态/重启（撤销窗语义记录）。

use crate::core::{
    DeviceCapability, DeviceDescriptor, DeviceError, DeviceHealth, DeviceProvider, OpOutcome,
    ProviderId,
};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct MockFaults {
    /// face_match 恒定返回的 identities（None = 无新鲜画面 → video_no_frame）。
    pub face_identities: Option<Vec<String>>,
    /// PAD 状态（not_applicable / model_live / model_non_live / inconclusive / stale / not_evaluated）。
    pub pad_status: String,
    /// PAD 是否已安全标定（旧板实测：未标定）。
    pub pad_security_calibrated: bool,
    /// 声纹校验恒定结果（None = 无库）。
    pub voiceprint_match: Option<(String, f64)>,
    /// 直播 ready。
    pub view_ready: bool,
    /// 注入：设备执行挂死 → UnknownOutcome。
    pub hang_devices: Vec<String>,
    /// 注入：设备离线。
    pub offline_devices: Vec<String>,
    /// 重启窗口是否处于待取消状态。
    pub reboot_pending: bool,
}

#[derive(Debug, Clone)]
struct DeviceState {
    on: bool,
    mode: String, // recorder: off/auto/manual
}

pub struct MockHomeProvider {
    id: ProviderId,
    faults: MockFaults,
    /// 视频路可拆卸：false 时相机/录制/画面设备不注册（交给 VisionProvider/HttpBoard）。
    pub camera_enabled: bool,
    states: BTreeMap<String, DeviceState>,
    last_spoken: Option<String>,
    snapshots: Vec<String>,
}

impl MockHomeProvider {
    pub fn new(faults: MockFaults) -> Self {
        Self::with_camera(faults, true)
    }

    pub fn with_camera(faults: MockFaults, camera_enabled: bool) -> Self {
        let mut states = BTreeMap::new();
        for id in ["light.keting", "light.woshi"] {
            states.insert(
                id.to_string(),
                DeviceState {
                    on: false,
                    mode: "off".into(),
                },
            );
        }
        states.insert(
            "switch.recorder".into(),
            DeviceState {
                on: false,
                mode: "off".into(),
            },
        );
        Self {
            id: "mock-home".into(),
            faults,
            camera_enabled,
            states,
            last_spoken: None,
            snapshots: Vec::new(),
        }
    }

    pub fn last_spoken(&self) -> Option<&str> {
        self.last_spoken.as_deref()
    }

    pub fn snapshots(&self) -> &[String] {
        &self.snapshots
    }
}

impl DeviceProvider for MockHomeProvider {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn discover(&mut self) -> Vec<DeviceDescriptor> {
        let mut devs = vec![
            DeviceDescriptor {
                id: "light.keting".into(),
                provider: self.id.clone(),
                kind: "light".into(),
                version: 1,
                capabilities: vec![DeviceCapability::TurnOnOff, DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "light.woshi".into(),
                provider: self.id.clone(),
                kind: "light".into(),
                version: 1,
                capabilities: vec![DeviceCapability::TurnOnOff, DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "lock.door".into(),
                provider: self.id.clone(),
                kind: "lock".into(),
                version: 1,
                capabilities: vec![DeviceCapability::Unlock, DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "speaker.tts".into(),
                provider: self.id.clone(),
                kind: "speaker".into(),
                version: 1,
                capabilities: vec![DeviceCapability::Speak],
            },
            DeviceDescriptor {
                id: "system.host".into(),
                provider: self.id.clone(),
                kind: "system".into(),
                version: 1,
                capabilities: vec![
                    DeviceCapability::Reboot,
                    DeviceCapability::Time,
                    DeviceCapability::Status,
                ],
            },
        ];
        // 视频路三件（相机/录制/画面）可被拆卸：交给 VisionProvider 或 HttpBoardAdapter
        if self.camera_enabled {
            devs.push(DeviceDescriptor {
                id: "camera.menkou".into(),
                provider: self.id.clone(),
                kind: "face".into(),
                version: 1,
                capabilities: vec![
                    DeviceCapability::FaceMatch,
                    DeviceCapability::Capture,
                    DeviceCapability::Status,
                ],
            });
            devs.push(DeviceDescriptor {
                id: "switch.recorder".into(),
                provider: self.id.clone(),
                kind: "recorder".into(),
                version: 1,
                capabilities: vec![DeviceCapability::RecordMode, DeviceCapability::Status],
            });
            devs.push(DeviceDescriptor {
                id: "camera.view".into(),
                provider: self.id.clone(),
                kind: "camera".into(),
                version: 1,
                capabilities: vec![DeviceCapability::Status],
            });
        }
        devs
    }

    fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &serde_json::Value,
    ) -> Result<OpOutcome, DeviceError> {
        if self.faults.offline_devices.iter().any(|d| d == device) {
            return Err(DeviceError::NotAvailable(
                device.to_string(),
                crate::core::DeviceLifecycle::Offline,
            ));
        }
        if self.faults.hang_devices.iter().any(|d| d == device) {
            return Ok(OpOutcome::UnknownOutcome {
                reason: "注入挂死：操作已发出但结果未知".into(),
            });
        }
        match (device, op) {
            ("light.keting", "turn_on") | ("light.woshi", "turn_on") => {
                self.states.get_mut(device).unwrap().on = true;
                Ok(OpOutcome::Done(json!({"on": true, "entity_id": device})))
            }
            ("light.keting", "turn_off") | ("light.woshi", "turn_off") => {
                self.states.get_mut(device).unwrap().on = false;
                Ok(OpOutcome::Done(json!({"on": false, "entity_id": device})))
            }
            ("light.keting", "query") | ("light.woshi", "query") => {
                let on = self.states.get(device).unwrap().on;
                Ok(OpOutcome::Done(json!({"on": on, "entity_id": device})))
            }
            ("switch.recorder", "record_mode") => {
                let mode = params
                    .get("mode")
                    .and_then(|m| m.as_str())
                    .ok_or_else(|| DeviceError::BadParams("mode 必填".into()))?;
                // 8093 契约：mode 只接受 off/auto/manual
                if !matches!(mode, "off" | "auto" | "manual") {
                    return Err(DeviceError::BadParams(
                        "Mode must be off, auto or manual".into(),
                    ));
                }
                self.states.get_mut(device).unwrap().mode = mode.to_string();
                Ok(OpOutcome::Done(json!({"mode": mode})))
            }
            ("switch.recorder", "query") => {
                let st = self.states.get(device).unwrap();
                Ok(OpOutcome::Done(
                    json!({"mode": st.mode, "state": if st.mode=="off" {"idle"} else {"recording"}, "queue": 0}),
                ))
            }
            ("camera.menkou", "face_match") => match &self.faults.face_identities {
                Some(ids) if !ids.is_empty() => Ok(OpOutcome::Done(json!({
                    "identities": ids,
                    "face_count": ids.len(),
                    "presentation_attack_detection": {
                        "status": self.faults.pad_status,
                        "security_calibrated": self.faults.pad_security_calibrated,
                    },
                }))),
                Some(_) => Err(DeviceError::NoFreshFrame(device.to_string())),
                None => Err(DeviceError::NoFreshFrame(device.to_string())),
            },
            ("camera.menkou", "capture") => {
                let name = format!("snap-{}", self.snapshots.len() + 1);
                self.snapshots.push(name.clone());
                Ok(OpOutcome::Done(json!({"snapshot": name})))
            }
            ("camera.menkou", "query") | ("camera.view", "query") => Ok(OpOutcome::Done(json!({
                "ready": self.faults.view_ready,
                "session": if self.faults.view_ready { json!("live-1") } else { serde_json::Value::Null },
            }))),
            ("lock.door", "unlock") => {
                // 四道闸证据由上层（normos-home）验证后传入；此处只执行
                Ok(OpOutcome::Done(json!({"unlocked": true, "at": "now"})))
            }
            ("lock.door", "query") => Ok(OpOutcome::Done(json!({"locked": true}))),
            ("speaker.tts", "speak") => {
                let text = params
                    .get("text")
                    .and_then(|t| t.as_str())
                    .ok_or_else(|| DeviceError::BadParams("text 必填".into()))?;
                self.last_spoken = Some(text.to_string());
                Ok(OpOutcome::Done(json!({"spoken": text})))
            }
            ("system.host", "time") => Ok(OpOutcome::Done(json!({"time": "12:00"}))),
            ("system.host", "reboot") => {
                self.faults.reboot_pending = true;
                Ok(OpOutcome::Done(
                    json!({"reboot_scheduled": true, "cancel_window_s": 60}),
                ))
            }
            ("system.host", "cancel_reboot") => {
                self.faults.reboot_pending = false;
                Ok(OpOutcome::Done(json!({"reboot_cancelled": true})))
            }
            ("system.host", "query") => Ok(OpOutcome::Done(json!({
                "healthy": true,
                "reboot_pending": self.faults.reboot_pending,
            }))),
            _ => Err(DeviceError::UnsupportedOp(
                device.to_string(),
                op.to_string(),
            )),
        }
    }

    fn health(&self, device: &str) -> Result<DeviceHealth, DeviceError> {
        if self.faults.offline_devices.iter().any(|d| d == device) {
            return Ok(DeviceHealth::Faulted);
        }
        Ok(DeviceHealth::Ok)
    }

    fn telemetry(&self, device: &str) -> Result<serde_json::Value, DeviceError> {
        match device {
            "switch.recorder" => {
                let st = self.states.get(device).unwrap();
                Ok(json!({"mode": st.mode, "trash_count": 26}))
            }
            "camera.menkou" => {
                Ok(json!({"pad_security_calibrated": self.faults.pad_security_calibrated}))
            }
            _ => Ok(json!({})),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_rejects_invalid_mode_like_board() {
        let mut p = MockHomeProvider::new(MockFaults::default());
        let err = p
            .execute("switch.recorder", "record_mode", &json!({"mode": "on"}))
            .unwrap_err();
        assert!(err.to_string().contains("Mode must be off, auto or manual"));
        let ok = p
            .execute("switch.recorder", "record_mode", &json!({"mode": "auto"}))
            .unwrap();
        if let OpOutcome::Done(v) = ok {
            assert_eq!(v["mode"], "auto");
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn face_gate_semantics() {
        let mut p = MockHomeProvider::new(MockFaults {
            face_identities: Some(vec![]),
            ..Default::default()
        });
        // 无新鲜画面 → NoFreshFrame（503 契约）
        let err = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap_err();
        assert!(matches!(err, DeviceError::NoFreshFrame(_)));
        // 认出人 → identities
        p.faults.face_identities = Some(vec!["张三".into()]);
        let out = p
            .execute("camera.menkou", "face_match", &json!({}))
            .unwrap();
        if let OpOutcome::Done(v) = out {
            assert_eq!(v["face_count"], 1);
        } else {
            panic!("应为 Done");
        }
    }

    #[test]
    fn hang_returns_unknown_not_failed() {
        let mut p = MockHomeProvider::new(MockFaults {
            hang_devices: vec!["camera.menkou".into()],
            ..Default::default()
        });
        let out = p.execute("camera.menkou", "capture", &json!({})).unwrap();
        assert!(matches!(out, OpOutcome::UnknownOutcome { .. }));
    }
}

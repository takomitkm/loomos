//! 动作注册表：Intent + slots → 设备操作（旧 executor 双通道的落点）。

use normos_device::core::{DeviceError, DeviceRegistry, OpOutcome};
use normos_nlu::intents::{Intent, IntentName};
use normos_runtime::capability::{CapabilityStore, Token};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// 一条动作的执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOutcome {
    pub intent: String,
    pub device: String,
    pub op: String,
    pub state: String, // succeeded | failed | unknown
    pub result: serde_json::Value,
    pub error: Option<String>,
    /// 可念回执文本（旧系统 _speak_out 口径：取动作 message；LLM/生成文本不冒充执行回执）。
    pub speech: Option<String>,
}

impl ActionOutcome {
    fn from_op(
        intent: &IntentName,
        device: &str,
        op: &str,
        _id: normos_runtime::ids::OperationId,
        rec: &normos_device::core::OperationRecord,
    ) -> Self {
        let state = format!("{:?}", rec.state).to_lowercase();
        let speech = match rec.state {
            normos_device::core::OperationState::Succeeded => {
                op_success_speech(intent, op, rec.result.as_ref())
            }
            normos_device::core::OperationState::Unknown => {
                Some("操作已发出但结果未知，稍后请再确认。".to_string())
            }
            _ => None,
        };
        Self {
            intent: intent.as_str().to_string(),
            device: device.to_string(),
            op: op.to_string(),
            state,
            result: rec.result.clone().unwrap_or(json!({})),
            error: rec.error.clone(),
            speech,
        }
    }
}

fn op_success_speech(
    intent: &IntentName,
    op: &str,
    result: Option<&serde_json::Value>,
) -> Option<String> {
    let _ = op;
    match intent {
        IntentName::HassTurnOn => result
            .and_then(|r| r.get("entity_id"))
            .and_then(|e| e.as_str())
            .map(|entity_id| format_entity_speech(entity_id, "开了。")),
        IntentName::HassTurnOff => result
            .and_then(|r| r.get("entity_id"))
            .and_then(|e| e.as_str())
            .map(|entity_id| format_entity_speech(entity_id, "关了。")),
        _ => None,
    }
}

/// entity_id → 中文播报（light.keting → 客厅的灯）。
pub fn format_entity_speech(entity_id: &str, suffix: &str) -> String {
    let (domain, name) = entity_id.split_once('.').unwrap_or((entity_id, ""));
    let zh = match domain {
        "light" => match name {
            "keting" => "客厅的灯",
            "woshi" => "卧室的灯",
            _ => "灯",
        },
        "switch" => "开关",
        _ => name,
    };
    format!("{zh}{suffix}")
}

/// 动作注册表：把意图编译成设备操作并执行。
pub struct ActionRegistry;

impl ActionRegistry {
    /// 执行意图（多动作顺序执行）。返回每条操作的结果。
    pub fn run(
        intent: &Intent,
        registry: &mut DeviceRegistry,
        principal: &str,
        caps: &CapabilityStore,
        token: &Token,
    ) -> Vec<ActionOutcome> {
        match intent.name {
            IntentName::HassTurnOn | IntentName::HassTurnOff => {
                let entity_id = intent.slot("entity_id").unwrap_or_default().to_string();
                let op = if intent.name == IntentName::HassTurnOn {
                    "turn_on"
                } else {
                    "turn_off"
                };
                vec![exec(
                    registry,
                    &entity_id,
                    op,
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::WhoIsThere => {
                vec![exec(
                    registry,
                    "camera.menkou",
                    "face_match",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::StartRecording => {
                vec![exec(
                    registry,
                    "switch.recorder",
                    "record_mode",
                    &json!({"mode": "manual"}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::AutoRecording => {
                vec![exec(
                    registry,
                    "switch.recorder",
                    "record_mode",
                    &json!({"mode": "auto"}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::StopRecording => {
                vec![exec(
                    registry,
                    "switch.recorder",
                    "record_mode",
                    &json!({"mode": "off"}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::CapturePhoto => {
                vec![exec(
                    registry,
                    "camera.menkou",
                    "capture",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::QueryStatus => {
                // 旧 act_status：并发读三服务；原型顺序读三设备
                let mut outs = Vec::new();
                for (dev, op) in [
                    ("camera.menkou", "query"),
                    ("switch.recorder", "query"),
                    ("camera.view", "query"),
                ] {
                    outs.push(exec(
                        registry,
                        dev,
                        op,
                        &json!({}),
                        intent.name,
                        principal,
                        caps,
                        token,
                    ));
                }
                outs
            }
            IntentName::QueryTime => {
                vec![exec(
                    registry,
                    "system.host",
                    "time",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::QueryDoorStatus => {
                vec![exec(
                    registry,
                    "camera.menkou",
                    "query",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::StartVoiceprint => {
                vec![exec(
                    registry,
                    "camera.menkou",
                    "voiceprint_status",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::RebootSystem => {
                vec![exec(
                    registry,
                    "system.host",
                    "reboot",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            IntentName::CancelReboot => {
                vec![exec(
                    registry,
                    "system.host",
                    "cancel_reboot",
                    &json!({}),
                    intent.name,
                    principal,
                    caps,
                    token,
                )]
            }
            // DialogueReply / ArriveHome 不经此表（对话固定应答由 executor 处理；开门走 door 模块）
            IntentName::DialogueReply | IntentName::ArriveHome => Vec::new(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn exec(
    registry: &mut DeviceRegistry,
    device: &str,
    op: &str,
    params: &serde_json::Value,
    intent_name: IntentName,
    principal: &str,
    caps: &CapabilityStore,
    token: &Token,
) -> ActionOutcome {
    match registry.execute(device, op, params, principal, caps, token) {
        Ok(op_id) => {
            let rec = registry
                .operation(op_id)
                .cloned()
                .unwrap_or_else(|| OperationFallback::record(device, op));
            ActionOutcome::from_op(&intent_name, device, op, op_id, &rec)
        }
        Err(e) => ActionOutcome {
            intent: intent_name.as_str().to_string(),
            device: device.to_string(),
            op: op.to_string(),
            state: "failed".into(),
            result: json!({}),
            error: Some(match &e {
                DeviceError::NoFreshFrame(_) => "video_no_frame".to_string(),
                other => other.to_string(),
            }),
            speech: Some(match &e {
                DeviceError::NoFreshFrame(_) => "摄像头暂时没有新鲜画面。".to_string(),
                DeviceError::PermissionDenied(_) => "权限不足，不能执行这个操作。".to_string(),
                other => format!("操作失败：{other}"),
            }),
        },
    }
}

/// operation 记录缺失时的兜底（不应发生，防御性）。
struct OperationFallback;
impl OperationFallback {
    fn record(device: &str, op: &str) -> normos_device::core::OperationRecord {
        normos_device::core::OperationRecord {
            id: normos_runtime::ids::OperationId::next(),
            device: device.to_string(),
            op: op.to_string(),
            params: json!({}),
            state: normos_device::core::OperationState::Failed,
            principal: String::new(),
            submitted_ms: 0,
            finished_ms: None,
            result: None,
            error: Some("operation record missing".into()),
        }
    }
}

#[allow(dead_code)]
fn _unused(_: OpOutcome) {}

#![forbid(unsafe_code)]

//! loomos 执行器：文本 → 意图 → 动作 → 回执文本。
//!
//! 双通道分流纪律（旧 klar_executor.handle_text）：
//! 1. 通道 B：整句直连表精确匹配（确定性、~0 开销）；
//! 2. 通道 A：NLU 语法/外部引擎解析；
//! 3. 命中 DialogueReply 直接回固定应答；ArriveHome 走四道闸；
//!    RebootSystem 需一次性确认；其余意图走动作注册表；
//! 4. 回执只取结构化结果或固定应答，不编造。

use loomos_device::core::{DeviceError, DeviceRegistry, OperationState};
use loomos_home::actions::{ActionOutcome, ActionRegistry};
use loomos_home::confirm::{ConfirmationStore, RiskyActionError};
use loomos_home::door::{PadMode, UnlockFlow, UnlockResult};
use loomos_home::voiceprint::MockVoiceprint;
use loomos_nlu::grammar::{NluDecision, RuleNlu};
use loomos_nlu::intents::{Intent, IntentName, MatchChannel};
use loomos_runtime::capability::{CapabilityStore, Token};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Mutex;

/// 一次文本处理的完整回执。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecReport {
    pub text: String,
    /// 命中的意图（没听懂时为 None）。
    pub intent: Option<String>,
    pub channel: Option<String>, // direct | grammar
    pub actions: Vec<ActionOutcome>,
    /// 可念回执。
    pub speech: String,
    /// 危险动作是否被拒。
    pub rejected: bool,
    /// ArriveHome 的四道闸明细。
    pub unlock: Option<UnlockResult>,
}

/// 执行器配置。
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    pub pad_mode: PadMode,
    pub voiceprint_threshold: f64,
    pub principal: String,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            pad_mode: PadMode::Warn,
            voiceprint_threshold: loomos_home::voiceprint::DEFAULT_THRESHOLD,
            principal: "voice".into(),
        }
    }
}

/// 双通道执行器。
pub struct Executor {
    nlu: RuleNlu,
    config: ExecutorConfig,
    confirmations: Mutex<ConfirmationStore>,
}

impl Executor {
    pub fn new(nlu: RuleNlu, config: ExecutorConfig) -> Self {
        Self {
            nlu,
            config,
            confirmations: Mutex::new(ConfirmationStore::new()),
        }
    }

    pub fn config(&self) -> &ExecutorConfig {
        &self.config
    }

    pub fn nlu(&self) -> &RuleNlu {
        &self.nlu
    }

    pub fn confirmations(&self) -> &Mutex<ConfirmationStore> {
        &self.confirmations
    }

    /// 为危险动作签发确认码（console/repl 路径用；语音路径由四道闸自证）。
    pub fn issue_confirmation(&self, action: &str, params: &serde_json::Value) -> String {
        self.confirmations
            .lock()
            .unwrap()
            .issue(action, params, &self.config.principal)
            .code
    }

    /// 主入口：handle_text(text)。
    #[allow(clippy::too_many_arguments)]
    pub fn handle_text(
        &self,
        text: &str,
        registry: &mut DeviceRegistry,
        caps: &CapabilityStore,
        token: &Token,
        voiceprint: Option<&MockVoiceprint>,
        samples: &[i16],
        rate: u32,
    ) -> ExecReport {
        match self.nlu.parse(text) {
            NluDecision::Execute(intent) => self.execute_intent(
                &intent, text, registry, caps, token, voiceprint, samples, rate,
            ),
            NluDecision::Clarify { speech } => ExecReport {
                text: text.to_string(),
                intent: None,
                channel: Some("grammar".into()),
                actions: vec![],
                speech,
                rejected: false,
                unlock: None,
            },
            NluDecision::Reject { speech } => ExecReport {
                text: text.to_string(),
                intent: None,
                channel: None,
                actions: vec![],
                speech,
                rejected: false,
                unlock: None,
            },
        }
    }

    /// 执行意图（含危险动作闸口）。
    #[allow(clippy::too_many_arguments)]
    pub fn execute_intent(
        &self,
        intent: &Intent,
        text: &str,
        registry: &mut DeviceRegistry,
        caps: &CapabilityStore,
        token: &Token,
        voiceprint: Option<&MockVoiceprint>,
        samples: &[i16],
        rate: u32,
    ) -> ExecReport {
        match intent.name {
            // 固定应答（旧 act_dialogue_reply：end_request）
            IntentName::DialogueReply => {
                let reply = intent.slot("reply").unwrap_or("我在。").to_string();
                ExecReport {
                    text: text.to_string(),
                    intent: Some(IntentName::DialogueReply.as_str().into()),
                    channel: Some(channel_name(&intent.matched_by)),
                    actions: vec![],
                    speech: reply,
                    rejected: false,
                    unlock: None,
                }
            }
            // 四道闸开门
            IntentName::ArriveHome => {
                // 人脸：先查设备
                let face_val: Result<serde_json::Value, DeviceError> = match registry.execute(
                    "camera.menkou",
                    "face_match",
                    &json!({}),
                    &self.config.principal,
                    caps,
                    token,
                ) {
                    Ok(id) => match registry.operation(id).cloned() {
                        Some(r) if r.state == OperationState::Succeeded => {
                            Ok(r.result.unwrap_or(json!({})))
                        }
                        Some(r) if r.state == OperationState::Unknown => {
                            Err(DeviceError::Failed("unknown outcome".into()))
                        }
                        Some(r) => Err(r
                            .error
                            .map(DeviceError::Failed)
                            .unwrap_or_else(|| DeviceError::Failed("face failed".into()))),
                        None => Err(DeviceError::Failed("no record".into())),
                    },
                    Err(e) => Err(e),
                };
                let flow = UnlockFlow {
                    pad_mode: self.config.pad_mode,
                    voiceprint_threshold: self.config.voiceprint_threshold,
                    voiceprint,
                    samples,
                    rate,
                };
                let unlock = flow.run(text, &face_val, registry, caps, token);
                ExecReport {
                    text: text.to_string(),
                    intent: Some(IntentName::ArriveHome.as_str().into()),
                    channel: Some(channel_name(&intent.matched_by)),
                    actions: vec![],
                    speech: unlock.speech.clone(),
                    rejected: !unlock.ok,
                    unlock: Some(unlock),
                }
            }
            // 重启：需一次性确认
            IntentName::RebootSystem => {
                let mut confirm = self.confirmations.lock().unwrap();
                let params = json!({"window_s": 60});
                let result = confirm.consume(
                    intent.slot("confirm_code").unwrap_or(""),
                    "system.reboot",
                    &params,
                    &self.config.principal,
                );
                match result {
                    Ok(()) => {
                        let outs = ActionRegistry::run(
                            intent,
                            registry,
                            &self.config.principal,
                            caps,
                            token,
                        );
                        ExecReport {
                            text: text.to_string(),
                            intent: Some(IntentName::RebootSystem.as_str().into()),
                            channel: Some(channel_name(&intent.matched_by)),
                            speech: outs
                                .first()
                                .and_then(|o| o.speech.clone())
                                .unwrap_or_else(|| "重启已排定。".into()),
                            actions: outs,
                            rejected: false,
                            unlock: None,
                        }
                    }
                    Err(RiskyActionError::NoPendingConfirmation(_)) => {
                        // 语音通道没有确认码输入 → 生成待确认并提示（真实路径：console 下发确认码）
                        let c = confirm.issue("system.reboot", &params, &self.config.principal);
                        ExecReport {
                            text: text.to_string(),
                            intent: Some(IntentName::RebootSystem.as_str().into()),
                            channel: Some(channel_name(&intent.matched_by)),
                            actions: vec![],
                            speech: format!("重启是危险操作，请输入确认码 {} 后再说一次。", c.code),
                            rejected: true,
                            unlock: None,
                        }
                    }
                    Err(e) => ExecReport {
                        text: text.to_string(),
                        intent: Some(IntentName::RebootSystem.as_str().into()),
                        channel: Some(channel_name(&intent.matched_by)),
                        actions: vec![],
                        speech: format!("确认无效：{e}。"),
                        rejected: true,
                        unlock: None,
                    },
                }
            }
            // 其余意图走动作注册表
            other => {
                let outs =
                    ActionRegistry::run(intent, registry, &self.config.principal, caps, token);
                let speech = self.compose_speech(intent, &outs);
                ExecReport {
                    text: text.to_string(),
                    intent: Some(other.as_str().into()),
                    channel: Some(channel_name(&intent.matched_by)),
                    actions: outs,
                    speech,
                    rejected: false,
                    unlock: None,
                }
            }
        }
    }

    /// 把动作结果拼成可念回执（取结构化结果或固定应答；不编造）。
    fn compose_speech(&self, intent: &Intent, outs: &[ActionOutcome]) -> String {
        match intent.name {
            IntentName::WhoIsThere => {
                // 旧 act_face_recognize：念识别到的人名
                if let Some(o) = outs.first() {
                    if o.state == "succeeded" {
                        let ids: Vec<String> = o
                            .result
                            .get("identities")
                            .and_then(|i| i.as_array())
                            .map(|a| {
                                a.iter()
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
                        if ids.is_empty() {
                            return "门口没有人。".into();
                        }
                        return format!("门口是{}。", ids.join("、"));
                    }
                    if o.error
                        .as_deref()
                        .map(|e| e.contains("video_no_frame"))
                        .unwrap_or(false)
                    {
                        return "摄像头暂时没有新鲜画面。".into();
                    }
                }
                "人脸识别没有成功。".into()
            }
            IntentName::QueryStatus => {
                let mut parts = Vec::new();
                for o in outs {
                    if o.state != "succeeded" {
                        parts.push(format!("{}不可用", o.device));
                        continue;
                    }
                    match o.device.as_str() {
                        "camera.menkou" => parts.push("人脸服务正常".into()),
                        "switch.recorder" => {
                            let mode = o
                                .result
                                .get("mode")
                                .and_then(|m| m.as_str())
                                .unwrap_or("off");
                            parts.push(format!("录制模式{}", zh_mode(mode)));
                        }
                        "camera.view" => {
                            let ready = o
                                .result
                                .get("ready")
                                .and_then(|r| r.as_bool())
                                .unwrap_or(false);
                            parts.push(if ready {
                                "直播正常".into()
                            } else {
                                "直播未就绪".into()
                            });
                        }
                        _ => {}
                    }
                }
                if parts.is_empty() {
                    "当前没有可报告的状态。".into()
                } else {
                    format!("{}。", parts.join("，"))
                }
            }
            IntentName::StartRecording | IntentName::AutoRecording => "开始录制。".into(),
            IntentName::StopRecording => "已停止录制。".into(),
            IntentName::CapturePhoto => "照片已存档。".into(),
            IntentName::QueryTime => {
                let t = outs
                    .first()
                    .and_then(|o| o.result.get("time"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("未知");
                format!("现在是{t}。")
            }
            IntentName::QueryDoorStatus => {
                let o = outs.first();
                let calibrated = o
                    .and_then(|x| x.result.get("pad_security_calibrated"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false);
                if o.map(|x| x.state == "succeeded").unwrap_or(false) {
                    format!(
                        "门禁服务在线，防伪{}。",
                        if calibrated { "已标定" } else { "未标定" }
                    )
                } else {
                    "门禁服务无响应。".into()
                }
            }
            IntentName::StartVoiceprint => "声纹服务就绪。".into(),
            IntentName::CancelReboot => "已取消重启。".into(),
            _ => {
                // HassTurnOn/Off 的播报由动作侧给出（客厅的灯开了。）
                outs.iter()
                    .find_map(|o| o.speech.clone())
                    .unwrap_or_else(|| "已完成。".into())
            }
        }
    }
}

fn channel_name(c: &MatchChannel) -> String {
    match c {
        MatchChannel::Direct => "direct".into(),
        MatchChannel::Grammar => "grammar".into(),
    }
}

fn zh_mode(mode: &str) -> &str {
    match mode {
        "auto" => "自动",
        "manual" => "手动",
        _ => "关闭",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomos_device::core::DeviceLifecycle;
    use loomos_device::mock::{MockFaults, MockHomeProvider};
    use loomos_runtime::capability::Rights;
    use loomos_runtime::event::EventLog;

    fn setup(face_ids: Option<Vec<String>>, pad: &str) -> (DeviceRegistry, CapabilityStore, Token) {
        let prov = MockHomeProvider::new(MockFaults {
            face_identities: face_ids,
            pad_status: pad.to_string(),
            pad_security_calibrated: false,
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
        // 最小授权：voice 对全部 mock 设备有 command + observe（不给 Admin——见 capability 模块）
        let mut tok_grants = Vec::new();
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
            tok_grants.push(
                caps.grant("voice", Rights::DeviceCommand { device: d.into() })
                    .unwrap(),
            );
        }
        let tok = tok_grants[0].clone();
        let _ = &tok_grants;
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
    fn turn_on_light_end_to_end() {
        let (mut reg, caps, tok) = setup(None, "not_applicable");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text(
            "小克，打开客厅的灯",
            &mut reg,
            &caps,
            &tok,
            None,
            &[],
            16_000,
        );
        assert_eq!(r.intent.as_deref(), Some("HassTurnOn"));
        assert_eq!(r.channel.as_deref(), Some("grammar"));
        assert_eq!(r.speech, "客厅的灯开了。");
        assert_eq!(r.actions.len(), 1);
        assert_eq!(r.actions[0].state, "succeeded");
    }

    #[test]
    fn who_is_there_speaks_name() {
        let (mut reg, caps, tok) = setup(Some(vec!["张三".into()]), "model_live");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text("小克，看看是谁", &mut reg, &caps, &tok, None, &[], 16_000);
        assert_eq!(r.speech, "门口是张三。");
    }

    #[test]
    fn who_is_there_no_frame() {
        let (mut reg, caps, tok) = setup(Some(vec![]), "not_applicable");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text("小克，看看是谁", &mut reg, &caps, &tok, None, &[], 16_000);
        assert_eq!(r.speech, "摄像头暂时没有新鲜画面。");
    }

    #[test]
    fn dialogue_canned_reply() {
        let (mut reg, caps, tok) = setup(None, "not_applicable");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text("小克小克", &mut reg, &caps, &tok, None, &[], 16_000);
        assert_eq!(r.speech, "我在。");
        assert_eq!(r.channel.as_deref(), Some("direct"));
    }

    #[test]
    fn reboot_requires_confirmation() {
        let (mut reg, caps, tok) = setup(None, "not_applicable");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r1 = ex.handle_text("帮我重启板子", &mut reg, &caps, &tok, None, &[], 16_000);
        assert!(r1.rejected);
        assert!(r1.speech.contains("确认码"));
        let code = r1
            .speech
            .split(' ')
            .find(|s| s.starts_with("CONF-"))
            .unwrap_or("")
            .trim_end_matches('。')
            .to_string();
        assert!(!code.is_empty(), "回执应含确认码: {}", r1.speech);
        let mut intent = Intent {
            name: IntentName::RebootSystem,
            slots: Default::default(),
            matched_by: MatchChannel::Direct,
        };
        intent.slots.insert("confirm_code".into(), code);
        let r2 = ex.execute_intent(
            &intent,
            "帮我重启板子",
            &mut reg,
            &caps,
            &tok,
            None,
            &[],
            16_000,
        );
        assert!(!r2.rejected);
        // 用过即废
        let mut intent3 = Intent {
            name: IntentName::RebootSystem,
            slots: Default::default(),
            matched_by: MatchChannel::Direct,
        };
        intent3
            .slots
            .insert("confirm_code".into(), "CONF-9999".into());
        let r3 = ex.execute_intent(
            &intent3,
            "帮我重启板子",
            &mut reg,
            &caps,
            &tok,
            None,
            &[],
            16_000,
        );
        assert!(r3.rejected);
    }

    #[test]
    fn unknown_text_rejects() {
        let (mut reg, caps, tok) = setup(None, "not_applicable");
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text(
            "小克，今天天气怎么样",
            &mut reg,
            &caps,
            &tok,
            None,
            &[],
            16_000,
        );
        assert!(r.intent.is_none());
        assert!(r.speech.contains("没听懂"));
    }

    #[test]
    fn unlock_happy_path_via_executor() {
        let (mut reg, caps, tok) = setup(Some(vec!["张三".into()]), "model_live");
        let mut vp = MockVoiceprint::new();
        let samples = tone(16_000);
        vp.enroll("张三", &samples, 16_000).unwrap();
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text(
            "小克，我回来了",
            &mut reg,
            &caps,
            &tok,
            Some(&vp),
            &samples,
            16_000,
        );
        assert_eq!(r.intent.as_deref(), Some("ArriveHome"));
        assert!(r.speech.contains("门已打开"), "{}", r.speech);
        let unlock = r.unlock.unwrap();
        assert_eq!(unlock.steps.len(), 4);
    }

    #[test]
    fn unlock_voiceprint_fail_denies() {
        let (mut reg, caps, tok) = setup(Some(vec!["张三".into()]), "model_live");
        let mut vp = MockVoiceprint::new();
        let enrolled = tone(16_000);
        vp.enroll("张三", &enrolled, 16_000).unwrap();
        let other: Vec<i16> = (0..16_000)
            .map(|i| if i % 2 == 0 { 20000 } else { -20000 })
            .collect();
        let ex = Executor::new(
            RuleNlu::new(loomos_nlu::registry::EntityRegistry::default_home()),
            ExecutorConfig::default(),
        );
        let r = ex.handle_text(
            "小克，我回来了",
            &mut reg,
            &caps,
            &tok,
            Some(&vp),
            &other,
            16_000,
        );
        assert!(r.rejected);
        assert!(r.speech.contains("声音不像"));
    }
}

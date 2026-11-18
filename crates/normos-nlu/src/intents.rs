//! 意图与槽位（命名兼容 klar/HA 的 HassXxx 家族，便于外部引擎互译）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentName {
    /// 打开设备。
    HassTurnOn,
    /// 关闭设备。
    HassTurnOff,
    /// 并发查询各服务状态。
    QueryStatus,
    /// 人脸识别（看看是谁）。
    WhoIsThere,
    /// 手动开始录制。
    StartRecording,
    /// 有人时自动录制。
    AutoRecording,
    /// 停止录制。
    StopRecording,
    /// 拍照存档。
    CapturePhoto,
    /// 我回来了（四道闸开门，危险动作）。
    ArriveHome,
    /// 对话固定应答（小克小克/再见）。
    DialogueReply,
    /// 现在几点。
    QueryTime,
    /// 门禁状态。
    QueryDoorStatus,
    /// 启动声纹识别。
    StartVoiceprint,
    /// 重启系统（危险动作，留撤销窗）。
    RebootSystem,
    /// 取消重启。
    CancelReboot,
}

impl IntentName {
    pub fn as_str(&self) -> &'static str {
        match self {
            IntentName::HassTurnOn => "HassTurnOn",
            IntentName::HassTurnOff => "HassTurnOff",
            IntentName::QueryStatus => "QueryStatus",
            IntentName::WhoIsThere => "WhoIsThere",
            IntentName::StartRecording => "StartRecording",
            IntentName::AutoRecording => "AutoRecording",
            IntentName::StopRecording => "StopRecording",
            IntentName::CapturePhoto => "CapturePhoto",
            IntentName::ArriveHome => "ArriveHome",
            IntentName::DialogueReply => "DialogueReply",
            IntentName::QueryTime => "QueryTime",
            IntentName::QueryDoorStatus => "QueryDoorStatus",
            IntentName::StartVoiceprint => "StartVoiceprint",
            IntentName::RebootSystem => "RebootSystem",
            IntentName::CancelReboot => "CancelReboot",
        }
    }

    /// 是否危险动作（需要确认/多因子，旧系统 confirm_risky_actions 纪律）。
    pub fn is_risky(&self) -> bool {
        matches!(self, IntentName::ArriveHome | IntentName::RebootSystem)
    }
}

/// 槽位集合（entity_id / area / mode / reply 等自由键值）。
pub type Slots = BTreeMap<String, String>;

/// 解析结果：意图 + 槽位 + 命中通道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub name: IntentName,
    pub slots: Slots,
    #[serde(rename = "channel")]
    pub matched_by: MatchChannel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchChannel {
    /// 通道 B：直连整句表。
    Direct,
    /// 通道 A：规则语法（或外部引擎）。
    Grammar,
}

impl Intent {
    pub fn slot(&self, key: &str) -> Option<&str> {
        self.slots.get(key).map(|s| s.as_str())
    }
}

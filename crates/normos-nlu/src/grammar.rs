//! 规则语法 NLU（通道 A 的默认实现）+ 直连整句表（通道 B）。
//!
//! 直连表完整移植旧系统 `klar_executor.DIRECT_COMMANDS` 语义（整句归一化精确匹配，
//! 长词优先；裸「重启」**故意不进表**——防误触发）。

use crate::intents::{Intent, IntentName, MatchChannel, Slots};
use crate::registry::EntityRegistry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// NLU 决定（对应 klar decision.type：execute / clarify / reject；chat 在原型里并入 reject）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NluDecision {
    Execute(Intent),
    /// 需要澄清（旧系统 klar 有澄清多轮；原型明确输出澄清话术，不猜）。
    Clarify {
        speech: String,
    },
    /// 拒绝/没听懂。
    Reject {
        speech: String,
    },
}

/// 归一化：去空白与标点（旧系统 match_direct 口径：去标点空白，不做子串）。
pub fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '，' | '。' | '！' | '？' | '、' | ',' | '.' | '!' | '?' | '：' | ':'
                )
        })
        .collect()
}

/// 直连命令表：短语 → 意图构造器。整句精确匹配（长词优先排序）。
fn direct_table() -> Vec<(&'static str, IntentName, Slots)> {
    fn add(
        rows: &mut Vec<(&'static str, IntentName, Slots)>,
        phrases: &'static [&'static str],
        name: IntentName,
        slots: Slots,
    ) {
        for p in phrases {
            rows.push((p, name, slots.clone()));
        }
    }
    let mut rows: Vec<(&'static str, IntentName, Slots)> = Vec::new();
    // 对话
    add(
        &mut rows,
        &["小克小克", "小克"],
        IntentName::DialogueReply,
        slots([("reply", "我在。")]),
    );
    add(
        &mut rows,
        &["再见"],
        IntentName::DialogueReply,
        slots([("reply", "拜拜。")]),
    );
    // 开门（危险）：旧表含「我回来了开门/我回来了，开门/我回家了开门/开门吧/帮我开门/把门打开/门开一下/开一下门/我回来了」
    add(
        &mut rows,
        &[
            "我回来了开门",
            "我回家了开门",
            "我回来了",
            "我回家了",
            "开门吧",
            "帮我开门",
            "把门打开",
            "门开一下",
            "开一下门",
        ],
        IntentName::ArriveHome,
        Slots::new(),
    );
    // 门禁状态
    add(
        &mut rows,
        &["门禁状态", "门能开吗", "门锁状态"],
        IntentName::QueryDoorStatus,
        Slots::new(),
    );
    // 人脸识别（看看是谁）
    add(
        &mut rows,
        &[
            "打开人脸识别",
            "开始人脸识别",
            "启动人脸识别",
            "识别一下",
            "看看是谁",
            "谁来了",
            "人脸识别",
            "识别画面",
            "看看有人吗",
        ],
        IntentName::WhoIsThere,
        Slots::new(),
    );
    // 录制：mode 语义沿旧板（on→manual）
    add(
        &mut rows,
        &["开始自动录制", "打开自动录制", "自动录制", "有人时再录"],
        IntentName::AutoRecording,
        slots([("mode", "auto")]),
    );
    add(
        &mut rows,
        &["开始录制", "开启录制"],
        IntentName::StartRecording,
        slots([("mode", "manual")]),
    );
    add(
        &mut rows,
        &["停止录制", "关闭录制", "别录了"],
        IntentName::StopRecording,
        slots([("mode", "off")]),
    );
    // 状态 / 时间
    add(
        &mut rows,
        &["现在什么状态", "状态怎么样", "板子还好吗", "系统状态"],
        IntentName::QueryStatus,
        Slots::new(),
    );
    add(
        &mut rows,
        &["现在几点", "几点了", "现在时间", "看下时间"],
        IntentName::QueryTime,
        Slots::new(),
    );
    // 拍照
    add(
        &mut rows,
        &["拍照存档", "拍一张照片", "拍张照片", "存一张照片"],
        IntentName::CapturePhoto,
        Slots::new(),
    );
    // 声纹
    add(
        &mut rows,
        &["启动声纹识别", "开启声纹识别", "打开声纹识别"],
        IntentName::StartVoiceprint,
        Slots::new(),
    );
    // 重启（危险；裸「重启」不进表）
    add(
        &mut rows,
        &[
            "重启系统",
            "重启板子",
            "重启悟净",
            "把板子重启",
            "重启一下板子",
            "帮我重启板子",
            "执行重启",
        ],
        IntentName::RebootSystem,
        Slots::new(),
    );
    add(
        &mut rows,
        &["取消重启", "别重启了", "不重启了"],
        IntentName::CancelReboot,
        Slots::new(),
    );
    // 长词优先排序（旧系统 match_direct 纪律）
    rows.sort_by_key(|(p, _, _)| std::cmp::Reverse(p.chars().count()));
    rows
}

fn slots(pairs: [(&str, &str); 1]) -> Slots {
    let mut m = Slots::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v.to_string());
    }
    m
}

/// 规则语法解析结果。
pub struct RuleNlu {
    registry: EntityRegistry,
    direct: Vec<(&'static str, IntentName, Slots)>,
}

impl RuleNlu {
    pub fn new(registry: EntityRegistry) -> Self {
        Self {
            registry,
            direct: direct_table(),
        }
    }

    pub fn registry(&self) -> &EntityRegistry {
        &self.registry
    }

    /// 通道 B：整句精确匹配（归一化后）。不命中返回 None——**不做子串**。
    pub fn match_direct(&self, text: &str) -> Option<Intent> {
        let norm = normalize(text);
        for (phrase, name, slots) in &self.direct {
            if norm == *phrase {
                return Some(Intent {
                    name: *name,
                    slots: slots.clone(),
                    matched_by: MatchChannel::Direct,
                });
            }
        }
        None
    }

    /// 通道 A：规则语法。打开/关闭 + [区域]的[设备]，支持「把…的…打开/关掉」变体。
    pub fn parse_grammar(&self, text: &str) -> NluDecision {
        let norm = normalize(text);
        // 去唤醒词前缀（「小克，打开…」）
        let norm = strip_wake(&norm);

        // 否定/疑问句不得当执行指令（「不要开始录制」「怎么开始录制」）
        if starts_with_any(
            norm,
            &[
                "不要",
                "别",
                "怎么",
                "为什么",
                "能不能",
                "可以",
                "试试",
                "如果",
            ],
        ) {
            return NluDecision::Clarify {
                speech: "你想要我做什么？可以试着直接说指令。".into(),
            };
        }

        // 变体 1：把X的Y打开/关掉/关闭/开一下
        if let Some(intent) = self.try_pattern_put(norm) {
            return intent;
        }
        // 变体 2：打开/关闭/开一下 X的Y 或 全局Y
        if let Some(intent) = self.try_pattern_verb_first(norm) {
            return intent;
        }

        NluDecision::Reject {
            speech: "我没听懂，可以说「打开客厅的灯」这类指令。".into(),
        }
    }

    fn try_pattern_put(&self, norm: &str) -> Option<NluDecision> {
        let norm = strip_leading_fillers(norm);
        for (suffix, name) in [
            ("打开", IntentName::HassTurnOn),
            ("开一下", IntentName::HassTurnOn),
            ("开启", IntentName::HassTurnOn),
            ("关掉", IntentName::HassTurnOff),
            ("关闭", IntentName::HassTurnOff),
            ("关上", IntentName::HassTurnOff),
        ] {
            if let Some(head) = norm.strip_suffix(suffix)
                && let Some((area_word, entity_word)) = split_de(head)
            {
                return self.resolve(area_word, entity_word, name);
            }
        }
        None
    }

    fn try_pattern_verb_first(&self, norm: &str) -> Option<NluDecision> {
        let verbs: [(&str, IntentName); 6] = [
            ("打开", IntentName::HassTurnOn),
            ("开一下", IntentName::HassTurnOn),
            ("开启", IntentName::HassTurnOn),
            ("关闭", IntentName::HassTurnOff),
            ("关掉", IntentName::HassTurnOff),
            ("关上", IntentName::HassTurnOff),
        ];
        for (verb, name) in verbs {
            if let Some(rest) = norm.strip_prefix(verb) {
                let rest = strip_leading_fillers(rest.trim());
                if let Some((area_word, entity_word)) = split_de(rest) {
                    return self.resolve(area_word, entity_word, name);
                }
                // 无「的」：全局实体
                if let Some(e) = self.registry.find_entity_global(rest) {
                    return Some(NluDecision::Execute(Intent {
                        name,
                        slots: slot_entity(e.entity_id.clone(), e.area_id.clone()),
                        matched_by: MatchChannel::Grammar,
                    }));
                }
            }
        }
        None
    }

    fn resolve(&self, area_word: &str, entity_word: &str, name: IntentName) -> Option<NluDecision> {
        if let Some(area) = self.registry.find_area(area_word) {
            if let Some(e) = self.registry.find_entity_in_area(&area.id, entity_word) {
                return Some(NluDecision::Execute(Intent {
                    name,
                    slots: slot_entity(e.entity_id.clone(), area.id.clone()),
                    matched_by: MatchChannel::Grammar,
                }));
            }
            // 区域对但实体词不认识 → 澄清
            return Some(NluDecision::Clarify {
                speech: format!("{}里没有叫「{}」的设备。", area.name, entity_word),
            });
        }
        None
    }

    /// 完整解析入口：先通道 B 直连（整句），未命中再剥唤醒词重试直连，
    /// 最后通道 A 语法。与旧 executor 一致：带唤醒词的整句也能进直连表。
    pub fn parse(&self, text: &str) -> NluDecision {
        if let Some(intent) = self.match_direct(text) {
            return NluDecision::Execute(intent);
        }
        let norm = normalize(text);
        let stripped = strip_wake(&norm).to_string();
        if !stripped.is_empty()
            && stripped != norm
            && let Some(intent) = self.match_direct(&stripped)
        {
            return NluDecision::Execute(intent);
        }
        self.parse_grammar(text)
    }
}

fn strip_wake(norm: &str) -> &str {
    for w in ["小克小克", "小克"] {
        if let Some(rest) = norm.strip_prefix(w) {
            return rest;
        }
    }
    norm
}

fn starts_with_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// 剥离前导虚词（把/帮我/请/麻烦），循环直至无变化（「帮我把…」）。
fn strip_leading_fillers(s: &str) -> &str {
    let mut t = s;
    loop {
        let before = t;
        for f in ["帮我", "请", "麻烦", "把"] {
            if let Some(rest) = t.strip_prefix(f) {
                t = rest;
            }
        }
        if t == before {
            return t;
        }
    }
}

/// 把「X的Y」拆开；没有「的」返回 None。
fn split_de(s: &str) -> Option<(&str, &str)> {
    let idx = s.find('的')?;
    let (a, b) = s.split_at(idx);
    let b = &b['的'.len_utf8()..];
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a, b))
}

fn slot_entity(entity_id: String, area: String) -> Slots {
    let mut m = Slots::new();
    m.insert("entity_id".into(), entity_id);
    m.insert("area".into(), area);
    m
}

/// 外部 klar 引擎适配器契约（decision.type / candidates[].plan.steps[].intent 的归一化版）。
/// 真实 klar 部署时由 HTTP 适配器实现；本仓库原型默认用 RuleNlu。
pub trait ExternalNlu: Send {
    fn name(&self) -> &str;
    fn parse(&self, text: &str, language: &str) -> NluDecision;
}

/// klarResult 归一化形态（旧 control/adapters/klar.py parse_v2 的 Rust 版契约）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KlarV2Response {
    pub decision_type: String, // execute | clarify | confirm | chat | reject
    pub speech: Option<String>,
    pub intents: Vec<KlarIntentStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KlarIntentStep {
    pub name: String,
    pub slots: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nlu() -> RuleNlu {
        RuleNlu::new(EntityRegistry::default_home())
    }

    #[test]
    fn direct_table_exact_match_only() {
        let n = nlu();
        // 裸指令整句命中
        let hit = n.match_direct("开始录制").unwrap();
        assert_eq!(hit.name, IntentName::StartRecording);
        assert_eq!(hit.slot("mode"), Some("manual"));
        // 带唤醒词的整句由 parse() 负责剥词后命中
        let hit2 = n.parse("小克，开始录制");
        match hit2 {
            NluDecision::Execute(i) => {
                assert_eq!(i.name, IntentName::StartRecording);
                assert_eq!(i.matched_by, MatchChannel::Direct);
            }
            other => panic!("应为 Execute，实际 {other:?}"),
        }
        // 「不要开始录制」不得命中（只整句匹配）
        assert!(n.match_direct("不要开始录制").is_none());
        assert!(n.match_direct("怎么开始录制").is_none());
        // 裸「重启」不进表（防误触发）
        assert!(n.match_direct("重启").is_none());
        // 「帮我重启板子」进表
        assert_eq!(
            n.match_direct("帮我重启板子").unwrap().name,
            IntentName::RebootSystem
        );
    }

    #[test]
    fn grammar_turn_on_with_area() {
        let n = nlu();
        let d = n.parse("小克，打开客厅的灯");
        match d {
            NluDecision::Execute(i) => {
                assert_eq!(i.name, IntentName::HassTurnOn);
                assert_eq!(i.slot("entity_id"), Some("light.keting"));
                assert_eq!(i.matched_by, MatchChannel::Grammar);
            }
            other => panic!("应为 Execute，实际 {other:?}"),
        }
    }

    #[test]
    fn grammar_variants() {
        let n = nlu();
        for t in ["把卧室的灯关掉", "关闭卧室的灯", "帮我把卧室的灯关闭"] {
            match n.parse(t) {
                NluDecision::Execute(i) => {
                    assert_eq!(i.name, IntentName::HassTurnOff, " utterance {t}");
                    assert_eq!(i.slot("entity_id"), Some("light.woshi"));
                }
                other => panic!("{t} 应为 Execute，实际 {other:?}"),
            }
        }
    }

    #[test]
    fn negation_goes_to_clarify() {
        let n = nlu();
        match n.parse("不要打开客厅的灯") {
            NluDecision::Clarify { .. } => {}
            other => panic!("否定句应澄清，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_area_entity_gives_clarify_not_guess() {
        let n = nlu();
        match n.parse("打开厨房的灯") {
            // 厨房不在注册表 → Reject（区域词未匹配，语法不认）
            NluDecision::Reject { .. } | NluDecision::Clarify { .. } => {}
            other => panic!("未知区域不得伪造执行，实际 {other:?}"),
        }
    }

    #[test]
    fn normalize_strips_punct() {
        assert_eq!(normalize("我回来了，开门！"), "我回来了开门");
    }

    #[test]
    fn dialogue_reply_canned() {
        let n = nlu();
        match n.parse("小克小克") {
            NluDecision::Execute(i) => {
                assert_eq!(i.name, IntentName::DialogueReply);
                assert_eq!(i.slot("reply"), Some("我在。"));
            }
            other => panic!("实际 {other:?}"),
        }
    }
}

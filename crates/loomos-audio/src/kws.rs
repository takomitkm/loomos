//! KWS（唤醒词）档位。
//!
//! 旧系统：sherpa-onnx KWS「小克」（keywords_threshold=0.25，命中才往下走）；
//! 原型默认档 `TextGateKws` 在 ASR 文本上做唤醒词门控——语义等价（没叫名字不理人），
//! 真引擎接 sherpa KWS 适配器（同 trait）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KwsHit {
    pub keyword: String,
    /// 命中分数（真实引擎为模型分数；文本门控为精确 1.0）。
    pub score: f64,
}

/// KWS 档位 trait。`detect_text` 是 echo 档接口；真引擎档实现 `detect_audio`。
pub trait KwsBackend: Send {
    fn name(&self) -> &str;
    fn detect_text(&self, transcript: &str) -> Option<KwsHit>;
}

/// 文本门控：转写文本必须包含唤醒词（前缀或包含）。
#[derive(Debug, Clone)]
pub struct TextGateKws {
    pub keywords: Vec<String>,
}

impl TextGateKws {
    pub fn new(keywords: &[&str]) -> Self {
        Self {
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 默认唤醒词「小克」（旧系统 KWS 词表主词）。
    pub fn xiaoke() -> Self {
        Self::new(&["小克", "小克小克"])
    }
}

impl KwsBackend for TextGateKws {
    fn name(&self) -> &str {
        "text-gate"
    }

    fn detect_text(&self, transcript: &str) -> Option<KwsHit> {
        let cleaned = transcript.trim();
        for kw in &self.keywords {
            if cleaned.contains(kw.as_str()) {
                return Some(KwsHit {
                    keyword: kw.clone(),
                    score: 1.0,
                });
            }
        }
        None
    }
}

/// Passthrough 档：全部放行（链路自检用）。
pub struct PassthroughKws;

impl KwsBackend for PassthroughKws {
    fn name(&self) -> &str {
        "passthrough"
    }
    fn detect_text(&self, _transcript: &str) -> Option<KwsHit> {
        Some(KwsHit {
            keyword: String::new(),
            score: 1.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_gate_blocks_without_hotword() {
        let kws = TextGateKws::xiaoke();
        assert!(kws.detect_text("小克，打开客厅的灯").is_some());
        assert!(kws.detect_text("打开客厅的灯").is_none());
    }
}

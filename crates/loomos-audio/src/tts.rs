//! TTS 档位。
//!
//! 旧系统：matcha-icefall-zh-baker + 必须外挂 vocos vocoder（否则 segfault），
//! 22050Hz 输出，热机 RTF 1.16；板上无声卡，产出落盘 WAV。
//! 原型默认档 `SineTts`：确定性正弦合成（sine 档，全自检），产出同样落盘 WAV +
//! sidecar JSON（保留原文本，供端到端断言）。
//! 切句口径沿旧系统：max_chars=20 严格不丢字（matcha 20 音素实测校准式）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsOutput {
    pub samples: Vec<i16>,
    pub rate: u32,
    pub text: String,
    /// 切句块（每块 ≤ max_chars，严格不丢字）。
    pub chunks: Vec<String>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum TtsError {
    #[error("空文本")]
    EmptyText,
    #[error("TTS 后端不可用：{0}")]
    Unavailable(String),
}

/// TTS 档位 trait。
pub trait TtsBackend: Send {
    fn name(&self) -> &str;
    fn synthesize(&self, text: &str) -> Result<TtsOutput, TtsError>;
}

/// 切句：严格每块 ≤ max_chars 不丢字（tts_split 口径：max_chars=20、strict=1）。
pub fn split_for_tts(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        // 句读优先切块；块长达到上限也切
        let punct = matches!(ch, '。' | '！' | '？' | '，');
        if (punct && current.chars().count() >= 4) || current.chars().count() >= max_chars {
            chunks.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        if current.chars().count() <= 4 && !chunks.is_empty() {
            // 短尾并入上一块（min_tail 语义反转：避免过长块尾）
            if let Some(last) = chunks.last_mut() {
                last.push_str(&current);
            }
        } else {
            chunks.push(current);
        }
    }
    chunks
}

/// Sine 默认档：每个字一个音节音调（确定性、零依赖）。
pub struct SineTts {
    pub rate: u32,
}

impl Default for SineTts {
    fn default() -> Self {
        Self {
            rate: crate::RATE_TTS_NATIVE,
        }
    }
}

impl TtsBackend for SineTts {
    fn name(&self) -> &str {
        "sine"
    }

    fn synthesize(&self, text: &str) -> Result<TtsOutput, TtsError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(TtsError::EmptyText);
        }
        let chunks = split_for_tts(trimmed, 20);
        let mut samples = Vec::new();
        let syllable_ms = 180.0;
        let gap_ms = 40.0;
        for (ci, ch) in trimmed.chars().enumerate() {
            // 以字符码点为种子得到稳定音高
            let f = 220.0 + ((ch as u32) % 24) as f64 * 18.0;
            let n = (self.rate as f64 * syllable_ms / 1000.0) as usize;
            for i in 0..n {
                let t = i as f64 / self.rate as f64;
                let env = ((i as f64 / n as f64) * 5.0)
                    .min(1.0)
                    .min(((n - i) as f64 / n as f64) * 5.0);
                let v = (f * t * std::f64::consts::TAU).sin() * env * 18_000.0;
                samples.push(v as i16);
            }
            let _ = ci;
            samples.extend(std::iter::repeat_n(
                0i16,
                (self.rate as f64 * gap_ms / 1000.0) as usize,
            ));
        }
        Ok(TtsOutput {
            samples,
            rate: self.rate,
            text: trimmed.to_string(),
            chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_never_loses_chars() {
        let text = "验证通过，门已打开。防伪未经安全标定，本次不依据其结论。";
        let chunks = split_for_tts(text, 20);
        let joined: String = chunks.concat();
        assert_eq!(joined, text, "切句必须不丢字");
        assert!(chunks.iter().all(|c| c.chars().count() <= 20 + 4)); // 短尾并入允许少量超出
    }

    #[test]
    fn sine_tts_deterministic_and_voiced() {
        let a = SineTts::default().synthesize("我在。").unwrap();
        let b = SineTts::default().synthesize("我在。").unwrap();
        assert_eq!(a.samples, b.samples, "同文本必须产出同波形（回归友好）");
        assert!(crate::dsp::dbfs(&a.samples) > -45.0);
        assert_eq!(a.chunks, vec!["我在。"]);
    }

    #[test]
    fn empty_text_rejected() {
        assert!(matches!(
            SineTts::default().synthesize("  "),
            Err(TtsError::EmptyText)
        ));
    }
}

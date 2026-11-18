//! ASR 档位。
//!
//! 旧系统：sherpa-onnx streaming zipformer（14M int8，RTF 0.40，16k/80 维 fbank/greedy）。
//! 原型默认档：
//! - `SidecarAsr`：夹具 wav 内容哈希 → 夹具元数据里的标准文本（确定性、可回归）；
//! - `EchoAsr`：echo 档（返回占位文本，链路自检用）。
//!   真引擎接 sherpa ASR 适配器（同 trait）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrResult {
    pub text: String,
    pub rtf: Option<f64>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum AsrError {
    #[error("无法识别（无对应转写）")]
    NoTranscript,
    #[error("ASR 后端不可用：{0}")]
    Unavailable(String),
}

/// ASR 档位 trait。
pub trait AsrBackend: Send {
    fn name(&self) -> &str;
    fn transcribe(&self, samples: &[i16], rate: u32) -> Result<AsrResult, AsrError>;
}

fn content_key(samples: &[i16]) -> String {
    // FNV-1a 64：确定性、无依赖
    let mut h: u64 = 0xcbf29ce484222325;
    for s in samples {
        let v = (*s as u16).to_le_bytes();
        for b in v {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

/// 夹具旁车档：wav 内容哈希 → 文本（gen-fixtures 生成的映射）。
pub struct SidecarAsr {
    map: BTreeMap<String, String>,
}

impl SidecarAsr {
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        Self {
            map: pairs.into_iter().collect(),
        }
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let m: BTreeMap<String, String> = serde_json::from_str(json)?;
        Ok(Self { map: m })
    }

    pub fn register(&mut self, samples: &[i16], text: &str) {
        self.map.insert(content_key(samples), text.to_string());
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl AsrBackend for SidecarAsr {
    fn name(&self) -> &str {
        "sidecar-mock"
    }

    fn transcribe(&self, samples: &[i16], _rate: u32) -> Result<AsrResult, AsrError> {
        let key = content_key(samples);
        self.map
            .get(&key)
            .map(|t| AsrResult {
                text: t.clone(),
                rtf: None,
            })
            .ok_or(AsrError::NoTranscript)
    }
}

/// Echo 档：占位转写（全自检链路用）。
pub struct EchoAsr;

impl AsrBackend for EchoAsr {
    fn name(&self) -> &str {
        "echo"
    }
    fn transcribe(&self, _samples: &[i16], _rate: u32) -> Result<AsrResult, AsrError> {
        Ok(AsrResult {
            text: "（echo 占位转写）".into(),
            rtf: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_maps_content_hash_to_text() {
        let mut asr = SidecarAsr::from_pairs(vec![]);
        let a = vec![1i16, 2, 3];
        asr.register(&a, "打开客厅的灯");
        assert_eq!(asr.transcribe(&a, 16_000).unwrap().text, "打开客厅的灯");
        assert!(matches!(
            asr.transcribe(&[9, 9], 16_000),
            Err(AsrError::NoTranscript)
        ));
        // 内容相同 → 相同 key（确定性回归）
        asr.register(&a, "changed");
        assert_eq!(asr.transcribe(&a, 16_000).unwrap().text, "changed");
    }
}

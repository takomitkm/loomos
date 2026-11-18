//! 声纹 mock：录入/校验流程与旧 boards/voiceprint.py 同构
//! （store 格式 `{"version":1,"dim":N,"speakers":{name:[floats]}}`，余弦 ≥ 阈值判定）。
//!
//! **诚实声明**：嵌入为确定性 mock 向量（非 campplus 真模型）；
//! 真引擎接 sherpa SpeakerEmbeddingExtractor 适配器（接口同构）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_THRESHOLD: f64 = 0.5;
/// mock 嵌入维度（真 campplus-zh 为 192 维；mock 用 64 维示意）。
pub const MOCK_DIM: usize = 64;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum VoiceprintError {
    #[error("音频过短（至少 {0} ms）")]
    TooShort(u64),
    #[error("声纹库为空")]
    EmptyLibrary,
    #[error("维度不符：期望 {0}，实际 {1}")]
    DimMismatch(usize, usize),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VoiceprintStore {
    pub version: u32,
    pub dim: usize,
    pub speakers: BTreeMap64,
}

type BTreeMap64 = std::collections::BTreeMap<String, Vec<f64>>;

/// 校验结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub name: Option<String>,
    pub score: f64,
    pub threshold: f64,
    /// 各人分数（调试用）。
    pub scores: Vec<(String, f64)>,
}

pub struct MockVoiceprint {
    store: VoiceprintStore,
}

impl MockVoiceprint {
    pub fn new() -> Self {
        Self {
            store: VoiceprintStore {
                version: 1,
                dim: MOCK_DIM,
                speakers: BTreeMap64::new(),
            },
        }
    }

    pub fn speakers(&self) -> usize {
        self.store.speakers.len()
    }

    /// 录入：wav 样本 → mock 嵌入 → 入库。
    pub fn enroll(
        &mut self,
        name: &str,
        samples: &[i16],
        rate: u32,
    ) -> Result<(), VoiceprintError> {
        let min_ms = 600;
        let ms = samples.len() as u64 * 1000 / rate as u64;
        if ms < min_ms {
            return Err(VoiceprintError::TooShort(min_ms));
        }
        let emb = mock_embed(samples);
        self.store.speakers.insert(name.to_string(), emb);
        tracing::info!(name, dim = MOCK_DIM, "voiceprint enrolled");
        Ok(())
    }

    /// 校验：余弦相似度最高者 ≥ 阈值才认人。
    pub fn verify(
        &self,
        samples: &[i16],
        rate: u32,
        threshold: f64,
    ) -> Result<VerifyResult, VoiceprintError> {
        if self.store.speakers.is_empty() {
            return Err(VoiceprintError::EmptyLibrary);
        }
        let ms = samples.len() as u64 * 1000 / rate as u64;
        if ms < 600 {
            return Err(VoiceprintError::TooShort(600));
        }
        let emb = mock_embed(samples);
        let mut scores: Vec<(String, f64)> = self
            .store
            .speakers
            .iter()
            .map(|(name, v)| (name.clone(), cosine(&emb, v)))
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let (best_name, best_score) = scores
            .first()
            .map(|(n, s)| (Some(n.clone()), *s))
            .unwrap_or((None, f64::MIN));
        Ok(VerifyResult {
            name: if best_score >= threshold {
                best_name
            } else {
                None
            },
            score: best_score,
            threshold,
            scores,
        })
    }

    pub fn export_json(&self) -> String {
        serde_json::to_string_pretty(&self.store).unwrap_or_default()
    }

    pub fn load_json(json: &str) -> Result<Self, serde_json::Error> {
        Ok(Self {
            store: serde_json::from_str(json)?,
        })
    }
}

impl Default for MockVoiceprint {
    fn default() -> Self {
        Self::new()
    }
}

/// mock 嵌入：64 频段对数滤波器组幅值（Goertzel 单点 DFT），频率敏感、确定性。
/// 同音频 → 同向量；不同频谱内容 → 明显不同向量。
/// **真实引擎替换点**：campplus-zh 192 维嵌入（接口同构，换掉本函数即可）。
fn mock_embed(samples: &[i16]) -> Vec<f64> {
    let n = samples.len();
    if n == 0 {
        return vec![0.0; MOCK_DIM];
    }
    // 对数均匀分布 64 频段：100 Hz → 8000 Hz（16k 采样率的 Nyquist 内）
    let f_min = 100.0f64;
    let f_max = 8000.0f64;
    let mut emb = Vec::with_capacity(MOCK_DIM);
    for i in 0..MOCK_DIM {
        let frac = i as f64 / (MOCK_DIM - 1) as f64;
        let f = f_min * (f_max / f_min).powf(frac);
        // Goertzel：单频点 DFT 幅值
        let k = 2.0 * std::f64::consts::PI * f / 16_000.0;
        let coeff = 2.0 * k.cos();
        let mut s1 = 0.0f64;
        let mut s2 = 0.0f64;
        for &x in samples {
            let xf = x as f64;
            let s0 = xf + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        emb.push(power.sqrt() / n as f64);
    }
    // 单位化
    let norm: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
    emb.into_iter().map(|x| x / norm).collect()
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na * nb < 1e-12 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use normos_audio_stub::*;

    #[test]
    fn enroll_verify_same_speaker_passes() {
        let mut vp = MockVoiceprint::new();
        let samples = tone_vec(1_000_000); // 1 s @ 16k * 62.5 → use len
        vp.enroll("主人", &samples, 16_000).unwrap();
        let r = vp.verify(&samples, 16_000, DEFAULT_THRESHOLD).unwrap();
        assert_eq!(r.name.as_deref(), Some("主人"));
        assert!((r.score - 1.0).abs() < 1e-9, "同人同音频余弦应为 1.0");
    }

    #[test]
    fn different_audio_below_threshold_fails() {
        let mut vp = MockVoiceprint::new();
        let a = tone_vec(1_000_000);
        vp.enroll("主人", &a, 16_000).unwrap();
        let b: Vec<i16> = (0..16_000)
            .map(|i| if i % 2 == 0 { 20000 } else { -20000 })
            .collect();
        let r = vp.verify(&b, 16_000, DEFAULT_THRESHOLD).unwrap();
        assert!(
            r.name.is_none(),
            "异音频不应认人（mock 特征差异下）score={}",
            r.score
        );
    }

    #[test]
    fn too_short_rejected() {
        let mut vp = MockVoiceprint::new();
        assert!(matches!(
            vp.enroll("x", &[1i16; 100], 16_000),
            Err(VoiceprintError::TooShort(_))
        ));
    }

    // 轻量音调向量生成（避免依赖 normos-audio 造成环）。
    mod normos_audio_stub {
        pub fn tone_vec(fake_len: usize) -> Vec<i16> {
            let len = 16_000; // 1 s @16k
            let _ = fake_len;
            (0..len)
                .map(|i| {
                    ((440.0 * (i as f64 / 16_000.0) * std::f64::consts::TAU).sin() * 20000.0) as i16
                })
                .collect()
        }
    }
}

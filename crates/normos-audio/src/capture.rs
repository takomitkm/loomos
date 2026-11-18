//! 音频采集抽象：AudioChunk / AudioSource trait / 夹具档实现。
//!
//! 板上没有麦克风（旧系统事实），音频来自 HTTP POST / WAV 回放 / 未来硬件；
//! 所以采集层一开始就是「源可插拔」的。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioChunk {
    pub samples: Vec<i16>,
    pub rate: u32,
    /// 源标识（fixture 名 / 设备名）。
    pub source: String,
}

#[derive(Error, Debug, Clone)]
pub enum AudioError {
    #[error("源已结束")]
    Ended,
    #[error("音频 IO 错误：{0}")]
    Io(String),
}

/// 音频源：一次 pull 一个 chunk。
pub trait AudioSource: Send {
    fn next_chunk(&mut self, max_samples: usize) -> Result<AudioChunk, AudioError>;
}

/// WAV 文件回放源（夹具/HTTP 上传内容都可包装）。
pub struct WavFileSource {
    chunk: Option<AudioChunk>,
    pos: usize,
    name: String,
}

impl WavFileSource {
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<Self, AudioError> {
        let (samples, rate) =
            crate::wav::read_wav_mono_pcm16(bytes).map_err(|e| AudioError::Io(e.to_string()))?;
        Ok(Self {
            chunk: Some(AudioChunk {
                samples,
                rate,
                source: name.to_string(),
            }),
            pos: 0,
            name: name.to_string(),
        })
    }

    pub fn from_chunk(chunk: AudioChunk) -> Self {
        let name = chunk.source.clone();
        Self {
            chunk: Some(chunk),
            pos: 0,
            name,
        }
    }
}

impl AudioSource for WavFileSource {
    fn next_chunk(&mut self, max_samples: usize) -> Result<AudioChunk, AudioError> {
        let stored = self.chunk.as_ref().ok_or(AudioError::Ended)?;
        if self.pos >= stored.samples.len() {
            return Err(AudioError::Ended);
        }
        let end = (self.pos + max_samples).min(stored.samples.len());
        let out = AudioChunk {
            samples: stored.samples[self.pos..end].to_vec(),
            rate: stored.rate,
            source: self.name.clone(),
        };
        self.pos = end;
        Ok(out)
    }
}

/// 确定性音调源：夹具生成用（纯 Rust，无模型文件）。
/// 生成「伪语音」：按音节节奏的音调爆发 + 静音间隔，能量门/段切器可识别。
pub struct ToneSource {
    syllables: Vec<f64>, // 每个音节频率
    rate: u32,
    pos: usize,
    name: String,
}

impl ToneSource {
    pub fn new(name: &str, syllables: &[f64], rate: u32) -> Self {
        Self {
            syllables: syllables.to_vec(),
            rate,
            pos: 0,
            name: name.to_string(),
        }
    }

    /// 生成完整波形（每音节 220ms 有声 + 80ms 静音，首尾各 200ms 静音）。
    pub fn to_full_chunk(&self) -> AudioChunk {
        let mut samples = Vec::new();
        let lead = (self.rate as f64 * 0.2) as usize;
        samples.extend(std::iter::repeat_n(0i16, lead));
        for (si, f) in self.syllables.iter().enumerate() {
            let voiced = (self.rate as f64 * 0.22) as usize;
            for i in 0..voiced {
                let t = i as f64 / self.rate as f64;
                // 幅度包络避免咔哒声
                let env = ((i as f64 / voiced as f64) * 4.0)
                    .min(1.0)
                    .min(((voiced - i) as f64 / voiced as f64) * 4.0);
                let v = (f * t * std::f64::consts::TAU).sin() * env * 20_000.0;
                samples.push(v as i16);
            }
            let gap = (self.rate as f64 * 0.08) as usize;
            let last = si == self.syllables.len() - 1;
            if !last {
                samples.extend(std::iter::repeat_n(0i16, gap));
            }
        }
        samples.extend(std::iter::repeat_n(0i16, lead * 2));
        AudioChunk {
            samples,
            rate: self.rate,
            source: self.name.clone(),
        }
    }
}

impl AudioSource for ToneSource {
    fn next_chunk(&mut self, _max_samples: usize) -> Result<AudioChunk, AudioError> {
        if self.pos > 0 {
            return Err(AudioError::Ended);
        }
        let full = self.to_full_chunk();
        self.pos = full.samples.len();
        Ok(full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_source_produces_voiced_and_silence() {
        let src = ToneSource::new("t1", &[440.0, 550.0, 660.0], 48_000);
        let chunk = src.to_full_chunk();
        assert_eq!(chunk.rate, 48_000);
        let d = crate::dsp::dbfs(&chunk.samples);
        assert!(d > -45.0, "含语音段，整体 dBFS 应高于静音门");
        // 首 200ms 为静音
        let lead_len = 9600;
        let lead = crate::dsp::dbfs(&chunk.samples[..lead_len]);
        assert!(lead < -45.0, "开头应为静音，实际 {lead}");
    }
}

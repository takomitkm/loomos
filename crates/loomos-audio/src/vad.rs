//! VAD 与断句（档位：energy 默认档；silero/sherpa 为真实引擎档位，同 trait）。
//!
//! 参数沿旧系统实测口径：
//! - 能量门 silence_dbfs = −45 dBFS，min_voiced_ms = 150（lang/config rms 档）；
//! - 断句 silence_ms = 700、min_speech_ms = 300、max_s = 20、帧 100 ms
//!   （simplest-chain 实测口径：start_rms=0.012 ≈ −38 dBFS 量级）。

use serde::{Deserialize, Serialize};

/// 一段切出的语音。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub samples: Vec<i16>,
    pub rate: u32,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// VAD 档位 trait：判断帧是否有声。
pub trait VadBackend: Send {
    fn name(&self) -> &str;
    fn is_voiced(&self, frame: &[i16]) -> bool;
}

/// 能量门默认档：dBFS 阈值。
pub struct EnergyVad {
    pub silence_dbfs: f64,
}

impl EnergyVad {
    pub fn new(silence_dbfs: f64) -> Self {
        Self { silence_dbfs }
    }
}

impl Default for EnergyVad {
    /// −45 dBFS（旧系统 rms 档 silence_dbfs 默认值）。
    fn default() -> Self {
        Self::new(-45.0)
    }
}

impl VadBackend for EnergyVad {
    fn name(&self) -> &str {
        "energy"
    }

    fn is_voiced(&self, frame: &[i16]) -> bool {
        crate::dsp::dbfs(frame) > self.silence_dbfs
    }
}

/// Passthrough 档：永远有声（echo 全自检链用）。
pub struct PassthroughVad;

impl VadBackend for PassthroughVad {
    fn name(&self) -> &str {
        "passthrough"
    }
    fn is_voiced(&self, _frame: &[i16]) -> bool {
        true
    }
}

/// 断句器：VAD 档 + 节奏参数（100ms 帧、700ms 静音切段、最短 300ms 有声）。
pub struct SpeechSegmenter {
    pub vad: Box<dyn VadBackend>,
    pub frame_ms: u64,
    pub silence_ms: u64,
    pub min_speech_ms: u64,
    pub max_ms: u64,
}

impl Default for SpeechSegmenter {
    fn default() -> Self {
        Self {
            vad: Box::new(EnergyVad::default()),
            frame_ms: 100,
            silence_ms: 700,
            min_speech_ms: 300,
            max_ms: 20_000,
        }
    }
}

impl SpeechSegmenter {
    /// 把一段音频切成语音段（离线一次性处理，足够原型与夹具回放）。
    pub fn segment(&self, samples: &[i16], rate: u32) -> Vec<Segment> {
        let frame_len = (rate as u64 * self.frame_ms / 1000) as usize;
        let mut segments = Vec::new();
        let mut voiced_buf: Vec<i16> = Vec::new();
        let mut silent_run_ms = 0u64;
        let mut voiced_start_ms = 0u64;
        let mut pos = 0usize;

        while pos < samples.len() {
            let end = (pos + frame_len).min(samples.len());
            let frame = &samples[pos..end];
            let t_ms = (pos as u64) * 1000 / rate as u64;
            if self.vad.is_voiced(frame) {
                if voiced_buf.is_empty() {
                    voiced_start_ms = t_ms;
                }
                voiced_buf.extend_from_slice(frame);
                silent_run_ms = 0;
            } else if !voiced_buf.is_empty() {
                silent_run_ms += self.frame_ms;
                voiced_buf.extend_from_slice(frame);
                let voiced_ms = (voiced_buf.len() as u64 * 1000 / rate as u64) - silent_run_ms;
                if silent_run_ms >= self.silence_ms
                    || voiced_buf.len() as u64 * 1000 / rate as u64 >= self.max_ms
                {
                    if voiced_ms >= self.min_speech_ms {
                        segments.push(Segment {
                            samples: voiced_buf.clone(),
                            rate,
                            start_ms: voiced_start_ms,
                            end_ms: t_ms,
                        });
                    }
                    voiced_buf.clear();
                    silent_run_ms = 0;
                }
            }
            pos = end;
        }
        // 收尾
        if !voiced_buf.is_empty() {
            let voiced_ms =
                (voiced_buf.len() as u64 * 1000 / rate as u64).saturating_sub(silent_run_ms);
            if voiced_ms >= self.min_speech_ms {
                let end_t = (samples.len() as u64) * 1000 / rate as u64;
                segments.push(Segment {
                    samples: voiced_buf,
                    rate,
                    start_ms: voiced_start_ms,
                    end_ms: end_t,
                });
            }
        }
        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::ToneSource;

    #[test]
    fn energy_vad_classifies_frames() {
        let vad = EnergyVad::default();
        let loud = vec![10_000i16; 1600];
        let quiet = vec![10i16; 1600];
        assert!(vad.is_voiced(&loud));
        assert!(!vad.is_voiced(&quiet));
    }

    #[test]
    fn segmenter_cuts_syllable_burst() {
        let src = ToneSource::new("t", &[440.0, 550.0, 660.0], 48_000);
        let chunk = src.to_full_chunk();
        let seg = SpeechSegmenter {
            silence_ms: 400,
            ..Default::default()
        };
        let segments = seg.segment(&chunk.samples, chunk.rate);
        // 一串连续音节应切成 1 段（间隔 80ms < 400ms 静音阈值）
        assert_eq!(segments.len(), 1, "应切成 1 段，实际 {}", segments.len());
        assert!(segments[0].samples.len() * 1000 / 48_000 > 300);
    }

    #[test]
    fn segmenter_ignores_silence_only() {
        let chunk = AudioChunkSilence::get();
        let seg = SpeechSegmenter::default();
        assert!(seg.segment(&chunk, 48_000).is_empty());
    }

    struct AudioChunkSilence;
    impl AudioChunkSilence {
        fn get() -> Vec<i16> {
            vec![0i16; 48_000]
        }
    }
}

//! loomos 音频数据面。
//!
//! 档位纪律（沿旧系统 lang/config 的 chain 设计）：
//! - **默认档全自检（echo/sine/mock）**：空机可跑全链，不依赖任何模型文件；
//! - 真实引擎（sherpa-onnx 的 KWS/VAD/ASR/TTS）作为适配器档位接入，同一 trait；
//! - 重采样选型沿用旧板实测（docs/dev/EVIDENCE.md）：
//!   升采样 linear（比 polyphase 快 107 倍、频谱相关 0.9997）；
//!   降采样 decimate3-mavg3（与 31 抽 sinc 识别结果一致，快 11–14 倍）。

pub mod asr;
pub mod capture;
pub mod dsp;
pub mod kws;
pub mod tts;
pub mod vad;
pub mod wav;

pub use asr::{AsrBackend, AsrError, EchoAsr, SidecarAsr};
pub use capture::{AudioChunk, AudioSource, ToneSource, WavFileSource};
pub use kws::{KwsBackend, PassthroughKws, TextGateKws};
pub use tts::{SineTts, TtsBackend, TtsOutput};
pub use vad::{EnergyVad, Segment, SpeechSegmenter, VadBackend};
pub use wav::{WavError, read_wav_mono_pcm16, write_wav_mono_pcm16};

/// 全链统一采样率（板上链路 16k 推理 / 48k 输出）。
pub const RATE_INFERENCE: u32 = 16_000;
pub const RATE_OUTPUT: u32 = 48_000;
pub const RATE_TTS_NATIVE: u32 = 22_050;

//! ASR 适配器：流式 zipformer（板同款 14M int8）经 sherpa Python API 整句解码。

use crate::{SherpaConfig, SherpaError, parse_json_line};
use loomos_audio::AsrResult;

/// sherpa ASR 适配器（实现 `loomos_audio::AsrBackend`）。
#[derive(Debug, Clone)]
pub struct SherpaAsr {
    pub config: SherpaConfig,
}

impl SherpaAsr {
    pub fn new(config: SherpaConfig) -> Self {
        Self { config }
    }
}

impl loomos_audio::AsrBackend for SherpaAsr {
    fn name(&self) -> &str {
        "sherpa-zipformer-small-zh-en-int8"
    }

    fn transcribe(&self, samples: &[i16], rate: u32) -> Result<AsrResult, loomos_audio::AsrError> {
        self.config
            .available_asr()
            .map_err(|e| loomos_audio::AsrError::Unavailable(e.to_string()))?;
        // 写临时 wav（驱动读 PCM16；非 16k 由驱动线性重采样——与 loomos-audio dsp 同口径）
        let tmp = std::env::temp_dir().join(format!(
            "loomos-sherpa-asr-{}-{}.wav",
            std::process::id(),
            samples.len()
        ));
        crate::write_wav_pcm16(&tmp, samples, rate)
            .map_err(|e| loomos_audio::AsrError::Unavailable(e.to_string()))?;
        let model_dir = self.config.asr_model_dir.display().to_string();
        let result = self
            .config
            .run_driver(&[
                "asr",
                "--model-dir",
                &model_dir,
                "--wav",
                &tmp.display().to_string(),
                "--num-threads",
                &self.config.num_threads.to_string(),
            ])
            .map_err(|e| match e {
                SherpaError::DriverFailed { stderr } => loomos_audio::AsrError::Unavailable(stderr),
                other => loomos_audio::AsrError::Unavailable(other.to_string()),
            });
        let _ = std::fs::remove_file(&tmp);
        let json = result.map_err(|e| loomos_audio::AsrError::Unavailable(e.to_string()))?;
        let v = parse_json_line(&json)
            .map_err(|e| loomos_audio::AsrError::Unavailable(e.to_string()))?;
        let text = v["text"]
            .as_str()
            .ok_or_else(|| loomos_audio::AsrError::Unavailable("无 text 字段".into()))?
            .to_string();
        Ok(AsrResult { text, rtf: None })
    }
}

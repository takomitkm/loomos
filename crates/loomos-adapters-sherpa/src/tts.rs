//! TTS 适配器：vits-melo-tts-zh_en 经 sherpa Python API 合成 → 读回 wav 样本。

use crate::{SherpaConfig, SherpaError, parse_json_line};
use loomos_audio::TtsOutput;

/// sherpa TTS 适配器（实现 `loomos_audio::TtsBackend`）。
#[derive(Debug, Clone)]
pub struct SherpaTts {
    pub config: SherpaConfig,
}

impl SherpaTts {
    pub fn new(config: SherpaConfig) -> Self {
        Self { config }
    }
}

impl loomos_audio::TtsBackend for SherpaTts {
    fn name(&self) -> &str {
        "sherpa-vits-melo-zh-en"
    }

    fn synthesize(&self, text: &str) -> Result<TtsOutput, loomos_audio::TtsError> {
        self.config
            .available_tts()
            .map_err(|e| loomos_audio::TtsError::Unavailable(e.to_string()))?;
        let tmp = std::env::temp_dir().join(format!(
            "loomos-sherpa-tts-{}-{}.wav",
            std::process::id(),
            text.len()
        ));
        let model_dir = self.config.tts_model_dir.display().to_string();
        let result = self
            .config
            .run_driver(&[
                "tts",
                "--model-dir",
                &model_dir,
                "--text",
                text,
                "--out",
                &tmp.display().to_string(),
                "--num-threads",
                &self.config.num_threads.to_string(),
            ])
            .map_err(|e| match e {
                SherpaError::DriverFailed { stderr } => loomos_audio::TtsError::Unavailable(stderr),
                other => loomos_audio::TtsError::Unavailable(other.to_string()),
            });
        if let Err(e) = &result {
            let _ = std::fs::remove_file(&tmp);
            return Err(loomos_audio::TtsError::Unavailable(e.to_string()));
        }
        let (samples, rate) = crate::read_wav_pcm16(&tmp)
            .map_err(|e| loomos_audio::TtsError::Unavailable(e.to_string()))?;
        let json = result.unwrap();
        let v = parse_json_line(&json)
            .map_err(|e| loomos_audio::TtsError::Unavailable(e.to_string()))?;
        let _ = v["sample_rate"].as_u64();
        let _ = std::fs::remove_file(&tmp);
        Ok(TtsOutput {
            samples,
            rate,
            text: text.to_string(),
            chunks: loomos_audio::tts::split_for_tts(text, 20),
        })
    }
}

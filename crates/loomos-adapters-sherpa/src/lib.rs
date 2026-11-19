//! sherpa-onnx 真引擎适配器（03/04 号文档 §2 的删减路线：引擎侧走
//! sherpa-onnx 官方 Python API 子进程，**平台无关**——Windows / aarch64 Linux
//! 同一套命令与 wheel，不引入任何 C 链接依赖）。
//!
//! 边界（agent/tasks/add-real-engine.md）：
//! - 默认构建/测试不依赖真模型：模型缺失 → `SherpaError::Unavailable`，测试 skip；
//! - 驱动脚本随 crate 交付（driver/sherpa_driver.py），板端只需
//!   `pip install sherpa-onnx`（aarch64 wheel 官方发布）+ 模型目录；
//! - 唯一 onnxruntime：sherpa Python wheel 内置，与「唯一 runtime」纪律一致。

pub mod asr;
pub mod tts;

pub use asr::SherpaAsr;
pub use tts::SherpaTts;

use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;

/// sherpa 适配器配置（路径驱动，无 feature、无编译期依赖）。
#[derive(Debug, Clone)]
pub struct SherpaConfig {
    /// Python 解释器（默认 "python"；板上可用 venv 路径）。
    pub python: String,
    /// driver/sherpa_driver.py 路径。
    pub driver: PathBuf,
    /// ASR 模型目录（streaming-zipformer-small-bilingual-zh-en 2023-02-16，板同款 int8）。
    pub asr_model_dir: PathBuf,
    /// TTS 模型目录（vits-melo-tts-zh_en）。
    pub tts_model_dir: PathBuf,
    /// 推理线程数。
    pub num_threads: i32,
}

impl SherpaConfig {
    /// 环境探测：python/driver/两个模型目录是否就位（测试据此 skip，不 fail）。
    pub fn available(&self) -> Result<(), SherpaError> {
        if !self.driver.exists() {
            return Err(SherpaError::Unavailable(format!(
                "driver 缺失: {}",
                self.driver.display()
            )));
        }
        for (name, dir) in [
            ("asr_model_dir", &self.asr_model_dir),
            ("tts_model_dir", &self.tts_model_dir),
        ] {
            if !dir.exists() {
                return Err(SherpaError::Unavailable(format!(
                    "{name} 缺失: {}",
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    /// ASR 路径就位检查（不要求 TTS）。
    pub fn available_asr(&self) -> Result<(), SherpaError> {
        if !self.driver.exists() {
            return Err(SherpaError::Unavailable(format!(
                "driver 缺失: {}",
                self.driver.display()
            )));
        }
        if !self.asr_model_dir.exists() {
            return Err(SherpaError::Unavailable(format!(
                "asr_model_dir 缺失: {}",
                self.asr_model_dir.display()
            )));
        }
        Ok(())
    }

    /// TTS 路径就位检查（不要求 ASR）。
    pub fn available_tts(&self) -> Result<(), SherpaError> {
        if !self.driver.exists() {
            return Err(SherpaError::Unavailable(format!(
                "driver 缺失: {}",
                self.driver.display()
            )));
        }
        if !self.tts_model_dir.exists() {
            return Err(SherpaError::Unavailable(format!(
                "tts_model_dir 缺失: {}",
                self.tts_model_dir.display()
            )));
        }
        Ok(())
    }

    fn run_driver(&self, args: &[&str]) -> Result<String, SherpaError> {
        let out = Command::new(&self.python)
            .arg(self.driver.display().to_string())
            .args(args)
            .output()
            .map_err(|e| SherpaError::Unavailable(format!("python 启动失败: {e}")))?;
        if !out.status.success() {
            return Err(SherpaError::DriverFailed {
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[derive(Error, Debug, Clone)]
pub enum SherpaError {
    #[error("sherpa-onnx 不可用（模型/驱动缺失）: {0}")]
    Unavailable(String),
    #[error("sherpa driver 执行失败: {stderr}")]
    DriverFailed { stderr: String },
    #[error("输出解析失败: {0}")]
    Parse(String),
}

impl SherpaError {
    /// 与 loomos-audio 错误类型的桥接判断：Unavailable 类错误供测试 skip。
    pub fn is_unavailable(&self) -> bool {
        matches!(self, SherpaError::Unavailable(_))
    }
}

/// driver 的 JSON 单行输出解析。
pub(crate) fn parse_json_line(stdout: &str) -> Result<serde_json::Value, SherpaError> {
    let line = stdout
        .lines()
        .rfind(|l| l.trim_start().starts_with('{'))
        .ok_or_else(|| SherpaError::Parse(format!("无 JSON 输出: {stdout:?}")))?;
    serde_json::from_str(line).map_err(|e| SherpaError::Parse(e.to_string()))
}

/// 写 PCM16 单声道 WAV（临时文件用；不引 tempfile——见 EVIDENCE 工程决策）。
pub(crate) fn write_wav_pcm16(
    path: &PathBuf,
    samples: &[i16],
    rate: u32,
) -> Result<(), SherpaError> {
    use std::io::Write;
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    let mut f = std::fs::File::create(path).map_err(|e| SherpaError::Unavailable(e.to_string()))?;
    f.write_all(&out)
        .map_err(|e| SherpaError::Unavailable(e.to_string()))?;
    Ok(())
}

/// 读 PCM16 单声道 WAV → (samples, rate)。
pub fn read_wav_pub(path: &PathBuf) -> Result<(Vec<i16>, u32), SherpaError> {
    read_wav_pcm16(path)
}

pub(crate) fn read_wav_pcm16(path: &PathBuf) -> Result<(Vec<i16>, u32), SherpaError> {
    let bytes = std::fs::read(path).map_err(|e| SherpaError::Unavailable(e.to_string()))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
        return Err(SherpaError::Parse("非 WAV".into()));
    }
    // 找 data chunk（16 字节 fmt + 8 字节头之后简化定位）
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let data = &bytes[44..];
    let samples: Vec<i16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok((samples, rate))
}

/// 环境默认根（EVIDENCE/测试约定：模型放 LOOMOS_SHERPA_ROOT 下）。
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("LOOMOS_SHERPA_ROOT").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_parse_tolerates_python_noise_lines() {
        let out = "Warming up...\n{\"text\": \"打开客厅的灯\"}\ndone\n";
        let v = parse_json_line(out).unwrap();
        assert_eq!(v["text"], "打开客厅的灯");
        assert!(parse_json_line("no json here").is_err());
    }

    #[test]
    fn config_available_reports_missing_paths() {
        let cfg = SherpaConfig {
            python: "python".into(),
            driver: PathBuf::from("nonexistent/driver.py"),
            asr_model_dir: PathBuf::from("nonexistent/asr"),
            tts_model_dir: PathBuf::from("nonexistent/tts"),
            num_threads: 1,
        };
        let err = cfg.available().unwrap_err();
        assert!(err.is_unavailable());
    }

    #[test]
    fn wav_roundtrip() {
        let path = std::env::temp_dir().join(format!("loomos-sys-test-{}.wav", std::process::id()));
        write_wav_pcm16(&path, &[1, -2, 3], 16000).unwrap();
        let (samples, rate) = read_wav_pcm16(&path).unwrap();
        assert_eq!(samples, vec![1, -2, 3]);
        assert_eq!(rate, 16000);
        let _ = std::fs::remove_file(&path);
    }
}

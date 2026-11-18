//! 最小 WAV 读写（PCM16 单声道）。不引入依赖：原型只需要自己的夹具与产出。

use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum WavError {
    #[error("不是 RIFF/WAVE 文件")]
    NotWav,
    #[error("不支持的格式：期望 PCM16 单声道（fmt={0:?})")]
    UnsupportedFormat(String),
    #[error("数据不完整：声明 {declared} 字节，实际 {actual}")]
    Truncated { declared: usize, actual: usize },
}

/// 读取 PCM16 单声道 WAV，返回 (samples, rate)。
pub fn read_wav_mono_pcm16(bytes: &[u8]) -> Result<(Vec<i16>, u32), WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    return Err(WavError::UnsupportedFormat("fmt chunk too short".into()));
                }
                let format_tag = u16::from_le_bytes([body[0], body[1]]);
                channels = u16::from_le_bytes([body[2], body[3]]);
                rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                bits = u16::from_le_bytes([body[14], body[15]]);
                if format_tag != 1 {
                    return Err(WavError::UnsupportedFormat(format!(
                        "format_tag={format_tag}"
                    )));
                }
            }
            b"data" => {
                data = Some(body);
            }
            _ => {}
        }
        pos = body_start + size + (size & 1); // chunk 对齐
    }
    let data = data.ok_or(WavError::UnsupportedFormat("missing data chunk".into()))?;
    if channels != 1 {
        return Err(WavError::UnsupportedFormat(format!("channels={channels}")));
    }
    if bits != 16 {
        return Err(WavError::UnsupportedFormat(format!("bits={bits}")));
    }
    let samples: Vec<i16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok((samples, rate))
}

/// 写 PCM16 单声道 WAV。
pub fn write_wav_mono_pcm16(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    // fmt
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    // data
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let samples: Vec<i16> = (0..1000).map(|i| ((i * 7) % 32768) as i16).collect();
        let bytes = write_wav_mono_pcm16(&samples, 16_000);
        let (back, rate) = read_wav_mono_pcm16(&bytes).unwrap();
        assert_eq!(rate, 16_000);
        assert_eq!(back, samples);
    }

    #[test]
    fn rejects_non_wav() {
        assert_eq!(read_wav_mono_pcm16(b"hello").unwrap_err(), WavError::NotWav);
    }
}

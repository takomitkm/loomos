//! 数字信号处理：重采样（选型沿用旧板实测）、能量度量。
//!
//! 实测依据（docs/dev/EVIDENCE.md，旧板 2026-10-30 读数）：
//! - 升采样 22050→48000 用 linear 插值：16.6 ms/2s，真实语音频谱相关 0.9997；
//!   polyphase 慢 107 倍，不采用。
//! - 降采样 48000→16000 用 decimate3-mavg3：3 点平均把 20 kHz 混叠压到 −12.3 dB，
//!   与 31 抽 sinc 低通的 ASR 识别结果逐字一致，快 11–14 倍；
//!   8 kHz 处 −3.5 dB、16 kHz（新 Nyquist）处 −∞，与 3 点均值频响吻合。
//! - 重采样总开销相对 TTS 推理 <0.6%，继续优化无意义。

/// 3 点滑动平均 + 每 3 抽 1（48k → 16k）。
pub fn decimate3_mavg3(x: &[i16]) -> Vec<i16> {
    let n = x.len() / 3;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = x[3 * i] as i32;
        let b = x[3 * i + 1] as i32;
        let c = x[3 * i + 2] as i32;
        out.push(((a + b + c) / 3) as i16);
    }
    out
}

/// linear 插值重采样（np.interp 等价：整段等距映射）。用于 22050→48000 升采样。
pub fn linear_resample(x: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let n_out = ((x.len() as u64) * (to as u64) / (from as u64)) as usize;
    let mut out = Vec::with_capacity(n_out);
    if n_out == 1 {
        out.push(x[0]);
        return out;
    }
    let step = (x.len() - 1) as f64 / (n_out - 1) as f64;
    for i in 0..n_out {
        let pos = i as f64 * step;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(x.len() - 1);
        let frac = pos - i0 as f64;
        let a = x[i0] as f64;
        let b = x[i1] as f64;
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

/// RMS。
pub fn rms(x: &[i16]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let sum: f64 = x.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / x.len() as f64).sqrt()
}

/// dBFS（0 dBFS = 满幅 32768）。
pub fn dbfs(x: &[i16]) -> f64 {
    let r = rms(x);
    if r <= 0.0 {
        return -120.0;
    }
    20.0 * (r / 32768.0).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimate3_shrinks_by_three() {
        let x: Vec<i16> = (0..3000).map(|i| (i % 97) as i16).collect();
        let y = decimate3_mavg3(&x);
        assert_eq!(y.len(), 1000);
    }

    #[test]
    fn decimate3_kills_new_nyquist_component() {
        // 16 kHz 分量（48k 的第 3 Nyquist）经 3 点平均应近乎消失（−∞）
        let fs = 48_000f64;
        let x: Vec<i16> = (0..4800)
            .map(|i| {
                let t = i as f64 / fs;
                let v = (16_000.0 * t * std::f64::consts::TAU).sin() * 10_000.0;
                v as i16
            })
            .collect();
        let y = decimate3_mavg3(&x);
        let energy = rms(&y);
        assert!(
            energy < 1.0,
            "16kHz（新 Nyquist）分量应被 3 点平均精确消除，实际 rms={energy}"
        );
    }

    #[test]
    fn linear_resample_ratio_and_smoothness() {
        let x: Vec<i16> = (0..2205).map(|i| (i * 31 % 2000) as i16).collect();
        let y = linear_resample(&x, 22_050, 48_000);
        assert_eq!(y.len(), 4800);
        // 单调缓变信号插值应保序
        let ramp: Vec<i16> = (0..2205).map(|i| (i * 14) as i16).collect();
        let r = linear_resample(&ramp, 22_050, 48_000);
        assert!(r.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn dbfs_scale() {
        let full: Vec<i16> = vec![32767; 1000];
        let d = dbfs(&full);
        assert!((d - 0.0).abs() < 0.01, "满幅应为 0 dBFS，实际 {d}");
        assert!(dbfs(&[0; 1000]) <= -120.0);
    }
}

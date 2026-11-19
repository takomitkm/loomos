//! 真引擎回归：TTS → ASR 往返（同一份 sherpa-onnx，Windows/aarch64 同命令）。
//!
//! 运行条件（缺任一 → eprintln skip 并 PASS，不 fail——add-real-engine 纪律）：
//! - 环境变量 LOOMOS_SHERPA_ROOT 指向模型根，其下有
//!   `asr/`（streaming-zipformer-small-bilingual-zh-en 2023-02-16 解包）
//!   `tts/`（vits-melo-tts-zh_en 解包）
//! - 本机 `pip install sherpa-onnx`
//!   通过后把读数回填 docs/dev/EVIDENCE.md（引擎真跑证据）。

use loomos_adapters_sherpa::{SherpaAsr, SherpaConfig, SherpaTts};
use loomos_audio::{AsrBackend, TtsBackend};
use std::path::PathBuf;

fn config() -> Option<SherpaConfig> {
    let cfg = base_config()?;
    cfg.available().ok()?;
    Some(cfg)
}

/// 只要求 ASR 就位（TTS 未下完时也能真跑 ASR 回归）。
fn base_config() -> Option<SherpaConfig> {
    let root = std::env::var("LOOMOS_SHERPA_ROOT").ok()?;
    let root = PathBuf::from(root);
    Some(SherpaConfig {
        python: std::env::var("LOOMOS_SHERPA_PYTHON").unwrap_or_else(|_| "python".into()),
        driver: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("driver/sherpa_driver.py"),
        asr_model_dir: root.join("asr"),
        tts_model_dir: root.join("tts"),
        num_threads: 2,
    })
}

#[test]
fn real_tts_then_asr_roundtrip() {
    let Some(cfg) = config() else {
        eprintln!("skip: LOOMOS_SHERPA_ROOT / sherpa-onnx / 模型未就位（Assumed, 不 fail）");
        return;
    };
    let text = "打开客厅的灯";
    let tts = SherpaTts::new(cfg.clone());
    let out = tts
        .synthesize(text)
        .expect("真 TTS 合成失败（模型在但推理出错——这是 fail，不是 skip）");
    assert_eq!(out.text, text);
    assert!(
        out.samples.len() > 8_000,
        "合成音频过短: {}",
        out.samples.len()
    );

    // 合成出的 44100 wav → 线性重采样 16k（驱动内做）→ 真 ASR
    let asr = SherpaAsr::new(cfg);
    let result = asr
        .transcribe(&out.samples, out.rate)
        .expect("真 ASR 识别失败（模型在但推理出错——这是 fail，不是 skip）");
    eprintln!("真 ASR 转写: {:?}", result.text);
    // 模糊断言：真引擎文字有变体（流式模型短句可能丢尾字），
    // 但至少要命中一个内容词，且非空
    let content_words = ["打开", "客厅", "灯"];
    assert!(
        !result.text.trim().is_empty() && content_words.iter().any(|w| result.text.contains(w)),
        "真 ASR 转写应命中内容词 {{打开,客厅,灯}}，实际: {:?}",
        result.text
    );
}

#[test]
fn real_asr_on_shipped_test_wav() {
    let Some(cfg) = base_config() else {
        eprintln!("skip: LOOMOS_SHERPA_ROOT / sherpa-onnx / 模型未就位（Assumed, 不 fail）");
        return;
    };
    // 模型包自带真人语音 test_wavs（0.wav，zh）；真 ASR 必须产出非空中文文本
    let wav = cfg
        .asr_model_dir
        .parent()
        .map(|p| p.join("asr"))
        .unwrap_or_else(|| cfg.asr_model_dir.clone())
        .join("test_wavs/0.wav");
    let wav = if wav.exists() {
        wav
    } else {
        cfg.asr_model_dir.join("test_wavs/0.wav")
    };
    if !wav.exists() {
        eprintln!("skip: test_wavs/0.wav 不存在");
        return;
    }
    let (samples, rate) = loomos_adapters_sherpa::read_wav_pub(&wav).expect("读 test_wav 失败");
    let asr = SherpaAsr::new(cfg);
    let result = asr
        .transcribe(&samples, rate)
        .expect("真 ASR 识别失败（这是 fail）");
    eprintln!("真 ASR（官方样例）: {:?}", result.text);
    assert!(!result.text.trim().is_empty(), "转写不应为空");
}

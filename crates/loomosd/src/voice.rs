//! 语音管线：source → 重采样(48k→16k decimate3-mavg3) → ASR → KWS 唤醒门 →
//! 执行器 → TTS(sine 档) → linear 升采样(22.05k→48k) → WAV 落盘。
//!
//! 链级顺序沿旧系统（sar → rms → kws → vad → asr → router → tts → sar 输出重采样）；
//! 默认档全 mock 全自检（echo/sine 纪律）；每级耗时逐段记录（stages_ms 契约）。

use crate::system::System;
use loomos_audio::{
    AsrBackend, EchoAsr, KwsBackend, SidecarAsr, SpeechSegmenter, TextGateKws, TtsBackend,
    TtsOutput,
};
use loomos_executor::ExecReport;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

/// 每级耗时（ms）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageTimings {
    pub resample_ms: u128,
    pub asr_ms: u128,
    pub kws_ms: u128,
    pub vad_ms: u128,
    pub executor_ms: u128,
    pub tts_ms: u128,
    pub total_ms: u128,
}

/// 一次语音回合的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnOutcome {
    pub wake_hit: Option<String>,
    pub transcript: Option<String>,
    /// 被跳过的原因（未唤醒 / 未识别）。
    pub skipped: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ExecReport>,
    /// TTS 文本与落盘文件。
    pub spoken_text: Option<String>,
    pub speech_file: Option<String>,
    pub stages_ms: StageTimings,
}

/// 语音管线。可变状态（sidecar 表）以内部可变性持有。
pub struct VoicePipeline {
    pub hotword: String,
    pub wake_required: bool,
    pub kws: Mutex<Box<dyn KwsBackend>>,
    pub vad: SpeechSegmenter,
    pub asr: Mutex<Box<dyn AsrBackend>>,
    pub tts: Mutex<Box<dyn TtsBackend>>,
    pub speech_dir: Option<PathBuf>,
    seq: Mutex<u64>,
}

impl VoicePipeline {
    /// 默认档：text-gate KWS + energy VAD + sidecar ASR + sine TTS。
    /// `fixtures`: (wav_bytes, text) 列表 → 注册进 SidecarAsr。
    pub fn default_profile(fixtures: Vec<(Vec<i16>, String)>, speech_dir: Option<PathBuf>) -> Self {
        let mut sidecar = SidecarAsr::from_pairs(vec![]);
        for (samples, text) in fixtures {
            sidecar.register(&samples, &text);
        }
        Self {
            hotword: "小克".into(),
            wake_required: true,
            kws: Mutex::new(Box::new(TextGateKws::xiaoke())),
            vad: SpeechSegmenter::default(),
            asr: Mutex::new(Box::new(sidecar)),
            tts: Mutex::new(Box::new(loomos_audio::SineTts::default())),
            speech_dir,
            seq: Mutex::new(0),
        }
    }

    /// Echo 档（无夹具也能全链自检：转写占位文本，执行器会拒绝但不崩）。
    pub fn echo_profile(speech_dir: Option<PathBuf>) -> Self {
        Self {
            hotword: "小克".into(),
            wake_required: true,
            kws: Mutex::new(Box::new(TextGateKws::xiaoke())),
            vad: SpeechSegmenter::default(),
            asr: Mutex::new(Box::new(EchoAsr)),
            tts: Mutex::new(Box::new(loomos_audio::SineTts::default())),
            speech_dir,
            seq: Mutex::new(0),
        }
    }

    /// 处理一个音频回合（mic 帧/夹具 wav）。
    pub fn run_turn(&self, sys: &System, source: &str, samples: &[i16], rate: u32) -> TurnOutcome {
        let t0 = Instant::now();
        let mut stages = StageTimings::default();

        // 1. 重采样到推理率（48k → decimate3-mavg3 → 16k；已 16k 则直通）
        let t = Instant::now();
        let at16k: Vec<i16> = if rate == loomos_audio::RATE_INFERENCE {
            samples.to_vec()
        } else {
            loomos_audio::dsp::decimate3_mavg3(samples)
        };
        stages.resample_ms = t.elapsed().as_millis();

        // 2. VAD 切段（夹具一条=一段；能量门先筛静音）
        let t = Instant::now();
        let segments = self.vad.segment(&at16k, loomos_audio::RATE_INFERENCE);
        stages.vad_ms = t.elapsed().as_millis();
        if segments.is_empty() {
            stages.total_ms = t0.elapsed().as_millis();
            return TurnOutcome {
                wake_hit: None,
                transcript: None,
                skipped: Some("silence".into()),
                report: None,
                spoken_text: None,
                speech_file: None,
                stages_ms: stages,
            };
        }

        // 3. ASR
        let t = Instant::now();
        let asr_result = self
            .asr
            .lock()
            .unwrap()
            .transcribe(&segments[0].samples, loomos_audio::RATE_INFERENCE);
        stages.asr_ms = t.elapsed().as_millis();
        let transcript = match asr_result {
            Ok(r) => r.text,
            Err(_) => {
                stages.total_ms = t0.elapsed().as_millis();
                return TurnOutcome {
                    wake_hit: None,
                    transcript: None,
                    skipped: Some("asr_no_transcript".into()),
                    report: None,
                    spoken_text: None,
                    speech_file: None,
                    stages_ms: stages,
                };
            }
        };

        // 4. KWS 唤醒门（旧链：命中才往下走）
        let t = Instant::now();
        let hit = self.kws.lock().unwrap().detect_text(&transcript);
        stages.kws_ms = t.elapsed().as_millis();
        if self.wake_required && hit.is_none() {
            sys.events.lock().unwrap().append(
                loomos_runtime::event::EventKind::Utterance,
                "voice",
                serde_json::json!({"source": source, "text": transcript, "wake": false}),
            );
            stages.total_ms = t0.elapsed().as_millis();
            return TurnOutcome {
                wake_hit: None,
                transcript: Some(transcript),
                skipped: Some("not_woken".into()),
                report: None,
                spoken_text: None,
                speech_file: None,
                stages_ms: stages,
            };
        }

        sys.events.lock().unwrap().append(
            loomos_runtime::event::EventKind::WakeDetected,
            "kws",
            serde_json::json!({"source": source, "keyword": hit.as_ref().map(|h| h.keyword.clone())}),
        );

        // 5. 执行（去掉唤醒词）
        let t = Instant::now();
        let stripped = strip_hotword(&transcript, &self.hotword);
        let vp = sys.voiceprint.lock().unwrap();
        let report = {
            let mut reg = sys.registry.lock().unwrap();
            sys.executor.handle_text(
                &stripped,
                &mut reg,
                &sys.caps,
                &sys.voice_token,
                Some(&vp),
                &segments[0].samples,
                loomos_audio::RATE_INFERENCE,
            )
        };
        drop(vp);
        stages.executor_ms = t.elapsed().as_millis();

        // 6. TTS（文本 → sine 波形 → 升采样 48k → WAV 落盘）
        let t = Instant::now();
        let (spoken, file) = self.speak(&report.speech);
        stages.tts_ms = t.elapsed().as_millis();
        sys.events.lock().unwrap().append(
            loomos_runtime::event::EventKind::TtsProduced,
            "tts",
            serde_json::json!({"text": spoken, "file": file}),
        );
        stages.total_ms = t0.elapsed().as_millis();

        TurnOutcome {
            wake_hit: hit.map(|h| h.keyword),
            transcript: Some(transcript),
            skipped: None,
            report: Some(report),
            spoken_text: Some(spoken),
            speech_file: file,
            stages_ms: stages,
        }
    }

    /// 合成 + 落盘（旧板无声卡 → WAV 文件纪律）。
    fn speak(&self, text: &str) -> (String, Option<String>) {
        let out = match self.tts.lock().unwrap().synthesize(text) {
            Ok(o) => o,
            Err(_) => return (text.to_string(), None),
        };
        let up =
            loomos_audio::dsp::linear_resample(&out.samples, out.rate, loomos_audio::RATE_OUTPUT);
        let file = match &self.speech_dir {
            Some(dir) => {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    tracing::warn!(error=%e, "speech dir 创建失败");
                    return (out.text.clone(), None);
                }
                let mut seq = self.seq.lock().unwrap();
                *seq += 1;
                let path = dir.join(format!("speech-{:04}.wav", *seq));
                match std::fs::write(
                    &path,
                    loomos_audio::wav::write_wav_mono_pcm16(&up, loomos_audio::RATE_OUTPUT),
                ) {
                    Ok(()) => Some(path.to_string_lossy().to_string()),
                    Err(e) => {
                        tracing::warn!(error=%e, "speech wav 写入失败");
                        None
                    }
                }
            }
            None => None,
        };
        (out.text.clone(), file)
    }

    /// 管线自检：echo 档空转一圈不 panic。
    pub fn self_check(&self) -> Result<(), String> {
        let probe: Vec<i16> = vec![0; loomos_audio::RATE_INFERENCE as usize / 2];
        self.vad.segment(&probe, loomos_audio::RATE_INFERENCE);
        self.kws.lock().unwrap().detect_text("小克");
        // 探针无转写属正常回包（NoTranscript）；只有后端不可用才算失败
        self.asr
            .lock()
            .unwrap()
            .transcribe(&probe, loomos_audio::RATE_INFERENCE)
            .map(|_| ())
            .or_else(|e| match e {
                loomos_audio::AsrError::NoTranscript => Ok(()),
                other => Err(other.to_string()),
            })?;
        self.tts
            .lock()
            .unwrap()
            .synthesize("自检")
            .map(|_: TtsOutput| ())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn strip_hotword(text: &str, hotword: &str) -> String {
    let mut t = text.trim();
    for w in [format!("{hotword}{hotword}"), hotword.to_string()] {
        if let Some(rest) = t.strip_prefix(w.as_str()) {
            t = rest.trim_start_matches(['，', '。', ' ', ',', '.']);
        }
    }
    // 整句就是唤醒词本身（「小克小克」）：保留原句——直连表有对应应答条目
    if t.is_empty() {
        return text.trim().to_string();
    }
    t.to_string()
}

/// 夹具描述（gen-fixtures 与 smoke 共用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureSpec {
    pub name: String,
    pub text: String,
    /// 音节数（决定波形）。
    pub syllables: usize,
}

/// 标准场景集（对齐旧板 8 条直连命令 + 语法/门控/未识别对照）。
pub fn standard_fixtures() -> Vec<FixtureSpec> {
    vec![
        FixtureSpec {
            name: "u01-who-is-there".into(),
            text: "小克，看看是谁".into(),
            syllables: 6,
        },
        FixtureSpec {
            name: "u02-start-record".into(),
            text: "小克，开始录制".into(),
            syllables: 5,
        },
        FixtureSpec {
            name: "u03-stop-record".into(),
            text: "小克，停止录制".into(),
            syllables: 5,
        },
        FixtureSpec {
            name: "u04-status".into(),
            text: "小克，现在什么状态".into(),
            syllables: 7,
        },
        FixtureSpec {
            name: "u05-time".into(),
            text: "小克，现在几点".into(),
            syllables: 5,
        },
        FixtureSpec {
            name: "u06-unlock".into(),
            text: "小克，我回来了开门".into(),
            syllables: 8,
        },
        FixtureSpec {
            name: "u07-light-on".into(),
            text: "小克，打开客厅的灯".into(),
            syllables: 7,
        },
        FixtureSpec {
            name: "u08-light-off".into(),
            text: "小克，把卧室的灯关掉".into(),
            syllables: 8,
        },
        FixtureSpec {
            name: "u09-dialogue".into(),
            text: "小克小克".into(),
            syllables: 3,
        },
        // 对照组
        FixtureSpec {
            name: "u10-no-wake".into(),
            text: "打开客厅的灯".into(),
            syllables: 6,
        },
        FixtureSpec {
            name: "u11-unknown".into(),
            text: "小克，今天天气怎么样".into(),
            syllables: 8,
        },
        FixtureSpec {
            name: "u12-negation".into(),
            text: "小克，不要开始录制".into(),
            syllables: 7,
        },
    ]
}

/// 从夹具文本生成波形：每个音节的音高由文本字符码点播种——
/// 确定性可复现，且不同文本的音频内容不同（sidecar ASR 的内容哈希才能区分）。
pub fn fixture_samples(spec: &FixtureSpec) -> Vec<i16> {
    let syllables = spec.syllables.max(1);
    let chars: Vec<u32> = spec.text.chars().map(|c| c as u32).collect();
    let rate = loomos_audio::RATE_INFERENCE;
    let mut samples = Vec::new();
    let lead = (rate as f64 * 0.2) as usize;
    samples.extend(std::iter::repeat_n(0i16, lead));
    for si in 0..syllables {
        let f = 250.0 + (chars[si % chars.len()] % 24) as f64 * 35.0;
        let voiced = (rate as f64 * 0.22) as usize;
        for i in 0..voiced {
            let t = i as f64 / rate as f64;
            let env = ((i as f64 / voiced as f64) * 4.0)
                .min(1.0)
                .min(((voiced - i) as f64 / voiced as f64) * 4.0);
            let v = (f * t * std::f64::consts::TAU).sin() * env * 20_000.0;
            samples.push(v as i16);
        }
        if si != syllables - 1 {
            samples.extend(std::iter::repeat_n(0i16, (rate as f64 * 0.08) as usize));
        }
    }
    samples.extend(std::iter::repeat_n(0i16, lead * 2));
    samples
}

/// 夹具清单 JSON（fixtures/manifest.json 形状）。
pub type FixtureManifest = BTreeMap<String, String>;

//! normos CLI：原型的一键执行面。
//!
//! 子命令：
//! - `gen-fixtures <dir>`      生成标准场景夹具（wav + sidecar + manifest）
//! - `smoke <dir> [--out DIR]` **一键原型验收**：装配 → 回放全部夹具 → 断言 → 报告
//! - `replay <wav> [--fixtures DIR]`：回放单个 wav
//! - `say <text>`：文本直接走执行器 + TTS
//! - `serve`：等同 normosd
//!
//! 退出码：smoke 全过 = 0；任何失败 = 1（供 agent/CI 判断）。

use normos_audio::wav::{read_wav_mono_pcm16, write_wav_mono_pcm16};
use normos_audio::{RATE_INFERENCE, RATE_OUTPUT, SpeechSegmenter, dsp};
use normos_vision::provider::VisionMode;
use normosd::console::ConsoleServer;
use normosd::system::{System, SystemConfig};
use normosd::voice::{FixtureSpec, TurnOutcome, VoicePipeline, fixture_samples, standard_fixtures};
use std::path::{Path, PathBuf};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,normos=info")),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        Some("gen-fixtures") => cmd_gen_fixtures(
            args.get(1)
                .map(PathBuf::from)
                .unwrap_or_else(default_fixtures_dir),
        ),
        Some("smoke") => cmd_smoke(
            args.get(1)
                .map(PathBuf::from)
                .unwrap_or_else(default_fixtures_dir),
            flag_value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(default_reports_dir),
        ),
        Some("replay") => cmd_replay(
            args.get(1).cloned().unwrap_or_default(),
            flag_value(&args, "--fixtures")
                .map(PathBuf::from)
                .unwrap_or_else(default_fixtures_dir),
        ),
        Some("say") => cmd_say(args.get(1).cloned().unwrap_or_default()),
        Some("serve") => cmd_serve(),
        _ => {
            eprintln!(
                "用法: normos-cli <gen-fixtures|smoke|replay|say|serve> [参数]\n\
                 \x20gen-fixtures <dir>             生成标准夹具\n\
                 \x20smoke [dir] [--out DIR]        一键原型验收（写报告）\n\
                 \x20replay <wav> [--fixtures DIR]  回放单个 wav\n\
                 \x20say <text>                     文本走执行器+TTS\n\
                 \x20serve                          启动控制台服务"
            );
            2
        }
    };
    std::process::exit(code);
}

fn default_fixtures_dir() -> PathBuf {
    PathBuf::from("fixtures")
}

fn default_reports_dir() -> PathBuf {
    PathBuf::from("reports")
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

// ---------------- gen-fixtures ----------------

fn cmd_gen_fixtures(dir: PathBuf) -> i32 {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("创建夹具目录失败: {e}");
        return 1;
    }
    let specs = standard_fixtures();
    let mut manifest: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for spec in &specs {
        let samples = fixture_samples(spec);
        let wav = write_wav_mono_pcm16(&samples, RATE_OUTPUT); // 48k 源（模拟 mic 降采样路径）
        let wav_path = dir.join(format!("{}.wav", spec.name));
        if let Err(e) = std::fs::write(&wav_path, wav) {
            eprintln!("写 {} 失败: {e}", wav_path.display());
            return 1;
        }
        manifest.insert(
            spec.name.clone(),
            serde_json::json!({"text": spec.text, "syllables": spec.syllables, "file": wav_path.file_name().unwrap().to_string_lossy()}),
        );
    }
    let manifest_path = dir.join("manifest.json");
    if let Err(e) = std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&serde_json::Value::Object(manifest)).unwrap(),
    ) {
        eprintln!("写 manifest 失败: {e}");
        return 1;
    }
    println!("已生成 {} 条夹具到 {}", specs.len(), dir.display());
    0
}

// ---------------- smoke（一键验收） ----------------

#[derive(Debug)]
struct ScenarioExpectation {
    name: &'static str,
    wake: bool,
    intent: Option<&'static str>,
    speech_contains: &'static str,
    /// skipped 原因（no-wake 场景）。
    skipped: Option<&'static str>,
}

fn expectations() -> Vec<ScenarioExpectation> {
    vec![
        ScenarioExpectation {
            name: "u01-who-is-there",
            wake: true,
            intent: Some("WhoIsThere"),
            speech_contains: "门口是张三",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u02-start-record",
            wake: true,
            intent: Some("StartRecording"),
            speech_contains: "开始录制",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u03-stop-record",
            wake: true,
            intent: Some("StopRecording"),
            speech_contains: "已停止录制",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u04-status",
            wake: true,
            intent: Some("QueryStatus"),
            speech_contains: "录制模式",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u05-time",
            wake: true,
            intent: Some("QueryTime"),
            speech_contains: "现在是12:00",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u06-unlock",
            wake: true,
            intent: Some("ArriveHome"),
            speech_contains: "门已打开",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u07-light-on",
            wake: true,
            intent: Some("HassTurnOn"),
            speech_contains: "客厅的灯开了",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u08-light-off",
            wake: true,
            intent: Some("HassTurnOff"),
            speech_contains: "卧室的灯关了",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u09-dialogue",
            wake: true,
            intent: Some("DialogueReply"),
            speech_contains: "我在",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u10-no-wake",
            wake: false,
            intent: None,
            speech_contains: "",
            skipped: Some("not_woken"),
        },
        ScenarioExpectation {
            name: "u11-unknown",
            wake: true,
            intent: None,
            speech_contains: "没听懂",
            skipped: None,
        },
        ScenarioExpectation {
            name: "u12-negation",
            wake: true,
            intent: None,
            speech_contains: "直接说指令",
            skipped: None,
        },
    ]
}

fn cmd_smoke(fixtures_dir: PathBuf, out_dir: PathBuf) -> i32 {
    println!("== normos 一键原型验收 ==");
    if !fixtures_dir.exists() {
        println!("夹具目录 {} 不存在，先生成…", fixtures_dir.display());
        let code = cmd_gen_fixtures(fixtures_dir.clone());
        if code != 0 {
            return code;
        }
    }

    // 1. 读夹具
    let mut fixtures: Vec<(FixtureSpec, Vec<i16>)> = Vec::new();
    for spec in standard_fixtures() {
        let wav_path = fixtures_dir.join(format!("{}.wav", spec.name));
        let bytes = match std::fs::read(&wav_path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("读 {} 失败: {e}", wav_path.display());
                return 1;
            }
        };
        let (samples, rate) = match read_wav_mono_pcm16(&bytes) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("解析 {} 失败: {e}", wav_path.display());
                return 1;
            }
        };
        if rate != RATE_OUTPUT {
            eprintln!(
                "{} 采样率应为 {RATE_OUTPUT}，实际 {rate}",
                wav_path.display()
            );
            return 1;
        }
        fixtures.push((spec, samples));
    }
    println!("夹具加载完成：{} 条", fixtures.len());

    // 2. 准备 ASR sidecar：48k → decimate3 → 16k → VAD 切段 → 首段注册
    //    （与管线运行时完全同路径，保证 key 可复现）
    let segmenter = SpeechSegmenter::default();
    let mut asr_entries: Vec<(Vec<i16>, String)> = Vec::new();
    for (spec, samples48) in &fixtures {
        let at16k = dsp::decimate3_mavg3(samples48);
        let segs = segmenter.segment(&at16k, RATE_INFERENCE);
        let Some(seg) = segs.first() else {
            eprintln!("夹具 {} 切不出语音段", spec.name);
            return 1;
        };
        asr_entries.push((seg.samples.clone(), spec.text.clone()));
    }

    // 3. 装配系统（mock 人脸=张三，PAD=model_live 未标定——对齐旧板 PAD 现状）
    let out_reports = out_dir.join("prototype-report.md");
    let speech_dir = out_dir.join("speech");
    let sys = match System::assemble(SystemConfig {
        data_dir: Some(out_dir.join("data")),
        speech_dir: Some(speech_dir.clone()),
        faults: normos_device::mock::MockFaults::default(),
        // 自研视频路（Pipeline 模式）：默认 gallery 含张三，PAD 未标定（对齐旧板现状）
        vision: VisionMode::Pipeline(Default::default()),
        console_port: 0,
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("装配失败: {e}");
            return 1;
        }
    };

    // 4. 声纹登记（u06 开门场景第 4 道闸）
    {
        let mut vp = sys.voiceprint.lock().unwrap();
        let (_, u06) = fixtures
            .iter()
            .find(|(s, _)| s.name == "u06-unlock")
            .unwrap();
        let at16k = dsp::decimate3_mavg3(u06);
        let segs = segmenter.segment(&at16k, RATE_INFERENCE);
        if let Some(seg) = segs.first()
            && let Err(e) = vp.enroll("张三", &seg.samples, RATE_INFERENCE)
        {
            eprintln!("声纹登记失败: {e}");
            return 1;
        }
    }

    // 5. 建管线 + 全场景回放
    let pipeline = VoicePipeline::default_profile(asr_entries, Some(speech_dir.clone()));
    if let Err(e) = pipeline.self_check() {
        eprintln!("管线自检失败: {e}");
        return 1;
    }

    let mut rows: Vec<String> = Vec::new();
    let mut latencies: Vec<&'static str> = Vec::new();
    let mut all_pass = true;
    let mut stage_stats: std::collections::BTreeMap<&'static str, Vec<u128>> = Default::default();

    for (spec, samples48) in &fixtures {
        let outcome = pipeline.run_turn(&sys, &spec.name, samples48, RATE_OUTPUT);
        let exp = expectations()
            .into_iter()
            .find(|e| e.name == spec.name)
            .expect("标准场景必须有条目");
        let (pass, why) = check_expectation(&outcome, &exp);
        if !pass {
            all_pass = false;
        }
        latencies.push(if pass { "PASS" } else { "FAIL" });
        let s = &outcome.stages_ms;
        stage_stats
            .entry("resample")
            .or_default()
            .push(s.resample_ms);
        stage_stats.entry("vad").or_default().push(s.vad_ms);
        stage_stats.entry("asr").or_default().push(s.asr_ms);
        stage_stats.entry("kws").or_default().push(s.kws_ms);
        stage_stats
            .entry("executor")
            .or_default()
            .push(s.executor_ms);
        stage_stats.entry("tts").or_default().push(s.tts_ms);
        stage_stats.entry("total").or_default().push(s.total_ms);
        let verdict = if pass {
            "PASS".to_string()
        } else {
            format!("FAIL: {why}")
        };
        let intent = outcome
            .report
            .as_ref()
            .and_then(|r| r.intent.clone())
            .unwrap_or_else(|| "-".into());
        let speech = outcome
            .report
            .as_ref()
            .map(|r| r.speech.clone())
            .or(outcome.skipped.clone().map(|x| format!("(跳过: {x})")))
            .unwrap_or_default();
        rows.push(format!(
            "| {} | {} | {} | {} | {} | {} |",
            spec.name,
            outcome.transcript.clone().unwrap_or_else(|| "-".into()),
            if outcome.wake_hit.is_some() {
                "✓"
            } else {
                "✗"
            },
            intent,
            speech.replace('|', "，"),
            verdict,
        ));
        println!(
            "  {} {} → {}",
            if pass { "✓" } else { "✗" },
            spec.name,
            speech
        );
    }

    // 6. 语音落盘检查
    let speech_files = std::fs::read_dir(&speech_dir)
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0);
    let spoken = rows.len() - 1; // u10 不说话
    let speech_ok = speech_files >= spoken.saturating_sub(1);

    // 7. 事件检查
    let events_total = sys.events.lock().unwrap().len();
    let events_ok = events_total > 10;

    // 8. 报告
    let report = build_report(
        &rows,
        &stage_stats,
        &latencies,
        events_total,
        speech_files,
        all_pass && speech_ok && events_ok,
    );
    if let Some(parent) = out_reports.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&out_reports, report) {
        eprintln!("写报告失败: {e}");
        return 1;
    }
    println!("报告已写入 {}", out_reports.display());
    println!(
        "== 结果: {} ==",
        if all_pass && speech_ok && events_ok {
            "全部通过"
        } else {
            "存在失败项"
        }
    );
    if all_pass && speech_ok && events_ok {
        0
    } else {
        1
    }
}

fn check_expectation(outcome: &TurnOutcome, exp: &ScenarioExpectation) -> (bool, String) {
    if let Some(skip) = exp.skipped {
        return (
            outcome.skipped.as_deref() == Some(skip),
            format!("期望 skipped={skip}，实际 {:?}", outcome.skipped),
        );
    }
    let report = match &outcome.report {
        Some(r) => r,
        None => return (false, format!("期望执行，实际跳过 {:?}", outcome.skipped)),
    };
    let wake_ok = exp.wake == outcome.wake_hit.is_some();
    let intent_ok = match (&exp.intent, &report.intent) {
        (Some(want), Some(got)) => want == got,
        (None, None) => true,
        _ => false,
    };
    let speech_ok = report.speech.contains(exp.speech_contains);
    (
        wake_ok && intent_ok && speech_ok,
        format!(
            "wake={} (期望 {}) intent={:?} (期望 {:?}) speech={:?} (期望含 {:?})",
            outcome.wake_hit.is_some(),
            exp.wake,
            report.intent,
            exp.intent,
            report.speech,
            exp.speech_contains
        ),
    )
}

fn percentile(v: &[u128], p: f64) -> u128 {
    let mut s = v.to_vec();
    s.sort_unstable();
    let idx = ((s.len() as f64 - 1.0) * p).round() as usize;
    s.get(idx).copied().unwrap_or(0)
}

fn build_report(
    rows: &[String],
    stage_stats: &std::collections::BTreeMap<&'static str, Vec<u128>>,
    verdicts: &[&str],
    events_total: usize,
    speech_files: usize,
    all_pass: bool,
) -> String {
    let pass_count = verdicts.iter().filter(|v| **v == "PASS").count();
    let mut s = String::new();
    s.push_str("# normos 原型验收报告\n\n");
    s.push_str(&format!(
        "- 时间：{}（Unix ms）\n- 版本：normos-runtime {}\n- 平台：{} / {}\n- 工具链：rustc {}\n",
        now_unix_ms(),
        normos_runtime::RUNTIME_VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        rustc_version(),
    ));
    s.push_str(&format!(
        "\n## 场景结果（{pass_count}/{} PASS）\n\n",
        rows.len()
    ));
    s.push_str("| 场景 | 转写 | 唤醒 | 意图 | 回执/跳过 | 判定 |\n|---|---|---|---|---|---|\n");
    for r in rows {
        s.push_str(r);
        s.push('\n');
    }
    s.push_str("\n## 各级耗时（ms，min/P50/P95/max）\n\n");
    s.push_str("| 阶段 | min | P50 | P95 | max |\n|---|---|---|---|---|\n");
    for (stage, v) in stage_stats {
        s.push_str(&format!(
            "| {stage} | {} | {} | {} | {} |\n",
            v.iter().min().copied().unwrap_or(0),
            percentile(v, 0.5),
            percentile(v, 0.95),
            v.iter().max().copied().unwrap_or(0),
        ));
    }
    s.push_str(&format!(
        "\n## 证据\n\n- 事件总数：{events_total}\n- 语音落盘文件：{speech_files} 个 WAV（48 kHz 输出率）\n- 判定：{}\n\n",
        if all_pass { "PASS" } else { "FAIL" }
    ));
    s.push_str("## 证据等级与残余假设\n\n");
    s.push_str("- 本报告全部证据等级为 `Tested`（单元/集成测试与端到端回放）与 `TypeChecked`（Rust 类型系统），无 `Proved` 项；\n");
    s.push_str("- KWS/VAD/ASR/TTS 为 **mock/echo 档**（echo/sine 纪律）；真 sherpa-onnx 适配器接同一 trait，接口见 docs/dev/PROTOTYPE.md §真实引擎接入；\n");
    s.push_str("- 板上实测数据（RTF/内存/重采样选型）引自 docs/dev/EVIDENCE.md，为旧板读数，**不是本机数据**；\n");
    s.push_str("- 开门四道闸的 PAD 防伪默认 warn 档（未标定不作开锁依据——旧板安全标定未完成）；\n");
    s.push_str(
        "- 「一键执行」脚本见 agent/run-prototype.sh；cargo 测试证据由该脚本先行产出并合并。\n",
    );
    s
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

// ---------------- replay / say / serve ----------------

fn cmd_replay(wav: String, fixtures_dir: PathBuf) -> i32 {
    let bytes = match std::fs::read(&wav) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读 {wav} 失败: {e}");
            return 1;
        }
    };
    let (samples, rate) = match read_wav_mono_pcm16(&bytes) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("解析 {wav} 失败: {e}");
            return 1;
        }
    };
    // 优先用夹具 sidecar 的转写（同哈希）；否则 echo 档
    let sidecar = load_sidecar_entries(&fixtures_dir);
    let at16k = if rate == RATE_INFERENCE {
        samples.clone()
    } else {
        dsp::decimate3_mavg3(&samples)
    };
    let text = sidecar
        .and_then(|m| m.get(&hash_key(&at16k)).cloned())
        .unwrap_or_else(|| "（echo 占位转写）".into());

    let speech_dir = std::env::current_dir()
        .unwrap()
        .join("reports")
        .join("speech");
    let sys = System::assemble(SystemConfig {
        speech_dir: Some(speech_dir.clone()),
        faults: normos_device::mock::MockFaults {
            face_identities: Some(vec!["张三".into()]),
            pad_status: "model_live".into(),
            ..Default::default()
        },
        ..Default::default()
    })
    .expect("装配失败");
    let pipeline = VoicePipeline::default_profile(vec![(at16k.clone(), text)], Some(speech_dir));
    let outcome = pipeline.run_turn(&sys, &wav, &samples, rate);
    println!(
        "{}",
        serde_json::to_string_pretty(&outcome).unwrap_or_default()
    );
    0
}

fn load_sidecar_entries(dir: &Path) -> Option<std::collections::BTreeMap<String, String>> {
    let mut entries = std::collections::BTreeMap::new();
    for spec in standard_fixtures() {
        let bytes = std::fs::read(dir.join(format!("{}.wav", spec.name))).ok()?;
        let (samples48, rate) = read_wav_mono_pcm16(&bytes).ok()?;
        if rate != RATE_OUTPUT {
            continue;
        }
        let at16k = dsp::decimate3_mavg3(&samples48);
        let segs = SpeechSegmenter::default().segment(&at16k, RATE_INFERENCE);
        let seg = segs.first()?;
        entries.insert(hash_key(&seg.samples), spec.text);
    }
    Some(entries)
}

fn hash_key(samples: &[i16]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for s in samples {
        let v = (*s as u16).to_le_bytes();
        for b in v {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    format!("{h:016x}")
}

fn cmd_say(text: String) -> i32 {
    if text.is_empty() {
        eprintln!("用法: normos-cli say <text>");
        return 2;
    }
    let speech_dir = std::env::current_dir()
        .unwrap()
        .join("reports")
        .join("speech");
    let sys = System::assemble(SystemConfig {
        speech_dir: Some(speech_dir.clone()),
        ..Default::default()
    })
    .expect("装配失败");
    let report = sys.handle_text(&text);
    println!("回执: {}", report.speech);
    let pipeline = VoicePipeline::echo_profile(Some(speech_dir.clone()));
    let (spoken, file) = {
        let out = pipeline
            .tts
            .lock()
            .unwrap()
            .synthesize(&report.speech)
            .expect("TTS 失败");
        let up = dsp::linear_resample(&out.samples, out.rate, RATE_OUTPUT);
        std::fs::create_dir_all(&speech_dir).ok();
        let path = speech_dir.join("say.wav");
        match std::fs::write(&path, write_wav_mono_pcm16(&up, RATE_OUTPUT)) {
            Ok(()) => (out.text, Some(path.display().to_string())),
            Err(_) => (out.text, None),
        }
    };
    println!("播报: {spoken}");
    if let Some(f) = file {
        println!("落盘: {f}");
    }
    0
}

fn cmd_serve() -> i32 {
    let sys = match System::assemble(SystemConfig {
        data_dir: Some(PathBuf::from("var")),
        speech_dir: Some(PathBuf::from("reports/speech")),
        ..Default::default()
    }) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("装配失败: {e}");
            return 1;
        }
    };
    let console = match ConsoleServer::spawn(sys.clone(), 10_900) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("控制台启动失败: {e}");
            return 1;
        }
    };
    println!("normosd listening on http://{}", console.addr);
    println!(
        "试试: curl -X POST http://{}/handle_text -d '{{\"text\":\"小克，打开客厅的灯\"}}'",
        console.addr
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

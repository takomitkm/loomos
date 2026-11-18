# EVIDENCE — 实测数据台账（旧板读数 + 本仓库验证记录）

> 纪律（architecture/07 §7.8、architecture/10 §10.5）：
> 直接实测 / 源码或文档直证 / 仿真 / 间接估计 / 无效读数，必须分开标注；
> **旧板读数不得当作本仓库本机结论**；本机新增读数只记「宿主 x86 开发机」口径。

## 1. 旧板实测（FMQL30TAI676H「悟净」，aarch64 4×A55，993 MB RAM，2026-10-30）

来源：HA4FPAI 仓库（archive-2026-11-06 / rebuild-uncompleted-2026-11-09 分支的 docs/）。
证据等级：**A = 板上实测**。仅摘取对本仓库设计有约束力的读数。

### 1.1 RTF 表（A）

| 组件 | 模型 | 大小 | RTF |
|---|---|---|---|
| VAD | silero v5 | 2.3 MB | — |
| ASR-A（默认档） | streaming-zipformer-small-bilingual-zh-en int8 | 14 M | **0.40**（实时） |
| ASR-B（可选档） | icefall multi-zh-hans zipformer-large | 163 M | 0.96–1.19（非实时，A55 带不动） |
| KWS | sherpa-onnx-kws-zipformer-zh-en-3M | 4.6 MB | 0.499 |
| TTS（现用） | matcha-icefall-zh-baker + vocos-22khz-univ | 75.6 + 53.9 MB | 热机 1.16，1.29–1.82 |
| TTS（兜底档） | piper zh_CN-xiao_ya-medium | 18.6 MB | 2.90–3.52 |
| 声纹 | campplus-zh-common（192 维） | 28.3 MB | ≈0.24 |
| klar parse | — | 5.4 MB RSS | ~25 ms |
| 人脸 recognize-camera | YuNet+MobileFaceNet NPU | 53 MB RSS | 45–80 ms |

设计约束提取：
- **matcha 声学模型不能当 vocoder 用（segfault），必须外挂 vocos**——写进 TTS 适配器任务。
- 大模型档（163M ASR）在 A55 上非实时 → 适配器必须支持双档切换（`asr.profile`），不删小档。
- piper/VITS 峰值 391–755 MB 在板上 OOM 杀过 12 次 → 内存预算是准入条件，不是参考值。

### 1.2 内存表（A）

| 服务 | RSS |
|---|---|
| 三语音服务合一（lang-server-voice） | 318–328 MB（合并前 383 MB） |
| klar-nlu-engine | 5.4 MB |
| 全板合计 | 471.7 MB / MemAvailable 409 MB（993 MB 总量） |

设计约束提取：合并进程省 65 MB → 新架构执行模型允许 same-address-space 组件部署（Manifest 的 `execution` 字段）。

### 1.3 重采样选型（A，RESAMPLER-CHOICE.md）

| 方向 | 选型 | 依据 |
|---|---|---|
| 升采样 22050→48000 | **linear 插值** | 16.6 ms/2s，比 polyphase 快 107 倍；真实语音频谱相关 0.9997 |
| 降采样 48000→16000 | **decimate3-mavg3** | 3 点平均 3.6–9.9 ms，比 31 抽 sinc 快 11–14 倍；**ASR 识别结果逐字一致**；20 kHz 混叠压到 −12.3 dB |

→ `normos-audio::dsp` 直接实现了这两个算法；测试断言 16 kHz（新 Nyquist）分量被精确消除、
8 kHz 处 −3.5 dB 与文档口径吻合。

### 1.4 平台事实（A/B，详见 platforms/FMQL30TAI676H/07-platform-baseline.md）

- Kernel 5.4.52、PREEMPT_NONE、cgroup controllers 全未编译、Namespace/seccomp/LSM 未启用。
- **实施等级结论**：本机 RLIMIT/taskset/nice = best_effort；cgroup 类限制 = unsupported。
  → 新架构如实上报（`EnforcementLevel`），不冒充 enforced。
- NPU/互连挂死需整板断电；`Device::reset(level)` 反汇编行为≠厂商语义，不作为恢复路径。
  → Supervisor L3 设计与「reset 语义未确认不自动 reset」策略的依据。
- DMA addr==phys 仅本机观测；SMMU 无 DT 布线 → Buffer typestate 显式 sync 的依据。

## 2. 本仓库验证记录（宿主 x86_64-pc-windows-gnu，rustc 1.99.1）

证据等级：**T = 本机测试自动化产出**（非板上实测）。

| 项 | 读数 | 等级 | 来源 |
|---|---|---|---|
| cargo test --workspace | 135 项全绿（runtime 37 / vision 32 / audio 15 / home 13 / inference 12 / nlu 11 / executor 8 / normosd 4 / device 5） | T | `cargo test` |
| smoke 场景 | 12/12 PASS（唤醒门控/四道闸/灯控/录制/状态/时间/对话/否定/未识别）；其中人脸识别经自研视频路流水线（Pipeline 模式） | T | `normos-cli smoke` |
| 一键脚本 | `agent/run-prototype.sh` 退出码 0 | T | run-prototype.sh |
| clippy | `--workspace --all-targets` 0 warning | T | CI 门禁 |
| 依赖树 | 纯 Rust（serde/thiserror/tracing/toml），无 C 编译器依赖 | T | `cargo tree` |
| e2e 各级耗时 | 见 `reports/prototype-report.md`（mock 档，仅供回归基线，**不构成性能结论**） | T | smoke 输出 |

**未验证（Unverified，诚实清单）**：
- aarch64 交叉构建在本机未执行（CI 中 `cargo check --target aarch64-unknown-linux-gnu`，宿主上已通过）；
- normos-vision 的真引擎（YuNet/MobileFaceNet/MiniFASNet）未接入（mock 档；契约与接入点见 PROTOTYPE.md §2.5）；
- sherpa-onnx / klar / 厂商 NPU 适配器未实现（任务模板已备）；
- 板上 RSS/延迟/稳定性未测（L2/L3 板测需人工窗口）。

## 3. 更新纪律

新增读数必须记录：日期、环境（板/宿主）、命令、原始输出位置、负载状态、证据等级。
升级依赖或换工具链后，受影响的 §2 条目必须重跑。

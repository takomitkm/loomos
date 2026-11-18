# PROTOTYPE — loomos 原型：范围、取舍与真实引擎接入

> 本文回答三个问题：原型做什么、相对旧系统（HA4FPAI 全部分支）删了什么留了什么、
> 真实引擎/真机怎么接。架构依据见 `architecture/`；板级事实见 `platforms/`。

## 1. 原型范围（同一用途，新架构）

端到端链路（与旧板同构，端口/协议只在适配层对应）：

```text
音频源(48k 夹具/未来 mic) → decimate3-mavg3 → 16k
  → [能量门 VAD 切段] → ASR（sidecar mock / sherpa 档）→ 文本
  → KWS 唤醒门（「小克」，未唤醒直接短路——旧链 kws gate 语义）
  → 剥唤醒词 → 双通道执行器：
      通道 B：直连整句表（确定性，~0 开销；只整句匹配，防误触发）
      通道 A：中文规则语法（实体/区域解析）/ 可切 klar 外部引擎
  → 动作注册表 → DeviceRegistry（Capability 校验 → Operation 状态机 → mock/HTTP 提供者）
  → 回执文本 → TTS（sine mock / matcha 档）→ linear 升采样 48k → WAV 落盘（无声卡纪律）
```

运行时底座（旧系统没有、新架构补齐的「OS 层」）：

| 设施 | 说明 |
|---|---|
| 租约管理 | Resource Kind 固定集合；独占冲突拒绝；Releasing 可观测确认点；持有者死亡→Orphaned→清扫 |
| Capability | 权限按主体聚合、子集委托、generation 撤销、审计日志；Admin 不发给业务组件 |
| 配置服务 | Namespace 规范化/Schema/Revision 冲突/Outbox（提交后发布）/Desired-Applied-Observed/Secret 脱敏 |
| 组件生命周期 | Manifest 声明 → 装配检查（依赖/能力）→ Registered→…→Active 状态机 |
| Supervisor | 四类故障（进程崩溃/请求超时/设备挂死/硬件互连）×三级恢复（L1 重启/L2 reset/L3 人工断电）；重启风暴熔断；reset 语义未确认不作为恢复路径 |
| 事件 | 全局 seq + JSONL 持久化 + 游标读取（/events?after=） |
| 推理编排 | 单 Owner、排队/准入/超时；Buffer typestate（CpuOwned→SyncForDevice→DeviceOwned→DeviceComplete→SyncForCpu）；超时请求的 Buffer 进隔离区，人工确认后才可释放 |

## 2. 与旧系统（HA4FPAI 各分支）的功能对照

### 2.1 保留（同用途，重实现）

| 旧功能 | 旧位置 | 新位置 | 说明 |
|---|---|---|---|
| 双通道执行器（直连表 + klar） | `klar/klar_executor.py` | `loomos-nlu` + `loomos-executor` | 直连表全量移植（含「裸重启不进表」「只整句匹配」纪律） |
| 板端 HTTP 设备 API（8091/8092/8093） | `boards/` + WCIS 系服务 | `loomos-device::HttpBoardAdapter` + mock 提供者 | 契约级复刻：503=video_no_frame、mode 只收 off/auto/manual、回读校验 |
| 四道闸开门 | `boards/door_unlock.py` | `loomos-home::door` | 口令 10 句双向包含 → 人脸 → PAD（warn/enforce 双档）→ 声纹；未标定提醒不阻断 |
| 声纹录入/校验 | `boards/voiceprint.py` | `loomos-home::voiceprint` | 流程/存储格式/阈值 0.5 同构；嵌入为 mock 滤波器组（真 campplus 走适配器） |
| 实体注册表 | `klar/make_registry.py` | `loomos-nlu::registry` | HA `.storage` 模拟弃用，直接结构化 JSON；映射质量 `Partial`（如实标注） |
| 统一控制台 | `boards/console.py`(10900) | `loomosd::console` | /status /entities /events /handle_text /action /config |
| 配置期望/生效分离 | `control/settings.py` | `loomos-runtime::config` | Desired/Applied/Observed + 未消费永不标 applied |
| 危险动作确认 | `control/commands.py` | `loomos-home::confirm` | 一次性确认码：绑定动作+参数摘要+主体+TTL，用过即废 |
| 事件游标读取 | `control/events.py` | `loomos-runtime::event` | producer_id + 全局 seq + read_after |
| 重采样选型 | `docs/RESAMPLER-CHOICE.md` | `loomos-audio::dsp` | linear 升采样 / decimate3-mavg3 降采样（实测依据，见 EVIDENCE.md） |
| 播报落盘 | `speak.py` → `/mnt/data/klar/speech/*.wav` | `loomosd::voice` → `reports/speech/*.wav` | 板上无声卡 → WAV 落盘纪律不变 |
| TTS 切句 | `lang/split/tts_split.py` | `loomos-audio::tts::split_for_tts` | max_chars=20 严格不丢字口径 |

### 2.2 删减（旧有但原型不做，理由如下）

| 删减项 | 理由 |
|---|---|
| Wyoming 协议面（10300/10301/10302） | 协议绑定 HA 生态；新架构设备/语音走 trait + 适配器，接 HA 时再补 Wyoming Personality |
| klar 前端 UI / langpacks（zh 词包 Rust 源码） | 新架构 NLU 默认内建规则语法；klar 作为外部引擎档位接 `/api/v2/parse`，不改其源码 |
| LLM 档（openai/npuserv）+ 多轮 history_turns | 旧主线本来就是确定性 NLU；LLM 是 lang/server 实验档，进 `ExternalNlu` 扩展位，不进原型主线 |
| control/ 的 SQLite 业务库（persons/samples/media/receipts…607 行迁移） | 原型不承载人员/声纹样本的持久业务库；Capability/Config 已覆盖系统语义；需要时按 outbox/幂等键契约重建 |
| 定时任务/scheduler/自动化场景 | 旧系统也未完成（klar 只输出 HassStartTimer 无人执行）；Operation 框架已留好位置 |
| 自动化/场景/实体历史 | 同上，旧系统明确未做 |
| 多用户鉴权/会话表 | 家庭内网单 principal；Capability 体系是将来接鉴权的挂点 |
| HA 裁剪（ha-small 分支 5701 文件） | 「板上装 HA」路线已证实走不通（py3.8/993MB），整线退役 |

### 2.3 语义红线（从旧系统实测教训直接继承）

1. 直连表**只整句匹配**：「不要开始录制」「怎么开始录制」不得命中（有测试钉死）。
2. 8093 录制 mode 只接受 off/auto/manual；「on」不存在（板端 400 契约复刻）。
3. 503 → `video_no_frame`；stale 帧不得用于授权。
4. PAD `security_calibrated=false` 时 warn 档播提醒、enforce 档必须 `model_live`。
5. 裸「重启」不进直连表；重启必须过一次性确认（旧板留 1 分钟撤销窗语义已记录）。
6. 超时 ≠ 设备停止：Buffer 隔离区设计直接对应 NPU 挂死教训（整板断电级别故障不自动恢复）。

## 2.5 视频路自研（重做自 fmql30tai-sdi-npu-face-recognition）

视频路不再只是「指向旧板 HTTP 服务的适配器」——`loomos-vision` 按原仓库
（YuNet 检测 + MobileFaceNet 特征 + 双 MiniFASNet PAD + 录制/直播）的用途契约
**自研重做**，且整体可拆卸：

```text
FrameSource（板端 SDI 采集 / 合成帧源）
  → FaceDetector（YuNet 语义：框 + 5 关键点 + 分数）
  → FaceEmbedder（MobileFaceNet 语义：L2 归一化向量）
  → FaceGallery（余弦匹配，阈值 0.40 口径，模板显式 update 不自动合并）
  → PadEngine（live_score → not_applicable/model_live/model_non_live/
               inconclusive/stale/not_evaluated 六态；security_calibrated
               恒如实上报；未标定不得作开锁依据）
  → Recognition（8091 JSON 契约同形，带 frame_id/session 审计）
RecorderCore（off/auto/manual；auto=有人脸才录+5s tail；purge 必须带 id）
ViewSession（ready/session 语义，manifest 只构造地址不代理分片）
```

继承的旧板纪律：帧身份五元组（session/frame_id/token/captured_at）、
帧过期（stale 不得用于授权，PAD max_age=3s）、MAX_RECOGNIZED_FACES=4
（超出 → not_evaluated/face_limit）、mode 只收 off/auto/manual。

**可拆卸开关**（`SystemConfig.vision: VisionMode`）：
- `Detached`——整条视频路拆掉，相机/录制/画面设备不注册（有拆卸验收测试）；
- `MockDevices`——改造前行为（faults 驱动）；
- `Pipeline`——自研视频路流水线（mock 引擎在真实像素上运行，smoke 默认用它）；
- `HttpBoard`——对接旧板 8091/8092/8093（契约测试已覆盖）。

换任何一级引擎 = 实现对应 trait（`FaceDetector`/`FaceEmbedder`/`PadEngine`/
`FrameSource`/`RecorderBackend`/`StreamEncoder`），真 YuNet/MobileFaceNet/
MiniFASNet 经 `loomos-inference::Backend` 单 Owner 编排接入（模板见
agent/tasks/add-real-engine.md §视频引擎）。

## 3. 真实引擎/真机接入

| 目标 | 接入点 | 关键约束 |
|---|---|---|
| sherpa-onnx KWS/VAD/ASR/TTS | `KwsBackend`/`VadBackend`/`AsrBackend`/`TtsBackend` 档位 | 唯一 onnxruntime；matcha 必须外挂 vocos；模型不进仓库 |
| klar NLU 引擎 | `ExternalNlu`（HTTP /api/v2/parse） | decision.type=execute/clarify/confirm/reject 契约归一化 |
| 厂商 NPU Runtime | `loomos_inference::Backend` + FFI wrapper | C ABI 边界 catch 全部异常；同步阻塞无 cancel；Buffer 走 typestate+隔离区 |
| 板端真实服务 | `HttpBoardAdapter`（已实现契约） | origin/鉴权不伪造；503→no frame |
| FMQL30TAI676H 实机 | aarch64 交叉构建 + 上板 smoke | 依赖按 Host→Cross→Hardware 准入；板测数据回填 platforms/ |

任务模板见 `agent/tasks/add-real-engine.md`。

## 4. 验收与证据

- 一键验收：`bash agent/run-prototype.sh`（test 全绿 + smoke 12/12 + 报告落盘）。
- 报告位置：`reports/prototype-report.md`（场景表/耗时/证据分级/残余假设）。
- 证据等级：本仓库全部为 `Tested`/`TypeChecked`；板上读数引自 EVIDENCE.md 并标注来源。
- 残余假设（当前）：KWS/VAD/ASR/TTS/声纹嵌入/人脸/NPU 均为 mock 档；
  直连表覆盖旧板词表的子集（标准场景 12 条）；中文语法覆盖开/关/状态/时间等主干句式。

# Agent Task: 接入真实引擎适配器（sherpa-onnx / klar / 厂商 NPU）

## Objective

在**不破坏默认档全自检**的前提下，为语音链/推理链接入一个真实引擎，
作为同一 trait 的另一档位。

## Rules

1. 默认档（echo/sine/mock）保持空机可跑全链；真实档位用 cargo feature 或
   配置档位切换（旧 lang/config 的 chain 纪律）。
2. 适配器必须实现既有 trait，不改 trait 签名：
   - ASR → `loomos_audio::AsrBackend`（真 sherpa：16k/80 维 fbank/greedy）
   - TTS → `loomos_audio::TtsBackend`（matcha 必须**外挂 vocos vocoder**——旧板实测
     matcha 声学模型直接当 vocoder 用会 segfault；输出 22050 → 管线升采样 48k）
   - KWS → `loomos_audio::KwsBackend`（keywords_threshold=0.25 口径）
   - VAD → `loomos_audio::VadBackend`（silero v5，threshold 0.5）
   - 推理 → `loomos_inference::Backend`（同步阻塞、无 cancel 建模，05 §5.4）
   - NLU → `loomos_nlu::ExternalNlu`（klar：POST /api/v2/parse，decision/candidates 契约）
3. **唯一 onnxruntime 纪律**（旧板 env.single_runtime）：同一进程内不得出现第二个
   ONNX runtime。
4. 模型/密钥不进仓库；密钥走环境变量（D-27 纪律）。
5. 依赖准入：纯 Rust 优先；sherpa-onnx 绑定属于原生 C 依赖，必须按
   docs/dev/architecture/08 §8.2 流程记录（许可证/交叉编译/目标板 RSS）后才可进
   Cargo.toml；进不了就做**外部进程适配器**（HTTP/Wyoming），绝不编译失败留坑。
6. 验收：echo 档 smoke 12/12 保持全绿 + 新档位的独立集成测试（真模型文件缺失时
   `Assumed`/skip，不 fail）。

## 视频引擎（loomos-vision）

视频路引擎同样全 trait 化（契约来源：WCIS-Team/fmql30tai-sdi-npu-face-recognition）：

| 引擎 | trait | 关键契约 |
|---|---|---|
| 人脸检测 | `FaceDetector` | YuNet：框+5 关键点；真适配器需实现 stride 8/16/32 解码（score=√(cls·obj)≥0.6）、NMS 0.6/0.3 |
| 身份特征 | `FaceEmbedder` | MobileFaceNet：**512 维** L2 归一化（mock 档 32 维仅示意） |
| PAD 防伪 | `PadEngine` | MiniFASNetV2+V1SE 双模型 80×80 softmax 均值 → live_score；六态值域；`security_calibrated` 恒如实 |
| 帧源 | `FrameSource` | 板端 SDI 采集（960×540 RGB565，pitch 8 字节对齐）；帧身份五元组 |
| 录制后端 | `RecorderBackend` | fMP4/MP4 分段（60s/256MB）、预缓冲 2s、回收站、purge 需 confirm==id |
| 直播编码 | `StreamEncoder` | VPU h264_v4l2m2m（540p 2Mbps CBR，GOP=1s）；ready=帧龄≤2.5s |

真 NPU 后端一律经 `loomos_inference::Backend`（单 Owner 编排、同步阻塞契约）；
模型包版本指纹（pipeline_id）与 gallery 隔离（models.json sha256 前缀）需在适配器内校验。

## Deliverables

- 适配器 crate/模块 + feature 门控
- 依赖准入记录（进 docs/dev/engineering/dependency-log.md）
- 板测读数（RTF/RSS，若在目标板验证过；否则标注 Unverified）

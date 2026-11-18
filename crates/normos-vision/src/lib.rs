//! normos 视频路（重做自 WCIS-Team/fmql30tai-sdi-npu-face-recognition 的用途契约）。
//!
//! 流水线（与旧板同构，引擎全部 trait 化、默认档全 mock 全自检）：
//! ```text
//! FrameSource（板端 SDI 采集 / 合成帧）
//!   → FaceDetector（YuNet：框 + 5 关键点）
//!   → FaceEmbedder（MobileFaceNet：128 维特征）
//!   → FaceGallery（余弦匹配 ≥ threshold）
//!   → PadEngine（MiniFASNet 双模型：live_score → 状态值域）
//!   → RecognizeResult（8091 JSON 契约同形）
//! RecorderCore（off/auto/manual，auto=有人脸才录）
//! ViewSession（直播 ready/session）
//! ```
//!
//! **可拆卸**（architecture/06：Device Core 不做万能 API；本 crate 整体是一个可换装模块）：
//! - 每个引擎是独立 trait；mock 档默认装，真引擎（YuNet/MobileFaceNet/MiniFASNet 走
//!   `normos-inference::Backend` 单 Owner 编排）按 `agent/tasks/add-real-engine.md` 接入；
//! - `VisionProvider` 是标准 DeviceProvider，装配时挂上/摘下都不影响其它组件；
//! - 帧过期（stale）语义沿用旧板：过期帧不得用于授权。

pub mod embed;
pub mod frame;
pub mod gallery;
pub mod pad;
pub mod pipeline;
pub mod provider;
pub mod recorder;
pub mod view;

pub use embed::{
    Embedding, FaceDetector, FaceEmbedder, MockDetector as _MockDetectorAlias, MockEmbedder,
    MockEmbedderError,
};
pub use frame::{Format, Frame, FrameId, FrameSource, MockFrameSource, SyntheticScene};
pub use gallery::{FaceGallery, GalleryError};
pub use pad::{
    DEFAULT_PAD_LIVE_THRESHOLD, MockPad, PadEngine, PadStatus, PresentationAttackDetection,
};
pub use pipeline::{MockDetector, PipelineError, Recognition, VisionPipeline, VisionStages};
pub use provider::{VisionConfig, VisionFaults, VisionMode, VisionProvider};
pub use recorder::{RecorderBackend, RecorderCore, RecorderMode, RecorderState, Segment};
pub use view::{StreamEncoder, ViewSession, ViewStats};

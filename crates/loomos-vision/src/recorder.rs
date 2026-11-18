//! 录制核心（旧板 wcis-face-recording / 8093 契约）：
//! - mode 状态机：off / auto / manual（**"on" 不存在**——板端 400 契约）；
//! - auto 模式：有人脸才录（旧板"开始自动录制=有人时再录"语义）；
//! - 分段 + 回收站语义；purge **必须逐条 id**（`{}` → 400，防误删）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecorderMode {
    Off,
    Auto,
    Manual,
}

impl RecorderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RecorderMode::Off => "off",
            RecorderMode::Auto => "auto",
            RecorderMode::Manual => "manual",
        }
    }

    /// 板端契约：mode 只接受 off/auto/manual；其余（如 "on"）拒绝。
    pub fn parse(s: &str) -> Result<Self, RecorderError> {
        match s {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            other => Err(RecorderError::InvalidMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecorderState {
    Idle,
    Recording,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: String,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// true = 在回收站；false = 正常留存。
    pub trashed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecorderStats {
    pub mode: RecorderMode,
    pub state: RecorderState,
    pub queue: usize,
    pub trash_count: usize,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum RecorderError {
    #[error("Mode must be off, auto or manual（板端 400 契约，收到 {0:?}）")]
    InvalidMode(String),
    #[error("purge 必须带具体 id（板端契约：{{}} → 400，防误删）")]
    PurgeRequiresId,
    #[error("分段 {0} 不存在")]
    NoSuchSegment(String),
    #[error("录制未在运行（mode={0:?}）")]
    NotRunning(RecorderMode),
}

/// 录制后端 trait（可拆卸：mock 存元数据；真实 MP4/存储后端接这里）。
pub trait RecorderBackend: Send {
    fn name(&self) -> &str;
    /// 开始一个分段；返回分段 id。
    fn start_segment(&mut self, started_ms: u64) -> String;
    fn end_segment(&mut self, id: &str, ended_ms: u64);
    /// 移入回收站（恢复/删除语义由后端自定）。
    fn trash(&mut self, id: &str);
    /// 只读快照（stats 用）。
    fn segments(&self) -> Vec<Segment>;
}

/// mock 后端：分段元数据留存内存。
#[derive(Debug, Default)]
pub struct MockRecorderBackend {
    pub segments: Vec<Segment>,
    next_id: u64,
}

impl RecorderBackend for MockRecorderBackend {
    fn name(&self) -> &str {
        "mock-segments"
    }

    fn start_segment(&mut self, started_ms: u64) -> String {
        self.next_id += 1;
        let id = format!("seg-{:04}", self.next_id);
        self.segments.push(Segment {
            id: id.clone(),
            started_ms,
            ended_ms: None,
            trashed: false,
        });
        id
    }

    fn end_segment(&mut self, id: &str, ended_ms: u64) {
        if let Some(s) = self.segments.iter_mut().find(|s| s.id == id) {
            s.ended_ms = Some(ended_ms);
        }
    }

    fn trash(&mut self, id: &str) {
        if let Some(s) = self.segments.iter_mut().find(|s| s.id == id) {
            s.trashed = true;
        }
    }

    fn segments(&self) -> Vec<Segment> {
        self.segments.clone()
    }
}

/// 录制核心状态机。
pub struct RecorderCore {
    pub mode: RecorderMode,
    pub state: RecorderState,
    pub backend: Box<dyn RecorderBackend>,
    /// auto 模式当前分段（人脸离开后 tail 收尾，旧板 5s）。
    auto_segment: Option<String>,
    /// 人脸最后出现时间（tail 计时起点）。
    last_face_ms: Option<u64>,
    /// auto 模式人脸消失后的收尾时长（ms，旧板 5s）。
    pub tail_ms: u64,
    /// manual 模式分段（模式退出时收尾）。
    manual_segment: Option<String>,
    /// 待写盘队列深度（mock 恒 0；真实后端自报）。
    queue: usize,
    clock_ms: fn() -> u64,
}

impl RecorderCore {
    pub fn new(backend: Box<dyn RecorderBackend>, clock_ms: fn() -> u64) -> Self {
        Self {
            mode: RecorderMode::Off,
            state: RecorderState::Idle,
            backend,
            auto_segment: None,
            last_face_ms: None,
            tail_ms: 5_000,
            manual_segment: None,
            queue: 0,
            clock_ms,
        }
    }

    /// 设置模式（板端契约：回读校验——返回实际生效的 mode）。
    pub fn set_mode(&mut self, mode: RecorderMode) -> Result<RecorderMode, RecorderError> {
        let now = (self.clock_ms)();
        // 从 auto 切走时收尾在录分段
        if self.mode == RecorderMode::Auto
            && let Some(id) = self.auto_segment.take()
        {
            self.backend.end_segment(&id, now);
        }
        // manual 分段生命周期 = manual 模式持续期
        if self.mode == RecorderMode::Manual
            && mode != RecorderMode::Manual
            && let Some(id) = self.manual_segment.take()
        {
            self.backend.end_segment(&id, now);
        }
        if mode == RecorderMode::Manual && self.manual_segment.is_none() {
            self.manual_segment = Some(self.backend.start_segment(now));
        }
        self.mode = mode;
        self.state = match mode {
            RecorderMode::Off => RecorderState::Idle,
            RecorderMode::Manual => RecorderState::Recording,
            RecorderMode::Auto => RecorderState::Idle, // 等第一帧人脸事件
        };
        tracing::info!(mode=?self.mode, state=?self.state, "recorder mode set");
        Ok(self.mode)
    }

    /// 每帧驱动一次：auto 模式按「有人脸才录」推进。
    /// `face_present` 来自视频路流水线最新一帧的检测结果。
    pub fn tick(&mut self, face_present: bool) {
        let now = (self.clock_ms)();
        match self.mode {
            RecorderMode::Off => self.state = RecorderState::Idle,
            RecorderMode::Manual => self.state = RecorderState::Recording,
            RecorderMode::Auto => {
                if face_present {
                    self.last_face_ms = Some(now);
                    if self.auto_segment.is_none() {
                        let id = self.backend.start_segment(now);
                        self.auto_segment = Some(id);
                    }
                    self.state = RecorderState::Recording;
                } else if self.auto_segment.is_some() {
                    // tail：人脸消失后继续录 tail_ms 再收尾（旧板 5s）
                    let last = self.last_face_ms.unwrap_or(now);
                    if now.saturating_sub(last) >= self.tail_ms {
                        if let Some(id) = self.auto_segment.take() {
                            self.backend.end_segment(&id, now);
                        }
                        self.state = RecorderState::Idle;
                    } else {
                        self.state = RecorderState::Recording;
                    }
                } else {
                    self.state = RecorderState::Idle;
                }
            }
        }
    }

    pub fn stats(&self) -> RecorderStats {
        let segments = self.backend.segments();
        RecorderStats {
            mode: self.mode,
            state: self.state,
            queue: self.queue,
            trash_count: segments.iter().filter(|s| s.trashed).count(),
        }
    }

    pub fn segments(&self) -> Vec<Segment> {
        self.backend.segments()
    }

    /// 把分段移入回收站。
    pub fn trash(&mut self, id: &str) -> Result<(), RecorderError> {
        if self.backend.segments().iter().all(|s| s.id != id) {
            return Err(RecorderError::NoSuchSegment(id.to_string()));
        }
        self.backend.trash(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> u64 {
        1_000_000
    }

    fn core() -> RecorderCore {
        RecorderCore::new(Box::new(MockRecorderBackend::default()), clock)
    }

    #[test]
    fn mode_contract_rejects_on_like_board() {
        assert!(matches!(
            RecorderMode::parse("on"),
            Err(RecorderError::InvalidMode(m)) if m == "on"
        ));
        assert_eq!(RecorderMode::parse("auto").unwrap(), RecorderMode::Auto);
    }

    #[test]
    fn manual_records_continuously() {
        let mut r = core();
        r.set_mode(RecorderMode::Manual).unwrap();
        r.tick(false); // manual 不看人脸
        assert_eq!(r.state, RecorderState::Recording);
    }

    #[test]
    fn auto_records_only_with_face() {
        // 可推进时钟（验证 tail 收尾契约；fn 指针 → 静态原子量）
        static NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000_000);
        fn clock() -> u64 {
            NOW.load(std::sync::atomic::Ordering::Relaxed)
        }
        let mut r = RecorderCore::new(Box::new(MockRecorderBackend::default()), clock);
        r.set_mode(RecorderMode::Auto).unwrap();
        r.tick(false);
        assert_eq!(r.state, RecorderState::Idle);
        r.tick(true);
        assert_eq!(r.state, RecorderState::Recording);
        r.tick(true);
        assert_eq!(r.state, RecorderState::Recording);
        // 人脸消失 → tail 期内仍 Recording（旧板 5s）
        NOW.store(1_002_000, std::sync::atomic::Ordering::Relaxed);
        r.tick(false);
        assert_eq!(r.state, RecorderState::Recording);
        // 超过 tail_ms → 收尾
        NOW.store(1_008_000, std::sync::atomic::Ordering::Relaxed);
        r.tick(false);
        assert_eq!(r.state, RecorderState::Idle);
        // 恰好一个分段（进出一次）
        let segs = r.segments();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].ended_ms.is_some());
    }

    #[test]
    fn switching_mode_from_auto_ends_segment() {
        let mut r = core();
        r.set_mode(RecorderMode::Auto).unwrap();
        r.tick(true);
        r.set_mode(RecorderMode::Off).unwrap();
        let segs = r.segments();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].ended_ms.is_some(), "切走 auto 应收尾分段");
        assert_eq!(r.state, RecorderState::Idle);
    }

    #[test]
    fn trash_counted_in_stats() {
        let mut r = core();
        r.set_mode(RecorderMode::Manual).unwrap();
        r.tick(true);
        r.set_mode(RecorderMode::Off).unwrap();
        let id = r.segments()[0].id.clone();
        r.trash(&id).unwrap();
        assert_eq!(r.stats().trash_count, 1);
        assert!(matches!(
            r.trash("ghost"),
            Err(RecorderError::NoSuchSegment(_))
        ));
    }
}

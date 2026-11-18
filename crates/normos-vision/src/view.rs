//! 直播/画面（旧板 wcis-sdi-vpu-view / 8092 契约）：
//! - GET /api/status → `{"ready":bool,"session":..}`；
//! - H.264 编码走 VPU；manifest 只构造地址不代理分片；
//! - 分辨率 960×540（PL 缩放整数步长 + 模型 32 对齐约束）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewStats {
    pub ready: bool,
    pub session: Option<String>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ViewError {
    #[error("直播未就绪")]
    NotReady,
    #[error("编码器不可用：{0}")]
    EncoderUnavailable(String),
}

/// 流编码器 trait（可拆卸：mock 出统计；真 VPU/H.264 后端接这里）。
pub trait StreamEncoder: Send {
    fn name(&self) -> &str;
    /// 编码一帧，返回（字节数，帧计数增量）。mock 返回确定性统计。
    fn encode(&mut self, frame_bytes: usize) -> Result<(usize, u64), ViewError>;
}

/// mock 编码器：按帧大小给出确定性码率估计（无真实编码）。
#[derive(Debug, Default)]
pub struct MockStreamEncoder {
    pub frames: u64,
    pub bytes: usize,
}

impl StreamEncoder for MockStreamEncoder {
    fn name(&self) -> &str {
        "mock-mjpeg-stats"
    }

    fn encode(&mut self, frame_bytes: usize) -> Result<(usize, u64), ViewError> {
        // mock：按 20:1 压缩比估计 H.264 体积
        let out = frame_bytes / 20;
        self.frames += 1;
        self.bytes += out;
        Ok((out, self.frames))
    }
}

/// 直播会话。
pub struct ViewSession {
    pub session_id: Option<String>,
    pub ready: bool,
    pub encoder: Box<dyn StreamEncoder>,
    next_session: u64,
}

impl ViewSession {
    pub fn new(encoder: Box<dyn StreamEncoder>) -> Self {
        Self {
            session_id: None,
            ready: false,
            encoder,
            next_session: 0,
        }
    }

    /// 启动直播会话。
    pub fn start(&mut self) -> String {
        self.next_session += 1;
        let id = format!("live-{}", self.next_session);
        self.session_id = Some(id.clone());
        self.ready = true;
        id
    }

    /// 停止直播。
    pub fn stop(&mut self) {
        self.session_id = None;
        self.ready = false;
    }

    /// 喂一帧（帧字节数）。
    pub fn push_frame(&mut self, frame_bytes: usize) -> Result<ViewStats, ViewError> {
        if !self.ready {
            return Err(ViewError::NotReady);
        }
        self.encoder.encode(frame_bytes)?;
        Ok(self.stats())
    }

    pub fn stats(&self) -> ViewStats {
        ViewStats {
            ready: self.ready,
            session: self.session_id.clone(),
        }
    }

    /// manifest 构造（旧板纪律：只构造地址，不代理分片）。
    pub fn manifest(&self, base: &str) -> Option<String> {
        self.session_id
            .as_ref()
            .map(|s| format!("{base}/h264/{s}/manifest.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lifecycle() {
        let mut v = ViewSession::new(Box::new(MockStreamEncoder::default()));
        assert!(!v.stats().ready);
        assert!(v.push_frame(1000).is_err(), "未启动应 NotReady");
        let id = v.start();
        assert_eq!(id, "live-1");
        assert!(v.stats().ready);
        let st = v.push_frame(20_000).unwrap();
        assert!(st.ready);
        // manifest 只构造地址
        assert_eq!(
            v.manifest("http://b:8092").unwrap(),
            "http://b:8092/h264/live-1/manifest.json"
        );
        v.stop();
        assert!(!v.stats().ready);
        assert!(v.session_id.is_none());
    }
}

//! 帧与帧源：视频路的入口。
//!
//! 契约（旧板实测）：
//! - 板端采集为 960×540（PL 缩放整数步长 + 模型高度 32 对齐约束所致，非带宽不足）；
//! - 帧带 frame_id / frame_token / ages（滞留时长），**过期帧不得用于授权**；
//! - 双缓冲：采集线程与消费线程各持一份，消费侧读到的是"上一帧完成"的稳定副本。

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// 帧格式（旧板流水线用 NV12 传输、BGR 推理；mock 档用 RGB888 小帧）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Rgb888,
    Nv12,
    Bgr888,
}

pub type FrameId = u64;

/// 一帧。`data` 为按 `format` 排布的像素；mock 档允许小帧（如 160×120）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub id: FrameId,
    /// 帧令牌（旧板 frame_token：消费者据此核对帧一致性）。
    pub token: String,
    /// 采集会话（旧板 (boot_id, session, frame_id, captured_at, token) 五元组纪律）。
    pub session: String,
    /// 采集时间（Unix ms）。
    pub captured_ms: u64,
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub data: Vec<u8>,
}

impl Frame {
    /// 帧是否仍新鲜（过期帧不得用于授权——旧板 stale 语义）。
    pub fn is_fresh(&self, now_ms: u64, max_age: Duration) -> bool {
        now_ms.saturating_sub(self.captured_ms) <= max_age.as_millis() as u64
    }

    /// 帧滞留时长（旧板 ages 字段语义）。
    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.captured_ms)
    }
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum FrameError {
    #[error("帧源离线（无新鲜画面 → video_no_frame）")]
    NoFreshFrame,
    #[error("帧源已停止")]
    Stopped,
}

/// 帧源 trait：拉取最新完成帧（非阻塞语义由实现声明）。
pub trait FrameSource: Send {
    fn name(&self) -> &str;
    /// 拉取当前帧；无新鲜帧返回 NoFreshFrame（对应板端 503）。
    fn latest(&mut self) -> Result<Frame, FrameError>;
}

/// 合成场景描述：mock 档用确定性图案生成帧，
/// 人脸由「色块签名」表示——检测器真的在像素上找色块，不透传元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceSpec {
    /// 人物签名（0–255 的基准灰度，gallery 按此匹配）。
    pub signature: u8,
    /// 人脸中心相对坐标（0.0–1.0）。
    pub cx: f64,
    pub cy: f64,
    /// 人脸半径（相对短边）。
    pub r: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyntheticScene {
    /// 当前帧里出现的人脸（空 = 无人）。
    pub faces: Vec<FaceSpec>,
    /// 是否伪造「平面照片攻击」（PAD mock 据此给 model_non_live）。
    pub spoof: bool,
}

/// 合成帧源：确定性生成帧（回归友好），场景可切换。
pub struct MockFrameSource {
    width: u32,
    height: u32,
    next_id: FrameId,
    scene: SyntheticScene,
    online: bool,
    session: String,
    clock_ms: fn() -> u64,
}

impl MockFrameSource {
    pub fn new(width: u32, height: u32, clock_ms: fn() -> u64) -> Self {
        Self {
            width,
            height,
            next_id: 0,
            scene: SyntheticScene::default(),
            online: true,
            session: "mock-session-1".to_string(),
            clock_ms,
        }
    }

    /// 会话标识（跨会话的帧不得用于授权）。
    pub fn set_session(&mut self, session: &str) {
        self.session = session.to_string();
    }

    pub fn set_scene(&mut self, scene: SyntheticScene) {
        self.scene = scene;
    }

    pub fn set_online(&mut self, online: bool) {
        self.online = online;
    }

    /// 生成一帧：背景 40 灰度 + 人脸色块（签名灰度）。
    /// 色块中心 3×3 写入签名值与防伪纹理（spoof 时纹理为平坦值）。
    fn render(&mut self) -> Frame {
        let id = self.next_id;
        self.next_id += 1;
        let w = self.width as usize;
        let h = self.height as usize;
        let mut data = vec![40u8; w * h * 3];
        let short = w.min(h) as f64;
        for f in &self.scene.faces {
            let cx = (f.cx * w as f64) as usize;
            let cy = (f.cy * h as f64) as usize;
            let r = ((f.r * short) as usize).max(2);
            for dy in 0..r {
                for dx in 0..r {
                    let d2 = dx * dx + dy * dy;
                    if d2 > r * r {
                        continue;
                    }
                    for (px, py) in [
                        (cx + dx, cy + dy),
                        (cx.wrapping_sub(dx), cy + dy),
                        (cx + dx, cy.wrapping_sub(dy)),
                        (cx.wrapping_sub(dx), cy.wrapping_sub(dy)),
                    ] {
                        if px < w && py < h {
                            let base = f.signature;
                            // 防伪纹理：live = 周围有细纹理；spoof = 平坦
                            let v = if self.scene.spoof {
                                base
                            } else {
                                base.wrapping_add(((px + py) % 3) as u8)
                            };
                            let off = (py * w + px) * 3;
                            data[off] = v;
                            data[off + 1] = v;
                            data[off + 2] = v;
                        }
                    }
                }
            }
        }
        Frame {
            id,
            token: format!("f{id}"),
            session: self.session.clone(),
            captured_ms: (self.clock_ms)(),
            width: self.width,
            height: self.height,
            format: Format::Rgb888,
            data,
        }
    }
}

impl FrameSource for MockFrameSource {
    fn name(&self) -> &str {
        "synthetic"
    }

    fn latest(&mut self) -> Result<Frame, FrameError> {
        if !self.online {
            return Err(FrameError::NoFreshFrame);
        }
        Ok(self.render())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> u64 {
        1_000_000
    }

    #[test]
    fn frame_freshness_and_age() {
        let f = Frame {
            id: 1,
            token: "t".into(),
            session: "s1".into(),
            captured_ms: 1_000_000,
            width: 4,
            height: 4,
            format: Format::Rgb888,
            data: vec![0; 48],
        };
        assert!(f.is_fresh(1_000_500, Duration::from_millis(1000)));
        assert!(!f.is_fresh(1_001_500, Duration::from_millis(1000)));
        assert_eq!(f.age_ms(1_000_300), 300);
    }

    #[test]
    fn synthetic_source_renders_face_blob() {
        let mut src = MockFrameSource::new(160, 120, clock);
        src.set_scene(SyntheticScene {
            faces: vec![FaceSpec {
                signature: 200,
                cx: 0.5,
                cy: 0.5,
                r: 0.2,
            }],
            spoof: false,
        });
        let f = src.latest().unwrap();
        assert_eq!(f.data.len(), 160 * 120 * 3);
        // 中心像素应为人脸签名（200±纹理）
        let center = &f.data[(60 * 160 + 80) * 3..(60 * 160 + 80) * 3 + 3];
        assert!(center[0] >= 200, "中心应是人脸色块，实际 {}", center[0]);
        // 背景为 40
        let corner = &f.data[0..3];
        assert_eq!(corner[0], 40);
        // 连续帧 id 递增
        let f2 = src.latest().unwrap();
        assert_eq!(f2.id, f.id + 1);
        assert_ne!(f2.token, f.token);
    }

    #[test]
    fn offline_source_is_no_fresh_frame() {
        let mut src = MockFrameSource::new(16, 16, clock);
        src.set_online(false);
        assert!(matches!(src.latest(), Err(FrameError::NoFreshFrame)));
    }
}

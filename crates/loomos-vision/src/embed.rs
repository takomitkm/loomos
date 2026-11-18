//! 人脸检测与特征嵌入（引擎 trait + mock 档）。
//!
//! 契约（fmql30tai-sdi-npu-face-recognition README）：
//! - 检测：YuNet，输出人脸框 + 5 点关键点（pipeline=yunet-mf-fp16-npu 的前半）；
//! - 特征：MobileFaceNet（w600k_mbf），输出余弦比较用的身份向量；
//!   mock 档在真实像素上做确定性计算（色块签名提取）；真引擎接
//!   `loomos-inference::Backend`（单 Owner 编排）后替换。

use crate::frame::Frame;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 一个人脸检测结果（YuNet 语义：框 + 5 关键点 + 分数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    /// 框（像素，x0,y0,x1,y1）。
    pub bbox: [u32; 4],
    /// 5 点关键点（眼×2、鼻、嘴角×2），YuNet 输出语义。
    pub landmarks: [[u32; 2]; 5],
    /// 检测分数 0.0–1.0。
    pub score: f64,
}

/// 人脸检测引擎 trait。
pub trait FaceDetector: Send {
    fn name(&self) -> &str;
    fn detect(&self, frame: &Frame) -> Vec<Detection>;
}

/// 在 RGB888 帧上扫描人脸签名色块：找灰度显著高于背景（40）的连通区域。
/// 返回框与区域平均签名（供嵌入器使用）。
pub fn scan_face_blobs(frame: &Frame) -> Vec<(u32, u32, u32, u32, f64)> {
    assert_eq!(
        frame.format,
        crate::frame::Format::Rgb888,
        "mock 检测只支持 RGB888"
    );
    let w = frame.width as usize;
    let h = frame.height as usize;
    let gray = |x: usize, y: usize| -> u32 {
        let off = (y * w + x) * 3;
        frame.data[off] as u32
    };
    // 行扫描：把每行中 >96 灰度的像素聚成段
    let mut boxes: Vec<(i64, i64, i64, i64, u64, usize)> = Vec::new(); // x0,y0,x1,y1,sum,count
    for y in (0..h).step_by(2) {
        let mut x = 0;
        while x < w {
            if gray(x, y) > 96 {
                let x0 = x;
                let mut sum = 0u64;
                let mut cnt = 0usize;
                while x < w && gray(x, y) > 96 {
                    sum += gray(x, y) as u64;
                    cnt += 1;
                    x += 2;
                }
                boxes.push((x0 as i64, y as i64, (x - 2) as i64, y as i64, sum, cnt));
            } else {
                x += 2;
            }
        }
    }
    // 合并纵向相邻段（行距 ≤6）为连通框
    let mut merged: Vec<(i64, i64, i64, i64, u64, usize)> = Vec::new();
    for b in boxes {
        let mut absorbed = false;
        for m in merged.iter_mut() {
            let vertical_overlap = (b.1 - m.3).abs() <= 6;
            let horizontal_overlap = b.0 <= m.2 + 4 && m.0 <= b.2 + 4;
            if vertical_overlap && horizontal_overlap {
                m.0 = m.0.min(b.0);
                m.1 = m.1.min(b.1);
                m.2 = m.2.max(b.2);
                m.3 = m.3.max(b.3);
                m.4 += b.4;
                m.5 += b.5;
                absorbed = true;
                break;
            }
        }
        if !absorbed {
            merged.push(b);
        }
    }
    merged
        .into_iter()
        .filter(|m| (m.2 - m.0) >= 8 && (m.3 - m.1) >= 8) // 噪点过滤
        .map(|m| {
            (
                m.0 as u32,
                m.1 as u32,
                m.2 as u32,
                m.3 as u32,
                (m.4 as f64) / (m.5.max(1) as f64),
            )
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockDetectorError;

/// mock 检测器：真实扫描帧像素，把签名色块输出为 Detection（中心即"鼻点"，
/// 关键点由框按 YuNet 布局近似生成）。确定性、可回归。
#[derive(Debug, Default)]
pub struct MockDetector;

impl FaceDetector for MockDetector {
    fn name(&self) -> &str {
        "mock-yunet"
    }

    fn detect(&self, frame: &Frame) -> Vec<Detection> {
        scan_face_blobs(frame)
            .into_iter()
            .map(|(x0, y0, x1, y1, avg)| {
                let bw = x1 - x0;
                let bh = y1 - y0;
                let cx = x0 + bw / 2;
                let cy = y0 + bh / 2;
                Detection {
                    bbox: [x0, y0, x1, y1],
                    // YuNet 5 点布局近似：两眼、鼻尖、两嘴角
                    landmarks: [
                        [x0 + bw / 5, y0 + bh / 3],
                        [x0 + bw * 4 / 5, y0 + bh / 3],
                        [cx, cy],
                        [x0 + bw / 3, y0 + bh * 3 / 4],
                        [x0 + bw * 2 / 3, y0 + bh * 3 / 4],
                    ],
                    // 分数按色块显著度归一（背景 40，人脸 ~200）
                    score: ((avg - 40.0) / 160.0).clamp(0.0, 1.0),
                }
            })
            .collect()
    }
}

/// 特征嵌入结果（MobileFaceNet 语义：L2 归一化向量）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub dim: usize,
    pub vector: Vec<f64>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum MockEmbedderError {
    #[error("框无效（尺寸为 0）")]
    EmptyBox,
    #[error("帧格式不支持：{0:?}")]
    UnsupportedFormat(crate::frame::Format),
}

/// 人脸特征引擎 trait。
pub trait FaceEmbedder: Send {
    fn name(&self) -> &str;
    fn embed(&self, frame: &Frame, det: &Detection) -> Result<Embedding, MockEmbedderError>;
}

/// mock 嵌入器：对框内像素做「16 段水平均值 v + 16 段均值 v²/255」双通道
/// （二次通道使不同签名灰度产生不同方向——单一线性投影会被 L2 归一化消掉标量差异）。
/// 确定性：同人同框位→同向量，可被 gallery 余弦匹配。
#[derive(Debug, Default)]
pub struct MockEmbedder;

pub const MOCK_EMBED_DIM: usize = 32;

impl FaceEmbedder for MockEmbedder {
    fn name(&self) -> &str {
        "mock-mbf"
    }

    fn embed(&self, frame: &Frame, det: &Detection) -> Result<Embedding, MockEmbedderError> {
        if frame.format != crate::frame::Format::Rgb888 {
            return Err(MockEmbedderError::UnsupportedFormat(frame.format));
        }
        let [x0, y0, x1, y1] = det.bbox;
        if x1 <= x0 || y1 <= y0 {
            return Err(MockEmbedderError::EmptyBox);
        }
        let w = frame.width as usize;
        let x0u = x0 as usize;
        let y0u = y0 as usize;
        let xmax = (x1.min(frame.width - 1)) as usize;
        let ymax = (y1.min(frame.height - 1)) as usize;
        // 只统计 blob 像素（v>96），排除框内背景
        let mut sum = 0f64;
        let mut n = 0u64;
        let mut seg_sum = [0f64; 16];
        let mut seg_cnt = [0u64; 16];
        let bw = (x1 - x0 + 1) as usize;
        for yy in y0u..=ymax {
            for xx in x0u..=xmax {
                let v = frame.data[(yy * w + xx) * 3] as f64;
                if v <= 96.0 {
                    continue;
                }
                sum += v;
                n += 1;
                let k = (((xx - x0u) * 16) / bw.max(1)).min(15);
                seg_sum[k] += v;
                seg_cnt[k] += 1;
            }
        }
        if n == 0 {
            return Err(MockEmbedderError::EmptyBox);
        }
        let mean_v = sum / n as f64;
        // 通道 1（16 维）：平均签名量化桶 one-hot（桶宽 10 灰度，容纹理 ±1）
        // 通道 2（16 维）：水平段形状模式（弱权重，位置/尺寸信息）
        let mut emb = vec![0f64; MOCK_EMBED_DIM];
        let bucket = (((mean_v - 100.0) / 10.0).floor().clamp(0.0, 15.0)) as usize;
        emb[bucket] = 1.0;
        let mut shape_max = 1f64;
        for k in 0..16 {
            if seg_cnt[k] > 0 {
                seg_sum[k] /= seg_cnt[k] as f64;
                shape_max = shape_max.max(seg_sum[k]);
            }
        }
        for k in 0..16 {
            emb[16 + k] = 0.1 * seg_sum[k] / shape_max;
        }
        // L2 归一化
        let norm: f64 = emb.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-12);
        let emb: Vec<f64> = emb.into_iter().map(|x| x / norm).collect();
        Ok(Embedding {
            dim: MOCK_EMBED_DIM,
            vector: emb,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FaceSpec, FrameSource, MockFrameSource, SyntheticScene};

    fn clock() -> u64 {
        1_000_000
    }

    fn frame_with_face(signature: u8, cx: f64, cy: f64) -> Frame {
        let mut src = MockFrameSource::new(160, 120, clock);
        src.set_scene(SyntheticScene {
            faces: vec![FaceSpec {
                signature,
                cx,
                cy,
                r: 0.2,
            }],
            spoof: false,
        });
        src.latest().unwrap()
    }

    #[test]
    fn detector_finds_face_blob() {
        let f = frame_with_face(200, 0.5, 0.5);
        let dets = MockDetector.detect(&f);
        assert_eq!(dets.len(), 1, "应检出 1 张人脸");
        let d = &dets[0];
        assert!(d.score > 0.8, "分数应高，实际 {}", d.score);
        // 框中心应接近 (80, 60)
        let cx = (d.bbox[0] + d.bbox[2]) / 2;
        let cy = (d.bbox[1] + d.bbox[3]) / 2;
        assert!((cx as i64 - 80).abs() <= 12, "cx={cx}");
        assert!((cy as i64 - 60).abs() <= 12, "cy={cy}");
        // 5 关键点
        assert_eq!(d.landmarks.len(), 5);
    }

    #[test]
    fn detector_empty_scene() {
        let mut src = MockFrameSource::new(160, 120, clock);
        src.set_scene(SyntheticScene::default());
        let f = src.latest().unwrap();
        assert!(MockDetector.detect(&f).is_empty());
    }

    #[test]
    fn embedder_same_face_same_vector() {
        let f = frame_with_face(200, 0.5, 0.5);
        let det = &MockDetector.detect(&f)[0];
        let e1 = MockEmbedder.embed(&f, det).unwrap();
        let e2 = MockEmbedder.embed(&f, det).unwrap();
        assert_eq!(e1.vector, e2.vector);
        assert_eq!(e1.dim, MOCK_EMBED_DIM);
        // L2 归一化
        let n: f64 = e1.vector.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((n - 1.0).abs() < 1e-6);
    }

    #[test]
    fn embedder_different_faces_different_vectors() {
        let f1 = frame_with_face(200, 0.5, 0.5);
        let f2 = frame_with_face(120, 0.5, 0.5);
        let d1 = &MockDetector.detect(&f1)[0];
        let d2 = &MockDetector.detect(&f2)[0];
        let e1 = MockEmbedder.embed(&f1, d1).unwrap();
        let e2 = MockEmbedder.embed(&f2, d2).unwrap();
        let dot: f64 = e1.vector.iter().zip(&e2.vector).map(|(a, b)| a * b).sum();
        assert!(dot < 0.95, "不同签名的人余弦应可区分，实际 {dot}");
    }
}

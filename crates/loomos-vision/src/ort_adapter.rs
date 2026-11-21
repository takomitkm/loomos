//! 真引擎适配器（onnxruntime 直连，`ort` feature 门控）。
//!
//! 契约来源（face-video 仓库移植过的实测语义，03/04 号文档 §2 路线）：
//! - **YuNet 检测**（face_detection_yunet_2023mar.onnx）：letterbox（等比缩放贴左上、
//!   黑边）→ BGR NCHW float32 → 12 输出解码（stride 8/16/32，
//!   score=√(cls·obj)≥0.6，NMS IoU 0.3）→ 坐标除回缩放（run_photos.decode 移植）；
//! - **SFace 嵌入**（face_recognition_sface onnx）：5 点相似变换 align 112×112
//!   （ArcFace 模板，与 face-video alignment_matrix 同几何）→ RGB NCHW → L2；
//! - onnxruntime.dll 经 `loomos-ort-sys`（LoadLibrary 动态加载，无 C 链接）。
//!
//! unsafe 全部隔离在 `loomos-ort-sys`；本模块纯 Rust。

use loomos_ort_sys as osys;
use loomos_ort_sys::OrtSysError;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

use crate::embed::FaceDetector;
use crate::embed::{Detection, Embedding};
use crate::frame::Frame;

#[derive(Error, Debug, Clone)]
pub enum OrtVisionError {
    #[error("onnxruntime 不可用: {0}")]
    Unavailable(String),
    #[error("ORT 运行失败: {0}")]
    Runtime(String),
    #[error("解码失败: {0}")]
    Decode(String),
}

impl OrtVisionError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, OrtVisionError::Unavailable(_))
    }
}

/// 真引擎配置（路径驱动）。
#[derive(Debug, Clone)]
pub struct OrtVisionConfig {
    /// onnxruntime.dll 路径。
    pub dll: PathBuf,
    /// face_detection_yunet_2023mar.onnx。
    pub yunet_model: PathBuf,
    /// face_recognition_sface onnx。
    pub sface_model: PathBuf,
    pub num_threads: i32,
}

/// 运行时探测到的模型 I/O 元数据（证据面）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrtModelInfo {
    pub yunet_input_name: String,
    pub yunet_input_shape: Vec<i64>,
    pub yunet_output_count: usize,
    pub sface_output_count: usize,
}

pub struct Sessions {
    ort: osys::Ort,
    /// env 必须活过所有 session
    #[allow(dead_code)]
    env: osys::RawHandle,
    #[allow(dead_code)]
    mem_info: osys::RawHandle,
    yunet: SessionSlot,
    sface: SessionSlot,
}

pub struct SessionSlot {
    session: osys::RawHandle,
    input_name: String,
    output_names: Vec<String>,
    input_shape: Vec<i64>,
}

// ORT CPU session 线程安全：内部指针经 ort-sys 的 RawHandle（Send+Sync 声明于彼）
impl Sessions {
    pub fn load(cfg: &OrtVisionConfig) -> Result<Arc<Sessions>, OrtVisionError> {
        let ort =
            osys::Ort::load(&cfg.dll).map_err(|e| OrtVisionError::Unavailable(e.to_string()))?;
        let env = ort
            .create_env()
            .map_err(|e| OrtVisionError::Unavailable(e.to_string()))?;
        let yunet_session = ort
            .create_session(env, &cfg.yunet_model, cfg.num_threads)
            .map_err(|e| OrtVisionError::Unavailable(format!("yunet session: {e}")))?;
        let sface_session = ort
            .create_session(env, &cfg.sface_model, cfg.num_threads)
            .map_err(|e| OrtVisionError::Unavailable(format!("sface session: {e}")))?;
        let (yin, yout) = ort
            .io_names(yunet_session)
            .map_err(|e| OrtVisionError::Unavailable(format!("yunet io: {e}")))?;
        let (sin, sout) = ort
            .io_names(sface_session)
            .map_err(|e| OrtVisionError::Unavailable(format!("sface io: {e}")))?;
        let (dims, _e) = ort
            .input_shape(yunet_session, 0)
            .map_err(|e| OrtVisionError::Unavailable(format!("yunet shape: {e}")))?;
        let mem_info = ort
            .create_cpu_memory_info()
            .map_err(|e| OrtVisionError::Unavailable(e.to_string()))?;
        tracing::info!(
            yunet_input = ?yin,
            yunet_shape = ?dims,
            yunet_outputs = yout.len(),
            sface_outputs = sout.len(),
            "ort vision sessions loaded"
        );
        Ok(Arc::new(Sessions {
            ort,
            env: osys::RawHandle(env),
            mem_info: osys::RawHandle(mem_info),
            yunet: SessionSlot {
                session: osys::RawHandle(yunet_session),
                input_name: yin.first().cloned().unwrap_or_default(),
                output_names: yout,
                input_shape: dims,
            },
            sface: SessionSlot {
                session: osys::RawHandle(sface_session),
                input_name: sin.first().cloned().unwrap_or_default(),
                output_names: sout,
                input_shape: vec![],
            },
        }))
    }

    fn run(
        &self,
        slot: &SessionSlot,
        input: &mut Vec<f32>,
        shape: &[i64],
    ) -> Result<Vec<Vec<f32>>, OrtVisionError> {
        let mem = self
            .ort
            .create_cpu_memory_info()
            .map_err(|e| OrtVisionError::Runtime(e.to_string()))?;
        let value = osys::RawHandle(
            self.ort
                .create_tensor_f32(mem, input, shape)
                .map_err(|e| OrtVisionError::Runtime(e.to_string()))?,
        );
        let in_namez: Vec<Vec<u8>> = vec![fmt_nul(&slot.input_name)];
        let in_ptrs: Vec<*const u8> = in_namez.iter().map(|s| s.as_ptr()).collect();
        let in_values: Vec<*mut std::ffi::c_void> = vec![value.0];
        let out_namez: Vec<Vec<u8>> = slot.output_names.iter().map(|s| fmt_nul(s)).collect();
        let out_ptrs: Vec<*const u8> = out_namez.iter().map(|s| s.as_ptr()).collect();
        let outputs = self
            .ort
            .run(
                slot.session.0,
                &in_ptrs,
                &in_values,
                &out_ptrs,
                out_namez.len(),
            )
            .map_err(|e| OrtVisionError::Runtime(e.to_string()))?;
        let mut out = Vec::new();
        for v in &outputs {
            let n = self
                .ort
                .output_elements(*v)
                .map_err(|e| OrtVisionError::Runtime(e.to_string()))?;
            out.push(
                self.ort
                    .tensor_data_f32_len(*v, n)
                    .map_err(|e| OrtVisionError::Runtime(e.to_string()))?,
            );
            self.ort.release_value(*v);
        }
        self.ort.release_value(value.0);
        self.ort.release_memory_info(mem);
        Ok(out)
    }
}

/// YuNet 检测器（实现 `FaceDetector`；真引擎档）。
pub struct OrtYuNetDetector {
    inner: Arc<Sessions>,
    conf_threshold: f64,
    nms_iou: f64,
}

/// SFace 嵌入器（实现 `FaceEmbedder`；真引擎档）。
pub struct OrtSFaceEmbedder {
    inner: Arc<Sessions>,
}

impl OrtYuNetDetector {
    pub fn new(inner: Arc<Sessions>) -> Self {
        Self {
            inner,
            conf_threshold: 0.6,
            nms_iou: 0.3,
        }
    }

    /// 检测（带错误；调用方可观测 letterbox/运行失败）。
    pub fn detect_full(&self, frame: &Frame) -> Result<Vec<Detection>, OrtVisionError> {
        let slot = &self.inner.yunet;
        // 目标输入尺寸：模型固定形状（dims[2]=H, dims[3]=W）；动态形状时用 320×320（zoo 默认）
        let dims = &slot.input_shape;
        let (th, tw) = if dims.len() >= 4 && dims[2] > 0 && dims[3] > 0 {
            (dims[2] as u32, dims[3] as u32)
        } else {
            (320, 320)
        };
        let (mut blob, sx, sy) = letterbox_and_blob(frame, tw, th);
        let shape: Vec<i64> = vec![1, 3, th as i64, tw as i64];
        let outputs = self
            .inner
            .run(slot, &mut blob, &shape)
            .map_err(|e| OrtVisionError::Runtime(e.to_string()))?;
        if outputs.len() != 12 {
            return Err(OrtVisionError::Decode(format!(
                "YuNet 需要 12 个输出，实际 {}",
                outputs.len()
            )));
        }
        let mut dets = decode_yunet(&outputs, sx, sy, self.conf_threshold);
        crate::ort_adapter::nms(&mut dets, self.nms_iou);
        Ok(dets)
    }
}

impl FaceDetector for OrtYuNetDetector {
    fn name(&self) -> &str {
        "yunet-2023mar-ort"
    }

    fn detect(&self, frame: &Frame) -> Vec<Detection> {
        self.detect_full(frame).unwrap_or_default()
    }
}

impl OrtSFaceEmbedder {
    pub fn new(inner: Arc<Sessions>) -> Self {
        Self { inner }
    }

    /// 对检测结果的 5 点关键点做 112×112 相似变换对齐后嵌入。
    pub fn embed_detection(
        &self,
        frame: &Frame,
        landmarks: &[[f32; 2]; 5],
    ) -> Result<Embedding, OrtVisionError> {
        let mat = alignment_matrix(landmarks);
        let crop = align_crop(frame, &mat, 112, 112);
        self.embed_crop(&crop)
    }

    /// 直接嵌入 112×112 RGB888 对齐图。
    pub fn embed_crop(&self, rgb112: &Frame) -> Result<Embedding, OrtVisionError> {
        let slot = &self.inner.sface;
        // SFace 输入：RGB 0-255（OpenCV FaceRecognizerSF.feature 语义）
        let mut blob = nchw_from_rgb(rgb112);
        let shape: Vec<i64> = vec![1, 3, 112, 112];
        let outputs = self
            .inner
            .run(slot, &mut blob, &shape)
            .map_err(|e| OrtVisionError::Runtime(e.to_string()))?;
        let feat = outputs.first().cloned().unwrap_or_default();
        let norm: f64 = feat
            .iter()
            .map(|x| (*x as f64) * (*x as f64))
            .sum::<f64>()
            .sqrt();
        if norm < 1e-10 {
            return Err(OrtVisionError::Runtime("零特征向量".into()));
        }
        let vector: Vec<f64> = feat.iter().map(|x| *x as f64 / norm).collect();
        Ok(Embedding {
            dim: vector.len(),
            vector,
        })
    }
}

// ---------------- 预处理 ----------------

/// letterbox + BGR NCHW blob：等比缩放贴左上、黑边填充（face-video prepare 同款）。
/// 返回 (blob, sx, sy)。Frame 是 RGB888，模型按 BGR 消费 → 通道反转。
pub fn letterbox_and_blob(frame: &Frame, tw: u32, th: u32) -> (Vec<f32>, f64, f64) {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let ratio = (tw as f64 / w as f64).min(th as f64 / h as f64);
    let dw = ((w as f64) * ratio) as usize;
    let dh = ((h as f64) * ratio) as usize;
    let (twu, thu) = (tw as usize, th as usize);
    let mut blob = vec![0f32; 3 * thu * twu];
    for y in 0..dh.min(thu) {
        let sy = ((y as f64 / ratio) as usize).min(h - 1);
        for x in 0..dw.min(twu) {
            let sx = ((x as f64 / ratio) as usize).min(w - 1);
            let off = (sy * w + sx) * 3;
            // RGB → BGR（模型按 OpenCV BGR 契约）：B=plane0, G=plane1, R=plane2
            let c2 = thu * twu;
            blob[y * twu + x] = frame.data[off + 2] as f32;
            blob[c2 + y * twu + x] = frame.data[off + 1] as f32;
            blob[2 * c2 + y * twu + x] = frame.data[off] as f32;
        }
    }
    (blob, dw as f64 / w as f64, dh as f64 / h as f64)
}

/// HWC RGB → NCHW（无通道反转；SFace 用 RGB）。
pub fn nchw_from_rgb(frame: &Frame) -> Vec<f32> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut blob = vec![0f32; 3 * h * w];
    for c in 0..3 {
        for y in 0..h {
            for x in 0..w {
                blob[c * h * w + y * w + x] = frame.data[(y * w + x) * 3 + c] as f32;
            }
        }
    }
    blob
}

fn blob_vec(blob: Vec<f32>) -> Vec<f32> {
    blob
}

// ---------------- YuNet 解码（run_photos.decode 移植） ----------------

/// 12 输出解码 + NMS + 坐标映射回原帧（输出 Detection，原坐标）。
pub fn decode_yunet(outputs: &[Vec<f32>], sx: f64, sy: f64, conf_threshold: f64) -> Vec<Detection> {
    assert_eq!(outputs.len(), 12, "YuNet requires 12 outputs");
    let mut candidates: Vec<Detection> = Vec::new();
    for (level, stride) in [8u32, 16, 32].into_iter().enumerate() {
        let side = 640usize / stride as usize;
        let count = side * side;
        let cls = &outputs[level];
        let obj = &outputs[level + 3];
        let boxes = &outputs[level + 6];
        let kps = &outputs[level + 9];
        if cls.len() != count
            || obj.len() != count
            || boxes.len() != count * 4
            || kps.len() != count * 10
        {
            continue; // 布局不符：跳过该层
        }
        for idx in 0..count {
            let score = (cls[idx].clamp(0.0, 1.0) as f64 * obj[idx].clamp(0.0, 1.0) as f64).sqrt();
            if score < conf_threshold {
                continue;
            }
            let (gx, gy) = ((idx % side) as f64, (idx / side) as f64);
            // numpy boxes.reshape(count,4) → flat[idx*4+j]；kps.reshape(count,5,2) → flat[idx*10+k*2+j]
            let cx = (gx + boxes[idx * 4] as f64) * stride as f64;
            let cy = (gy + boxes[idx * 4 + 1] as f64) * stride as f64;
            let w = boxes[idx * 4 + 2].exp() as f64 * stride as f64;
            let h = boxes[idx * 4 + 3].exp() as f64 * stride as f64;
            let mut landmarks = [[0u32; 2]; 5];
            for k in 0..5 {
                let px = ((kps[idx * 10 + k * 2] as f64) + gx) * stride as f64;
                let py = ((kps[idx * 10 + k * 2 + 1] as f64) + gy) * stride as f64;
                landmarks[k] = [(px / sx) as u32, (py / sy) as u32];
            }
            candidates.push(Detection {
                bbox: [
                    ((cx - w / 2.0) / sx) as u32,
                    ((cy - h / 2.0) / sy) as u32,
                    ((cx + w / 2.0) / sx) as u32,
                    ((cy + h / 2.0) / sy) as u32,
                ],
                landmarks,
                score,
            });
        }
    }
    candidates
}

/// 贪心 NMS（IoU 阈值）。
pub fn nms(dets: &mut Vec<Detection>, iou_threshold: f64) {
    dets.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut keep: Vec<Detection> = Vec::new();
    for d in dets.drain(..) {
        let suppressed = keep.iter().any(|k| {
            iou(
                [
                    d.bbox[0] as f64,
                    d.bbox[1] as f64,
                    d.bbox[2] as f64,
                    d.bbox[3] as f64,
                ],
                [
                    k.bbox[0] as f64,
                    k.bbox[1] as f64,
                    k.bbox[2] as f64,
                    k.bbox[3] as f64,
                ],
            ) > iou_threshold
        });
        if !suppressed {
            keep.push(d);
        }
    }
    *dets = keep;
}

fn iou(a: [f64; 4], b: [f64; 4]) -> f64 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = a[2].min(b[2]);
    let y1 = a[3].min(b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let area_a = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
    let area_b = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
    let union = area_a + area_b - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

// ---------------- 5 点相似变换对齐（face-video alignment_matrix 移植） ----------------

/// ArcFace 112×112 五点模板（与 InsightFace/face-video 同几何）。
pub const ARCFACE_TEMPLATE: [[f64; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// 最小二乘相似变换矩阵 [[a, -b, tx], [b, a, ty]]（5 点拟合）。
pub fn alignment_matrix(landmarks: &[[f32; 2]; 5]) -> [f64; 6] {
    let src: Vec<[f64; 2]> = landmarks
        .iter()
        .map(|p| [p[0] as f64, p[1] as f64])
        .collect();
    let tgt = ARCFACE_TEMPLATE;
    let mut sm = [0.0f64; 2];
    let mut tm = [0.0f64; 2];
    for i in 0..5 {
        sm[0] += src[i][0];
        sm[1] += src[i][1];
        tm[0] += tgt[i][0];
        tm[1] += tgt[i][1];
    }
    sm = [sm[0] / 5.0, sm[1] / 5.0];
    tm = [tm[0] / 5.0, tm[1] / 5.0];
    let mut a = 0.0;
    let mut b_num = 0.0;
    let mut denom = 0.0;
    for i in 0..5 {
        let sx = src[i][0] - sm[0];
        let sy = src[i][1] - sm[1];
        let tx = tgt[i][0] - tm[0];
        let ty = tgt[i][1] - tm[1];
        a += sx * tx + sy * ty;
        b_num += sx * ty - sy * tx;
        denom += sx * sx + sy * sy;
    }
    if denom < 1e-10 {
        // 退化：恒等变换
        return [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    }
    a /= denom;
    let b = b_num / denom;
    [
        a,
        -b,
        tm[0] - a * sm[0] + b * sm[1],
        b,
        a,
        tm[1] - b * sm[0] - a * sm[1],
    ]
}

/// 双线性 warpAffine 等价（112×112 RGB888 输出）。
pub fn align_crop(frame: &Frame, m: &[f64; 6], out_w: u32, out_h: u32) -> Frame {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut data = vec![0u8; (out_w as usize) * (out_h as usize) * 3];
    for oy in 0..out_h as usize {
        for ox in 0..out_w as usize {
            // forward 矩阵(s→t)求逆后采样（warpAffine 语义）：相似阵 R=[[a,-b],[b,a]]，
            // R⁻¹ = 1/(a²+b²)·[[a,b],[-b,a]]，平移取负
            let det = m[0] * m[4] - m[1] * m[3];
            let ia = m[0] / det;
            let ib = m[1] / det;
            let sx = ia * ox as f64 + ib * oy as f64 + (m[1] * m[5] - m[4] * m[2]) / det;
            let sy = -m[1] * ox as f64 + m[0] * oy as f64 + (m[3] * m[2] - m[0] * m[5]) / det;
            if sx < 0.0 || sy < 0.0 || sx >= w as f64 - 1.0 || sy >= h as f64 - 1.0 {
                continue;
            }
            let x0 = sx as usize;
            let y0 = sy as usize;
            let fx = sx - x0 as f64;
            let fy = sy - y0 as f64;
            for c in 0..3 {
                let p00 = frame.data[(y0 * w + x0) * 3 + c] as f64;
                let p10 = frame.data[(y0 * w + x0 + 1) * 3 + c] as f64;
                let p01 = frame.data[((y0 + 1).min(h - 1) * w + x0) * 3 + c] as f64;
                let p11 = frame.data[((y0 + 1).min(h - 1) * w + x0 + 1) * 3 + c] as f64;
                let v = p00 * (1.0 - fx) * (1.0 - fy)
                    + p10 * fx * (1.0 - fy)
                    + p01 * (1.0 - fx) * fy
                    + p11 * fx * fy;
                data[(oy * out_w as usize + ox) * 3 + c] = v.round() as u8;
            }
        }
    }
    Frame {
        id: 0,
        token: format!("align-{out_w}x{out_h}"),
        session: "align".into(),
        captured_ms: 0,
        width: out_w,
        height: out_h,
        format: crate::frame::Format::Rgb888,
        data,
    }
}

fn fmt_nul(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成 YuNet 12 输出：在 stride=8 网格 (gx,gy)=(10,5) 处放一个高置信度人脸，
    /// 其余位置低分。decode 后应检出恰好在 (10*8+8*box0) 附近的框 + 5 个关键点。
    #[test]
    fn decode_yunet_synthetic_one_face() {
        let stride = 8usize;
        let side = 640 / stride;
        let count = side * side;
        // 层序契约（face-video decode）：[cls8,cls16,cls32, obj8,obj16,obj32, box8,box16,box32, kps8,kps16,kps32]
        // 层长：level s → (640/s)²；box ×4，kps ×10
        let mut outputs = Vec::new();
        for stride_l in [8usize, 16, 32] {
            let n = (640 / stride_l) * (640 / stride_l);
            outputs.push(vec![0f32; n]); // cls
        }
        for stride_l in [8usize, 16, 32] {
            let n = (640 / stride_l) * (640 / stride_l);
            outputs.push(vec![0f32; n]); // obj
        }
        for stride_l in [8usize, 16, 32] {
            let n = (640 / stride_l) * (640 / stride_l);
            outputs.push(vec![0f32; n * 4]); // box
        }
        for stride_l in [8usize, 16, 32] {
            let n = (640 / stride_l) * (640 / stride_l);
            outputs.push(vec![0f32; n * 10]); // kps
        }
        let (gx, gy) = (10usize, 5usize);
        let idx = gy * side + gx;
        // cls=0.9, obj=0.9 → score=0.9（层序 cls@0, obj@3, box@6, kps@9）
        outputs[0][idx] = 0.9;
        outputs[3][idx] = 0.9;
        // box 偏移：中心=格点+0.5 格，宽高=2 格
        outputs[6][idx * 4] = 0.5;
        outputs[6][idx * 4 + 1] = 0.5;
        outputs[6][idx * 4 + 2] = 2.0f64.ln() as f32;
        outputs[6][idx * 4 + 3] = 2.0f64.ln() as f32;
        // 5 点关键点：格点附近 ±0.25
        for k in 0..5 {
            outputs[9][idx * 10 + k * 2] = 0.25;
            outputs[9][idx * 10 + k * 2 + 1] = -0.25;
        }
        let sx = 1.0;
        let sy = 1.0;
        let dets = decode_yunet(&outputs, sx, sy, 0.6);
        assert_eq!(dets.len(), 1, "合成单脸应检出 1");
        let d = &dets[0];
        // cx=(10+0.5)*8=84，w=exp(ln2)*8=16
        assert!((d.bbox[0] as f64 - (84.0 - 8.0)).abs() < 1.0);
        eprintln!("dbg bbox: {:?}", d.bbox);
        assert!((d.bbox[2] as f64 - (84.0 + 8.0)).abs() < 1.0);
        assert!(d.score > 0.89);
        assert_eq!(d.landmarks.len(), 5);
        // NMS：单脸不受影响
        let mut one = vec![d.clone()];
        nms(&mut one, 0.3);
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn nms_suppresses_overlap_keeps_distinct() {
        let mk = |x0: u32, y0: u32, score: f64| Detection {
            bbox: [x0, y0, x0 + 40, y0 + 40],
            landmarks: [[0, 0]; 5],
            score,
        };
        let mut dets = vec![mk(0, 0, 0.9), mk(5, 5, 0.8), mk(200, 200, 0.85)];
        nms(&mut dets, 0.3);
        assert_eq!(dets.len(), 2, "重叠者被抑制、远离者保留");
        assert_eq!(dets[0].score, 0.9);
    }

    #[test]
    fn letterbox_maps_ratio_and_black_border() {
        // 320×160 图 → 320×320 画布：等比 2x 贴上，下 160 行黑边
        let frame = Frame {
            id: 0,
            token: "t".into(),
            session: "s".into(),
            captured_ms: 0,
            width: 320,
            height: 160,
            format: crate::frame::Format::Rgb888,
            data: vec![255; 320 * 160 * 3],
        };
        let (blob, sx, sy) = letterbox_and_blob(&frame, 320, 320);
        assert!((sx - 1.0).abs() < 1e-9);
        // ratio=min(320/320, 320/160)=1.0 → 贴上 160 行，下 160 行黑边（face-video prepare 同款）
        assert!((sy - 1.0).abs() < 1e-9);
        // NCHW：通道 0 在前 320*320
        assert_eq!(blob.len(), 3 * 320 * 320);
        // BGR plane0（蓝通道）：图区（y=100,x=100）源 RGB[0]=255 → BGR[2]... 检查 G 通道（plane1）=255
        let p1 = 320 * 320;
        assert!(blob[p1 + 100 * 320 + 100] > 250.0);
        // 缩放区内（y=100）BGR-B(plane0) 应=RGB-R=255；黑边区（y=300）应为 0
        assert!(blob[100 * 320 + 100] > 250.0);
        assert!(blob[300 * 320 + 100] == 0.0);
    }

    #[test]
    fn alignment_matrix_translation_scale() {
        // 5 点全等于模板 → 恒等矩阵；整体平移 → tx/ty 平移
        let mut lm = [[0f32; 2]; 5];
        for (i, t) in ARCFACE_TEMPLATE.iter().enumerate() {
            lm[i] = [t[0] as f32, t[1] as f32];
        }
        let m = alignment_matrix(&lm);
        assert!((m[0] - 1.0).abs() < 1e-6 && m[1].abs() < 1e-6);
        let mut shifted = lm.clone();
        for p in shifted.iter_mut() {
            p[0] += 10.0;
            p[1] += 20.0;
        }
        let m2 = alignment_matrix(&shifted);
        // forward s→t：模板在源左上 10/20 处 → tx=-10/ty=-20（warpAffine 内部求逆采样）
        assert!((m2[2] + 10.0).abs() < 1e-3, "tx 应=-10，实际 {}", m2[2]);
        assert!((m2[5] + 20.0).abs() < 1e-3, "ty 应=-20，实际 {}", m2[5]);
    }
}

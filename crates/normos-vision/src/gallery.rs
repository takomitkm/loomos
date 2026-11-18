//! 人脸库：特征入库 + 余弦匹配（旧板 gallery / face.match.threshold=0.40 口径）。

use crate::embed::Embedding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// 旧 control/settings.py：face.match.threshold 默认 0.40。
pub const DEFAULT_FACE_MATCH_THRESHOLD: f64 = 0.40;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaceGallery {
    pub threshold: f64,
    pub persons: BTreeMap<String, Embedding>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum GalleryError {
    #[error("人脸库为空")]
    Empty,
    #[error("维度不符：库 {0}，查询 {1}")]
    DimMismatch(usize, usize),
    #[error("人物 {0} 已存在（enroll 需显式 update）")]
    AlreadyEnrolled(String),
}

/// 一次匹配的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchResult {
    /// 命中的人名；未达阈值时为 None。
    pub name: Option<String>,
    pub best_score: f64,
    pub threshold: f64,
    /// 全库分数（调试用）。
    pub scores: Vec<(String, f64)>,
}

impl FaceGallery {
    pub fn new(threshold: f64) -> Self {
        Self {
            threshold,
            persons: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.persons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.persons.is_empty()
    }

    pub fn enroll(&mut self, name: &str, emb: Embedding) -> Result<(), GalleryError> {
        if self.persons.contains_key(name) {
            return Err(GalleryError::AlreadyEnrolled(name.to_string()));
        }
        self.persons.insert(name.to_string(), emb);
        Ok(())
    }

    /// 更新已存在人物的模板（同 ID 显式覆盖，不自动合并——旧 control/persons 纪律）。
    pub fn update(&mut self, name: &str, emb: Embedding) -> Result<(), GalleryError> {
        if !self.persons.contains_key(name) {
            return Err(GalleryError::Empty);
        }
        self.persons.insert(name.to_string(), emb);
        Ok(())
    }

    /// 余弦匹配：最高分 ≥ 阈值才认人。
    pub fn match_embedding(&self, emb: &Embedding) -> Result<MatchResult, GalleryError> {
        if self.persons.is_empty() {
            return Err(GalleryError::Empty);
        }
        let mut scores: Vec<(String, f64)> = self
            .persons
            .iter()
            .filter(|(_, e)| e.dim == emb.dim)
            .map(|(name, e)| (name.clone(), cosine(&emb.vector, &e.vector)))
            .collect();
        if scores.is_empty() {
            let expect = self.persons.values().next().map(|e| e.dim).unwrap_or(0);
            return Err(GalleryError::DimMismatch(expect, emb.dim));
        }
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let (name, best) = scores
            .first()
            .map(|(n, s)| (Some(n.clone()), *s))
            .unwrap_or((None, 0.0));
        Ok(MatchResult {
            name: if best >= self.threshold { name } else { None },
            best_score: best,
            threshold: self.threshold,
            scores,
        })
    }
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na * nb < 1e-12 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{FaceDetector, FaceEmbedder, MOCK_EMBED_DIM, MockEmbedder};
    use crate::frame::{FaceSpec, FrameSource, MockFrameSource, SyntheticScene};

    fn clock() -> u64 {
        1_000_000
    }

    fn embedding_of(signature: u8, cx: f64, cy: f64) -> Embedding {
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
        let f = src.latest().unwrap();
        let det = &crate::embed::MockDetector.detect(&f)[0];
        MockEmbedder.embed(&f, det).unwrap()
    }

    #[test]
    fn enroll_match_same_person() {
        let mut g = FaceGallery::new(DEFAULT_FACE_MATCH_THRESHOLD);
        g.enroll("张三", embedding_of(200, 0.5, 0.5)).unwrap();
        let r = g.match_embedding(&embedding_of(200, 0.5, 0.5)).unwrap();
        assert_eq!(r.name.as_deref(), Some("张三"));
        assert!((r.best_score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn different_person_below_threshold() {
        let mut g = FaceGallery::new(DEFAULT_FACE_MATCH_THRESHOLD);
        g.enroll("张三", embedding_of(200, 0.5, 0.5)).unwrap();
        let r = g.match_embedding(&embedding_of(120, 0.5, 0.5)).unwrap();
        assert!(r.name.is_none(), "不同人不应命中，score={}", r.best_score);
        assert_eq!(r.scores.len(), 1);
    }

    #[test]
    fn duplicate_enroll_rejected() {
        let mut g = FaceGallery::new(DEFAULT_FACE_MATCH_THRESHOLD);
        g.enroll("张三", embedding_of(200, 0.5, 0.5)).unwrap();
        assert!(matches!(
            g.enroll("张三", embedding_of(200, 0.5, 0.5)),
            Err(GalleryError::AlreadyEnrolled(_))
        ));
        // 显式 update 允许
        g.update("张三", embedding_of(200, 0.4, 0.4)).unwrap();
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn default_dim_matches_mock() {
        assert_eq!(MOCK_EMBED_DIM, 32);
    }
}

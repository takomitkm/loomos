//! Bundle 清单（RAUC 兼容字段子集，TOML）。
//!
//! RAUC 的 `manifest.raucs` 是 INI 风格；本清单用 TOML 表达**同一语义子集**
//! （compatible/slot group/image/checksum），P5 板端接入真 RAUC 时由打包器
//! 转换。溯源字段（source/flake/wamr）是 loomos 扩展——对应 ADR-012 §决定 6。
//!
//! 纪律：签名密钥不入清单/包（ADR-013 §决定 5）；每个 image 必须带校验和。

use serde::{Deserialize, Serialize};

/// 一个目标镜像条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageEntry {
    /// 槽位类（RAUC 语义："rootfs" / "app" / ...；loomos v1 只用 rootfs）。
    pub slot_class: String,
    /// 文件名（bundle 内相对路径）。
    pub filename: String,
    /// SHA-256（hex，64 字符）。
    pub checksum: String,
    /// 字节数（读回校验用）。
    pub size: u64,
}

/// 更新 bundle 清单。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    /// 清单格式版本。
    pub version: u32,
    /// 板型兼容标识（必须与设备 compatible 精确匹配——ADR-013 §决定 3 步骤 6）。
    pub compatible: String,
    /// 镜像列表。
    pub images: Vec<ImageEntry>,
    // ---- loomos 溯源扩展（ADR-012 §决定 6）----
    /// 源仓库 commit。
    #[serde(default)]
    pub source_commit: Option<String>,
    /// flake.lock 摘要（可复现构建指纹）。
    #[serde(default)]
    pub flake_ref: Option<String>,
    /// WAMR 版本（执行层工件一致性）。
    #[serde(default)]
    pub wamr_version: Option<String>,
}

/// 校验器 trait：签名 + 校验和 + 板型兼容。实现方注入（测试用真 sha256 实现）。
pub trait BundleVerifier: Send + Sync {
    /// 校验清单与实际文件；失败 → VerificationFailed（写槽绝不发生）。
    fn verify(
        &self,
        manifest: &BundleManifest,
        device_compatible: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), crate::AbError>;
}

/// 内置校验器：SHA-256 校验和（每文件）+ 板型精确匹配 + 清单内部一致性。
/// **签名校验**由部署层提供实现（密钥在 Nix 之外——ADR-013）；本器只做
/// 完整性与兼容——无签名实现时如实以 `SignatureUnverified` 记录。
pub struct Sha256Verifier;

impl BundleVerifier for Sha256Verifier {
    fn verify(
        &self,
        manifest: &BundleManifest,
        device_compatible: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), crate::AbError> {
        use crate::AbError;
        if manifest.version != 1 {
            return Err(AbError::VerificationFailed(format!(
                "清单版本 {} 不支持",
                manifest.version
            )));
        }
        if manifest.compatible != device_compatible {
            return Err(AbError::VerificationFailed(format!(
                "板型不匹配: bundle={} device={}",
                manifest.compatible, device_compatible
            )));
        }
        if manifest.images.is_empty() {
            return Err(AbError::VerificationFailed("清单无镜像".into()));
        }
        for img in &manifest.images {
            let found = files
                .iter()
                .find(|(name, _)| name == &img.filename)
                .ok_or_else(|| AbError::VerificationFailed(format!("缺文件: {}", img.filename)))?;
            if found.1.len() as u64 != img.size {
                return Err(AbError::VerificationFailed(format!(
                    "大小不符: {} (清单 {} 实际 {})",
                    img.filename,
                    img.size,
                    found.1.len()
                )));
            }
            let got = loomos_runtime::hash::sha256_hex(&found.1);
            if got != img.checksum {
                return Err(AbError::VerificationFailed(format!(
                    "校验和不符: {} (want {} got {got})",
                    img.filename, img.checksum
                )));
            }
        }
        Ok(())
    }
}

/// 便捷构造：为 `files` 计算校验和生成清单（打包器用）。
pub fn build_manifest(
    compatible: &str,
    images: Vec<(String, Vec<u8>)>,
    provenance: BundleManifest,
) -> BundleManifest {
    let imgs = images
        .iter()
        .map(|(name, bytes)| ImageEntry {
            slot_class: "rootfs".into(),
            filename: name.clone(),
            checksum: loomos_runtime::hash::sha256_hex(bytes),
            size: bytes.len() as u64,
        })
        .collect();
    BundleManifest {
        version: 1,
        compatible: compatible.to_string(),
        images: imgs,
        source_commit: provenance.source_commit,
        flake_ref: provenance.flake_ref,
        wamr_version: provenance.wamr_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loomos_runtime::hash::sha256_hex;

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![("rootfs.img".into(), b"loomos-system-image-v7".to_vec())]
    }

    fn manifest(compatible: &str, files: &[(String, Vec<u8>)]) -> BundleManifest {
        build_manifest(
            compatible,
            files.to_vec(),
            BundleManifest {
                version: 1,
                compatible: compatible.into(),
                images: vec![],
                source_commit: Some("06f6c8c".into()),
                flake_ref: Some("nixos-26.05".into()),
                wamr_version: Some("06f6c8c".into()),
            },
        )
    }

    #[test]
    fn verify_accepts_matching_bundle() {
        let f = files();
        let m = manifest("fmql30tai676h", &f);
        Sha256Verifier.verify(&m, "fmql30tai676h", &f).unwrap();
    }

    #[test]
    fn verify_rejects_board_mismatch() {
        let f = files();
        let m = manifest("fmql30tai676h", &f);
        let err = Sha256Verifier.verify(&m, "generic-x86_64", &f).unwrap_err();
        assert!(err.to_string().contains("板型不匹配"));
    }

    #[test]
    fn verify_rejects_checksum_tamper() {
        let mut f = files();
        // 同长度篡改（保持 size 校验通过，专测 checksum 校验）
        let mut tampered = f[0].1.clone();
        tampered[0] ^= 0xFF;
        f[0].1 = tampered;
        let m = manifest("fmql30tai676h", &files());
        let err = Sha256Verifier.verify(&m, "fmql30tai676h", &f).unwrap_err();
        assert!(err.to_string().contains("校验和不符"));
    }

    #[test]
    fn verify_rejects_missing_file_and_bad_size() {
        let f = files();
        let m = manifest("fmql30tai676h", &f);
        assert!(Sha256Verifier.verify(&m, "fmql30tai676h", &[]).is_err());
        let mut short = files();
        short[0].1.truncate(5);
        assert!(Sha256Verifier.verify(&m, "fmql30tai676h", &short).is_err());
    }

    #[test]
    fn checksum_is_deterministic_sha256() {
        let f = files();
        let m = manifest("x", &f);
        assert_eq!(m.images[0].checksum, sha256_hex(&f[0].1));
        assert_eq!(m.images[0].checksum.len(), 64);
    }
}

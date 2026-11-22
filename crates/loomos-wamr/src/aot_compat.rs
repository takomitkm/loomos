//! AOT 兼容键（ADR-011 §4.4）。
//!
//! `.wasm` 是正本分发产物；`.aot` 是目标平台专属工件。**绝不因为「架构名看起来
//! 兼容」就加载 AOT**——加载前校验兼容键（WAMR 版本/构建配置/wamrc 版本/OS/ISA/
//! CPU 基线/ABI/Host ABI 版本/源哈希/编译旗标）。
//!
//! 兼容键由构建管线（Nix wamrc derivation / CI）生成 `module.aot.compat.json`
//! sidecar；本模块校验 sidecar 与当前运行时键的一致性。
//!
//! 两道防线：
//! 1. 本模块：兼容键一致性（来源/管线追溯，本 crate 职责）；
//! 2. WAMR 本体：aot_load 对目标 ISA/ABI 的运行时校验（权威执行点，sys 层传递错误）。

use crate::error::WamrError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// AOT 魔数：文件前 4 字节 "\0aot"（WAMR core/config.h AOT_MAGIC_NUMBER=0x746f6100，
/// 小端序落盘即 [0x00,0x61,0x6f,0x74]）。
pub const AOT_MAGIC: [u8; 4] = [0x00, 0x61, 0x6f, 0x74];

/// AOT 文件格式版本（WAMR core/config.h AOT_CURRENT_VERSION=6，@06f6c8c 核对）。
pub const AOT_FORMAT_VERSION: u32 = 6;

/// AOT 兼容键——标识一个 `.aot` 工件能在哪个运行时环境执行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AotCompatKey {
    /// WAMR 源码版本（钉死 commit / tag）。
    pub wamr_version: String,
    /// WAMR 构建配置（feature 集指纹，如 "aot+interp+wasi"）。
    pub wamr_build_config: String,
    /// wamrc 版本（编译器工件身份）。
    pub wamrc_version: String,
    /// 宿主 OS（如 "linux"）。
    pub host_os: String,
    /// 目标 ISA（如 "x86_64" / "aarch64"）。
    pub target_isa: String,
    /// CPU 基线/特性（如 "generic" / "cortex-a53"）。
    pub cpu_features: String,
    /// 目标 ABI / libc（如 "gnu" / "musl"）。
    pub target_abi: String,
    /// Host ABI 版本（loomos 侧）。
    pub host_abi_version: u32,
    /// Wasm 源工件哈希（.wasm → .aot 的溯源）。
    pub wasm_source_hash: String,
    /// 编译旗标（wamrc 参数指纹）。
    pub compile_flags: String,
}

impl AotCompatKey {
    /// 当前运行时键。
    ///
    /// WAMR/ISA/OS 取构建期环境（`LOOMOS_WAMR_*` 环境变量由构建管线注入；
    /// 未注入时用 "unpinned" 占位——**stub/开发档**如实标记，生产档必须由
    /// Nix 管线显式注入，缺注入的生产构建会在与 sidecar 比对时暴露不一致）。
    pub fn current(host_abi_version: u32) -> Self {
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| "unpinned".into());
        Self {
            wamr_version: env("LOOMOS_WAMR_VERSION"),
            wamr_build_config: env("LOOMOS_WAMR_BUILD_CONFIG"),
            wamrc_version: env("LOOMOS_WAMRC_VERSION"),
            host_os: std::env::consts::OS.into(),
            target_isa: default_target_isa(),
            cpu_features: env("LOOMOS_TARGET_CPU"),
            target_abi: default_target_abi(),
            host_abi_version,
            wasm_source_hash: String::new(),
            compile_flags: env("LOOMOS_WAMRC_FLAGS"),
        }
    }

    /// 与当前运行时键比较；返回**不匹配字段名列表**（空 = 兼容）。
    pub fn mismatches(&self, current: &AotCompatKey) -> Vec<String> {
        let mut out = Vec::new();
        if self.wamr_version != current.wamr_version {
            out.push("wamr_version".into());
        }
        if self.wamr_build_config != current.wamr_build_config {
            out.push("wamr_build_config".into());
        }
        if self.wamrc_version != current.wamrc_version {
            out.push("wamrc_version".into());
        }
        if self.host_os != current.host_os {
            out.push("host_os".into());
        }
        if self.target_isa != current.target_isa {
            out.push("target_isa".into());
        }
        if self.cpu_features != current.cpu_features {
            out.push("cpu_features".into());
        }
        if self.target_abi != current.target_abi {
            out.push("target_abi".into());
        }
        if self.host_abi_version != current.host_abi_version {
            out.push("host_abi_version".into());
        }
        // wasm_source_hash / compile_flags：来源追溯字段，不参与运行时兼容判定
        // （同一运行时可以执行不同源哈希的工件；一致性由部署层校验）。
        out
    }
}

/// sidecar 路径约定：`module.aot` → `module.aot.compat.json`。
pub fn sidecar_path(aot_path: &Path) -> PathBuf {
    let mut s = aot_path.as_os_str().to_os_string();
    s.push(".compat.json");
    PathBuf::from(s)
}

/// 读取并校验 sidecar；`mode = Production` 时 sidecar 缺失直接拒绝（fail closed）。
pub fn check_sidecar(
    aot_path: &Path,
    current: &AotCompatKey,
    production: bool,
) -> Result<AotCompatKey, WamrError> {
    let side = sidecar_path(aot_path);
    let raw = match std::fs::read_to_string(&side) {
        Ok(raw) => raw,
        Err(_) if production => {
            return Err(WamrError::AotCompatKeyMissing(side.display().to_string()));
        }
        Err(_) => {
            // 开发档：无 sidecar 时放行——WAMR 本体仍是 ISA/ABI 校验的权威执行点。
            tracing::warn!(side = %side.display(), "AOT sidecar 缺失（开发档放行；WAMR 本体校验 ISA/ABI）");
            return Ok(AotCompatKey::current(current.host_abi_version));
        }
    };
    let key: AotCompatKey = serde_json::from_str(&raw).map_err(|e| WamrError::InvalidFormat {
        path: side.display().to_string(),
        reason: format!("sidecar 解析失败: {e}"),
    })?;
    let mm = key.mismatches(current);
    if !mm.is_empty() {
        return Err(WamrError::AotIncompatible {
            got: mm,
            want: format!("{current:?}"),
        });
    }
    Ok(key)
}

fn default_target_isa() -> String {
    // 与 wamrc --target 的 ISA 名对齐（x86_64 / aarch64）
    match std::env::consts::ARCH {
        "x86_64" => "x86_64".into(),
        "aarch64" => "aarch64".into(),
        other => other.into(),
    }
}

fn default_target_abi() -> String {
    if cfg!(target_env = "musl") {
        "musl".into()
    } else if cfg!(target_env = "msvc") {
        "msvc".into()
    } else {
        "gnu".into()
    }
}

/// AOT 文件头校验：魔数 + 格式版本（WAMR 加载前的第一道格式防线）。
pub fn validate_aot_header(bytes: &[u8]) -> Result<(), crate::error::WamrError> {
    if bytes.len() < 8 {
        return Err(WamrError::InvalidFormat {
            path: "<bytes>".into(),
            reason: format!(
                "AOT 文件过小: {} 字节（头 8 字节 = 魔数+版本）",
                bytes.len()
            ),
        });
    }
    if bytes[..4] != AOT_MAGIC {
        return Err(WamrError::InvalidFormat {
            path: "<bytes>".into(),
            reason: "AOT 魔数不符（期望 \\0aot）".into(),
        });
    }
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != AOT_FORMAT_VERSION {
        return Err(WamrError::InvalidFormat {
            path: "<bytes>".into(),
            reason: format!("AOT 格式版本 {version} != 支持 {AOT_FORMAT_VERSION}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(isa: &str, abi: &str, host_abi: u32) -> AotCompatKey {
        AotCompatKey {
            wamr_version: "06f6c8c".into(),
            wamr_build_config: "aot+interp+wasi".into(),
            wamrc_version: "2026.11.20".into(),
            host_os: "linux".into(),
            target_isa: isa.into(),
            cpu_features: "generic".into(),
            target_abi: abi.into(),
            host_abi_version: host_abi,
            wasm_source_hash: "deadbeef".into(),
            compile_flags: "-O3".into(),
        }
    }

    #[test]
    fn identical_keys_are_compatible() {
        let a = key("aarch64", "gnu", 1);
        assert!(a.mismatches(&key("aarch64", "gnu", 1)).is_empty());
    }

    #[test]
    fn mismatched_isa_is_listed() {
        let a = key("aarch64", "gnu", 1);
        let b = key("x86_64", "gnu", 1);
        let mm = a.mismatches(&b);
        assert_eq!(mm, vec!["target_isa".to_string()]);
    }

    #[test]
    fn host_abi_version_mismatch_detected() {
        let a = key("aarch64", "gnu", 1);
        let b = key("aarch64", "gnu", 2);
        assert_eq!(a.mismatches(&b), vec!["host_abi_version".to_string()]);
    }

    #[test]
    fn source_hash_is_not_runtime_compat_field() {
        // 同一运行时可执行不同源哈希的工件（来源追溯由部署层管）
        let mut a = key("aarch64", "gnu", 1);
        a.wasm_source_hash = "other".into();
        assert!(a.mismatches(&key("aarch64", "gnu", 1)).is_empty());
    }

    #[test]
    fn aot_header_accepts_v6_and_rejects_other() {
        let mut ok = AOT_MAGIC.to_vec();
        ok.extend_from_slice(&6u32.to_le_bytes());
        ok.extend_from_slice(&[0u8; 16]);
        assert!(validate_aot_header(&ok).is_ok());

        let mut bad_version = AOT_MAGIC.to_vec();
        bad_version.extend_from_slice(&5u32.to_le_bytes());
        assert!(validate_aot_header(&bad_version).is_err());

        let mut bad_magic = vec![0x00u8, 0x61, 0x73, 0x6d]; // wasm 魔数
        bad_magic.extend_from_slice(&6u32.to_le_bytes());
        assert!(validate_aot_header(&bad_magic).is_err());

        assert!(validate_aot_header(&[0u8; 4]).is_err()); // 过小
    }

    #[test]
    fn sidecar_path_appends_suffix() {
        let p = Path::new("/opt/loomos/modules/nlu.aot");
        assert_eq!(
            sidecar_path(p),
            Path::new("/opt/loomos/modules/nlu.aot.compat.json")
        );
    }

    #[test]
    fn production_fails_closed_without_sidecar() {
        let dir = std::env::temp_dir().join(format!("loomos-aot-prod-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let aot = dir.join("missing.aot"); // sidecar 不存在
        let cur = key("aarch64", "gnu", 1);
        let r = check_sidecar(&aot, &cur, true);
        assert!(matches!(r, Err(WamrError::AotCompatKeyMissing(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn development_warns_but_proceeds_without_sidecar() {
        let dir = std::env::temp_dir().join(format!("loomos-aot-dev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let aot = dir.join("missing-dev.aot");
        let cur = key(std::env::consts::ARCH, "gnu", 1).clone();
        let r = check_sidecar(&aot, &cur, false);
        assert!(r.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidecar_mismatch_rejected_in_dev_too() {
        let dir = std::env::temp_dir().join(format!("loomos-aot-mismatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let aot = dir.join("mismatch.aot");
        let mut sk = key(std::env::consts::ARCH, "gnu", 1);
        sk.target_isa = "riscv64".into(); // 与当前运行时不符
        std::fs::write(sidecar_path(&aot), serde_json::to_string(&sk).unwrap()).unwrap();
        let cur = key(std::env::consts::ARCH, "gnu", 1);
        let r = check_sidecar(&aot, &cur, false);
        assert!(matches!(r, Err(WamrError::AotIncompatible { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

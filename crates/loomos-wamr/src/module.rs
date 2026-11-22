//! Wasm/AOT 模块管理：**真实加载/卸载**（ADR-011 §4.1：不得只记账不加载）。
//!
//! 格式校验（本 crate 职责）：
//! - `.wasm`：魔数 `\0asm`；
//! - `.aot`：魔数 `\0aot` + 格式版本 6（见 aot_compat）+ 兼容键 sidecar 校验；
//! - 生产档缺 AOT 兼容键 → fail closed。
//!
//! 所有权契约：**模块缓冲与模块同生命周期**——WAMR runtime 可能引用其内容
//! （nlu-bench 实测约定），`LoadedModule` 持有缓冲直到 unload。

use crate::aot_compat::{AotCompatKey, validate_aot_header};
use crate::error::{WamrError, WamrResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Wasm 魔数 "\0asm"。
pub const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleFormat {
    /// WebAssembly 字节码（开发模式，Interpreter 直接执行）。
    Wasm,
    /// WAMR AOT 预编译（生产模式，目标平台专属）。
    Aot,
}

/// 按魔数识别模块格式（不读内容语义，只做第一道格式防线）。
pub fn detect_format(bytes: &[u8]) -> WamrResult<ModuleFormat> {
    if bytes.len() < 4 {
        return Err(WamrError::InvalidFormat {
            path: "<bytes>".into(),
            reason: format!("过小: {} 字节", bytes.len()),
        });
    }
    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if magic == WASM_MAGIC {
        Ok(ModuleFormat::Wasm)
    } else if magic == crate::aot_compat::AOT_MAGIC {
        validate_aot_header(bytes)?;
        Ok(ModuleFormat::Aot)
    } else {
        Err(WamrError::InvalidFormat {
            path: "<bytes>".into(),
            reason: format!("魔数不符: {:02x?}（期望 \\0asm 或 \\0aot）", magic),
        })
    }
}

/// 已加载模块（真实句柄 + 同生命周期缓冲）。
pub struct LoadedModule {
    pub name: String,
    pub format: ModuleFormat,
    pub bytes: u64,
    /// AOT 兼容键（AOT 档必有；从 sidecar 或开发档现取）。
    pub compat_key: Option<AotCompatKey>,
    /// 模块缓冲——unload 前必须持有（WAMR 可能引用其内容）。
    /// 生命周期字段：不被读取是契约本身（提前释放 = WAMR 悬垂引用）。
    #[allow(dead_code)]
    buf: Vec<u8>,
    /// WAMR 模块句柄（不透明；只经 loomos-wamr-sys 传回，永不解引用）。
    /// stub 档无句柄（None）——stub 下 load 失败，不会出现半加载态。
    #[cfg(feature = "wamr-link")]
    handle: loomos_wamr_sys::WasmModule,
}

impl LoadedModule {
    /// WAMR 模块句柄（仅 wamr-link 档）。
    #[cfg(feature = "wamr-link")]
    pub fn handle(&self) -> loomos_wamr_sys::WasmModule {
        self.handle
    }
}

/// 模块管理器：真实加载 / 卸载 / 按名查找。
#[derive(Default)]
pub struct ModuleManager {
    modules: BTreeMap<String, LoadedModule>,
}

/// load 时的 AOT 校验上下文（生产档要求 sidecar；开发档放行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// 生产：AOT 必须有兼容键 sidecar 且与当前运行时一致（fail closed）。
    Production,
    /// 开发：sidecar 缺失放行（WAMR 本体仍是 ISA/ABI 校验权威）。
    Development,
}

impl ModuleManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从文件加载（读取字节 + 格式校验 + WAMR 真实加载）。
    pub fn load_from_file(
        &mut self,
        name: &str,
        path: &Path,
        mode: ExecMode,
        host_abi_version: u32,
    ) -> WamrResult<()> {
        let bytes = std::fs::read(path).map_err(|e| WamrError::InvalidFormat {
            path: path.display().to_string(),
            reason: format!("读取失败: {e}"),
        })?;
        self.load_from_bytes(
            name,
            bytes,
            Some(path.display().to_string()),
            mode,
            host_abi_version,
        )
    }

    /// 从内存字节加载（bytes 被 LoadedModule 持有至 unload）。
    pub fn load_from_bytes(
        &mut self,
        name: &str,
        bytes: Vec<u8>,
        display_path: Option<String>,
        mode: ExecMode,
        host_abi_version: u32,
    ) -> WamrResult<()> {
        if self.modules.contains_key(name) {
            return Err(WamrError::ModuleAlreadyLoaded(name.into()));
        }
        let path_label = display_path.unwrap_or_else(|| format!("<{name}>"));
        let format = match detect_format(&bytes) {
            Ok(f) => f,
            Err(WamrError::InvalidFormat { reason, .. }) => {
                return Err(WamrError::InvalidFormat {
                    path: path_label,
                    reason,
                });
            }
            Err(e) => return Err(e),
        };
        let compat_key = if format == ModuleFormat::Aot {
            let path = Path::new(&path_label);
            let current = AotCompatKey::current(host_abi_version);
            let key =
                crate::aot_compat::check_sidecar(path, &current, mode == ExecMode::Production)?;
            Some(key)
        } else {
            None
        };
        let mut buf = bytes;
        let handle = loomos_wamr_sys::load(&mut buf)?;
        #[cfg(not(feature = "wamr-link"))]
        let _ = handle; // stub 档：load 必然已 Err 提前返回，此绑定仅为消除告警
        #[cfg(feature = "wamr-link")]
        self.modules.insert(
            name.to_string(),
            LoadedModule {
                name: name.to_string(),
                format,
                bytes: buf.len() as u64,
                compat_key,
                buf,
                handle,
            },
        );
        #[cfg(not(feature = "wamr-link"))]
        self.modules.insert(
            name.to_string(),
            LoadedModule {
                name: name.to_string(),
                format,
                bytes: buf.len() as u64,
                compat_key,
                buf,
            },
        );
        tracing::info!(name, ?format, "WAMR module loaded");
        Ok(())
    }

    /// 卸载模块（调用方须先保证无存活实例——RuntimeHost::unload_module 已守护）。
    pub fn unload(&mut self, name: &str) -> WamrResult<()> {
        let module_removed = self
            .modules
            .remove(name)
            .ok_or_else(|| WamrError::NoSuchModule(name.into()))?;
        #[cfg(feature = "wamr-link")]
        loomos_wamr_sys::unload(module_removed.handle())?;
        #[cfg(not(feature = "wamr-link"))]
        let _ = &module_removed; // stub 档：缓冲随 LoadedModule 释放即完成卸载
        // 缓冲随 LoadedModule 释放（与模块同生命周期契约）
        tracing::info!(name, "WAMR module unloaded");
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&LoadedModule> {
        self.modules.get(name)
    }

    pub fn list(&self) -> Vec<&LoadedModule> {
        self.modules.values().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WASM_BYTES: &[u8] = b"\0asm\x01\x00\x00\x00rest-of-module";
    const AOT_BYTES: &[u8] = b"\0aot\x06\x00\x00\x00rest-of-aot-image";

    #[test]
    fn detects_wasm_magic() {
        assert_eq!(detect_format(WASM_BYTES).unwrap(), ModuleFormat::Wasm);
    }

    #[test]
    fn detects_aot_magic_and_v6() {
        assert_eq!(detect_format(AOT_BYTES).unwrap(), ModuleFormat::Aot);
    }

    #[test]
    fn rejects_aot_wrong_version() {
        let bad = b"\0aot\x05\x00\x00\x00rest";
        assert!(matches!(
            detect_format(bad),
            Err(WamrError::InvalidFormat { ref reason, .. }) if reason.contains("版本")
        ));
    }

    #[test]
    fn rejects_unknown_magic() {
        assert!(detect_format(b"GOFF ver 1").is_err());
        assert!(detect_format(b"").is_err());
        assert!(detect_format(b"\0a").is_err());
    }

    #[test]
    fn stub_mode_load_returns_notlinked_and_leaves_no_half_state() {
        // stub 档：load 必然失败；模块表不得出现半加载条目
        if loomos_wamr_sys::linked() {
            return; // wamr-link 档不测此路径（CI 有真实库）
        }
        let mut mm = ModuleManager::new();
        let err = mm
            .load_from_bytes(
                "nlu",
                WASM_BYTES.to_vec(),
                None,
                ExecMode::Development,
                crate::host_abi::HOST_ABI_VERSION,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            WamrError::Sys(loomos_wamr_sys::SysError::NotLinked)
        ));
        assert!(mm.is_empty(), "失败加载不得留下半加载态");
    }

    #[test]
    fn duplicate_name_rejected_before_any_load() {
        let mut mm = ModuleManager::new();
        // 直接注入同名条目（绕过 stub load 失败）验证重名拒绝
        mm.modules.insert(
            "nlu".into(),
            LoadedModule {
                name: "nlu".into(),
                format: ModuleFormat::Wasm,
                bytes: 4,
                compat_key: None,
                buf: vec![0u8; 4],
                #[cfg(feature = "wamr-link")]
                handle: std::ptr::null_mut(),
            },
        );
        let err = mm
            .load_from_bytes("nlu", WASM_BYTES.to_vec(), None, ExecMode::Development, 1)
            .unwrap_err();
        assert!(matches!(err, WamrError::ModuleAlreadyLoaded(_)));
    }
}

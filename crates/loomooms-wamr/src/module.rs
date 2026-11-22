//! Wasm/AOT 模块加载与卸载。

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub path: String,
    pub format: ModuleFormat,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleFormat {
    /// WebAssembly 字节码（开发模式，Interpreter 直接执行）。
    Wasm,
    /// WAMR AOT 预编译（生产模式，目标平台专属）。
    Aot,
}

pub struct ModuleHandle {
    pub name: String,
    #[cfg(feature = "wamr-link")]
    pub module: *mut std::ffi::c_void,
    #[cfg(not(feature = "wamr-link"))]
    pub module: (),
}

/// 模块管理器：加载 / 卸载 / 按名查找。
#[derive(Default)]
pub struct ModuleManager {
    modules: BTreeMap<String, ModuleInfo>,
}

impl ModuleManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_info(&mut self, info: ModuleInfo) {
        self.modules.insert(info.name.clone(), info);
    }

    pub fn get(&self, name: &str) -> Option<&ModuleInfo> {
        self.modules.get(name)
    }

    pub fn list(&self) -> Vec<&ModuleInfo> {
        self.modules.values().collect()
    }
}

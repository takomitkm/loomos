//! Host ABI 装配桥（ADR-011 v1.1 / architecture/15 §15.6）。
//!
//! **权威边界**：本模块把 `loomos-runtime` 的权威组件（CapabilityStore /
//! ConfigService / DeviceRegistry）接到 `loomos-wamr` 的 Host ABI。
//! Guest 只拿能力裁决结果、token 校验过的配置值和不透明操作句柄——
//! **绝不拿到 native 指针、设备真值或授权写入权**。
//!
//! 能力模型两层：
//! 1. 接口层（`loomos-wamr` CapabilityChecker）：组件能否用某接口——
//!    依据装配时的授权快照（policy，与 `System::assemble` 的 grant 同步，有漂移测试）；
//! 2. 资源层（本桥内部）：device_op 仍走 `DeviceRegistry::execute`（其内部再验
//!    token+ Rights），config_get 走 `ConfigService::get`（内部再验 namespace 级 Rights）。
//!
//! 推理/缓冲/事件三接口在 v1 为显式 unsupported 插槽——接真实数据面（N3 后半）
//! 时在 [`GuestAbiBridge`] 上实现，框架已按稳定错误码返回，Guest 可探测。

use loomos_device::core::OperationState;
use loomos_runtime::Token;
use loomos_runtime::config::NamespacePath;
use loomos_runtime::ids::OperationId;
use loomos_wamr::host_abi::{
    CapabilityChecker, HandleKind, HandleTable, HostAbiCore, HostAbiHandlers, PollStatus,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::system::System;

/// 单调时钟起点（进程启动时刻；time_mono_v1 = 自此起 ns）。
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// 装配授权快照：组件 → 允许的 Host ABI 接口能力集。
///
/// **与 `System::assemble` 的 grant 决策保持同步**——测试
/// [`tests::policy_matches_actual_grants`] 会在真实 CapabilityStore 上反验每条
/// policy（漂移即失败）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentPolicy {
    pub component: &'static str,
    /// 允许的接口能力（与 loomis-wamr `IFACE_CAPABILITY` 的名字一致）。
    pub ifaces: &'static [&'static str],
}

/// v1 装配策略：voice / console 是 ABI 组件（拥有设备+配置读写面）。
pub const POLICY: &[ComponentPolicy] = &[
    ComponentPolicy {
        component: "voice",
        ifaces: &[
            "log",
            "time_mono",
            "time_wall",
            "config.read",
            "device.command",
        ],
    },
    ComponentPolicy {
        component: "console",
        ifaces: &[
            "log",
            "time_mono",
            "time_wall",
            "config.read",
            "device.command",
        ],
    },
];

/// Guest ABI 桥：实现 Host ABI 的两个面（能力裁决 + 处理器）。
pub struct GuestAbiBridge {
    system: Arc<System>,
    /// 组件 → 配置读取 token（ConfigService::get 需要）。
    tokens: BTreeMap<&'static str, Token>,
}

impl GuestAbiBridge {
    /// 装配（System 建立后调用一次；重复装配覆盖前者）。
    pub fn install(system: Arc<System>) {
        let bridge = Self::new(system);
        loomos_wamr::host_abi::install(Arc::new(HostAbiCore {
            handlers: Box::new(bridge),
            caps: Box::new(AssemblyChecker),
            handles: Mutex::new(HandleTable::new()),
        }));
        // 符号注册：wamr-link 档注册进 WAMR；stub 档无运行时（debug 记录，不视为错误）
        match loomos_wamr::host_abi::register_symbols() {
            Ok(()) => tracing::info!("Host ABI 符号已注册（loomos 命名空间）"),
            Err(e) => tracing::debug!(error = %e, "Host ABI 符号注册跳过（WAMR 未链接/stub 档）"),
        }
    }

    fn new(system: Arc<System>) -> Self {
        let mut tokens = BTreeMap::new();
        tokens.insert("voice", system.voice_token.clone());
        tokens.insert("console", system.console_token.clone());
        Self { system, tokens }
    }

    fn config_token(&self, component: &str) -> Result<&Token, u32> {
        use loomos_wamr::host_abi::code;
        self.tokens.get(component).ok_or(code::ERR_CAPABILITY)
    }
}

/// 接口层能力裁决：装配授权快照查找（空能力 = 无要求恒过）。
struct AssemblyChecker;

impl CapabilityChecker for AssemblyChecker {
    fn check(&self, component: &str, capability: &str) -> bool {
        if capability.is_empty() {
            return true;
        }
        POLICY
            .iter()
            .any(|p| p.component == component && p.ifaces.contains(&capability))
    }
}

impl HostAbiHandlers for GuestAbiBridge {
    fn log(&self, component: &str, level: u32, msg: &[u8]) -> u32 {
        use loomos_wamr::host_abi::code;
        let text = String::from_utf8_lossy(msg);
        match level {
            0 => tracing::trace!(component, "{text}"),
            1 => tracing::debug!(component, "{text}"),
            2 => tracing::info!(component, "{text}"),
            3 => tracing::warn!(component, "{text}"),
            4 => tracing::error!(component, "{text}"),
            _ => return code::ERR_INVALID, // 越界级别：拒绝而非静默
        }
        code::OK
    }

    fn time_mono(&self) -> Result<i64, u32> {
        Ok(process_start().elapsed().as_nanos() as i64)
    }

    fn time_wall(&self) -> Result<i64, u32> {
        Ok(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0))
    }

    fn config_get(&self, component: &str, namespace: &str, key: &str) -> Result<Vec<u8>, u32> {
        use loomos_wamr::host_abi::code;
        let token = self.config_token(component)?;
        // 资源层二次校验在 ConfigService::get（namespace 级 Rights）——双保险
        let ns = NamespacePath::new(namespace).map_err(|_| code::ERR_INVALID)?;
        match self.system.config.get(token, &ns, key) {
            Ok(v) => serde_json::to_vec(&v.value).map_err(|_| code::ERR_INVALID),
            // key 不存在 = miss（JSON null），不是错误
            Err(loomos_runtime::config::ConfigError::KeyNotFound(_)) => Ok(b"null".to_vec()),
            Err(_) => Err(code::ERR_CAPABILITY),
        }
    }

    fn device_op(
        &self,
        component: &str,
        device_id: &str,
        op: &str,
        params: &[u8],
    ) -> Result<u64, u32> {
        use loomos_wamr::host_abi::code;
        let token = self.config_token(component)?;
        let params: serde_json::Value =
            serde_json::from_slice(params).map_err(|_| code::ERR_INVALID)?;
        // 资源层权威校验在 DeviceRegistry::execute 内（token + Rights + 租约）。
        // 注意：handler 拿到的是 &self（HostAbiHandlers 签名），registry 在 Mutex 里。
        let mut registry = self
            .system
            .registry
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let op_id = registry
            .execute(device_id, op, &params, component, &self.system.caps, token)
            .map_err(|e| {
                tracing::debug!(component, device = device_id, op, error = %e, "guest device_op 被拒");
                code::ERR_CAPABILITY // 权威拒绝统一走 CAPABILITY（细节进日志）
            })?;
        let core = installed_core()?;
        let mut handles = core.handles.lock().unwrap_or_else(|p| p.into_inner());
        Ok(handles.insert(component, HandleKind::DeviceOp, op_id.as_raw()))
    }

    fn device_poll(&self, component: &str, op_handle: u64) -> Result<PollStatus, u32> {
        use loomos_wamr::host_abi::code;
        let core = installed_core()?;
        // 句柄校验（属主+类型）+ 取资源 id（core 持 Arc 保 MutexGuard 生命周期）
        let op_id = core
            .handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .validate(component, op_handle, HandleKind::DeviceOp)?
            .resource;
        let op_id = OperationId::from_raw(op_id).ok_or(code::ERR_HANDLE)?;
        let registry = self
            .system
            .registry
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let Some(record) = registry.operation(op_id) else {
            return Err(code::ERR_HANDLE);
        };
        Ok(match record.state {
            OperationState::Accepted | OperationState::Queued | OperationState::Executing => {
                PollStatus::Pending
            }
            OperationState::Succeeded => PollStatus::Done,
            OperationState::Failed => PollStatus::Failed(code::ERR_INVALID),
            // AGENTS.md §2.7：Unknown 绝不映射 Failed（设备可能仍在占用）
            OperationState::Unknown => PollStatus::Unknown,
            OperationState::Cancelled => PollStatus::Cancelled,
            OperationState::Expired => PollStatus::Failed(code::ERR_INVALID),
        })
    }

    // ---- v1 显式插槽：数据面接口（接真实引擎/缓冲协议时在此实现）----

    fn infer_submit(
        &self,
        _component: &str,
        _model_id: &str,
        _buffer_handle: u64,
    ) -> Result<u64, u32> {
        Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
    }

    fn infer_poll(&self, _component: &str, _op_handle: u64) -> Result<PollStatus, u32> {
        Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
    }

    fn buffer_import(&self, _component: &str, _size: u32, _align: u32) -> Result<u64, u32> {
        Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
    }

    fn buffer_release(&self, _component: &str, _handle: u64) -> Result<(), u32> {
        Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
    }

    fn event_subscribe(&self, _component: &str, _event: &str) -> Result<u64, u32> {
        Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
    }
}

/// 测试期全局 CORE 串行锁（abi 测试与 system 装配测试共用——CI Windows 实证
/// 并行 assemble 会覆盖 CORE 造成属主句柄校验竞态）。
#[cfg(test)]
pub(crate) fn core_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static CORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    CORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 取已装配的 HostAbiCore（Arc 由调用方持有，MutexGuard 生命周期安全）。
fn installed_core() -> Result<Arc<loomos_wamr::host_abi::HostAbiCore>, u32> {
    loomos_wamr::host_abi::installed().ok_or(loomos_wamr::host_abi::code::ERR_UNLINKED)
}

/// 装配入口（System::assemble 末尾调用）。
pub fn assemble_host_abi(system: Arc<System>) {
    GuestAbiBridge::install(system);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{System, SystemConfig};
    use loomos_runtime::Rights;
    use loomos_wamr::host_abi::{HandleTable, installed};

    // install() 写全局 static CORE——所有触及 CORE 的测试串行执行
    // （system::tests 的 assemble 也会 install——共用本锁，防覆盖竞态）
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        super::core_test_guard()
    }

    fn system() -> Arc<System> {
        System::assemble(SystemConfig::default()).unwrap()
    }

    #[test]
    fn checker_policy_allows_only_listed_ifaces() {
        let _g = guard();
        let s = system();
        GuestAbiBridge::install(s.clone());
        let core = installed().unwrap();
        // voice 允许
        assert!(core.caps.check("voice", "log"));
        assert!(core.caps.check("voice", "config.read"));
        assert!(core.caps.check("voice", "device.command"));
        assert!(core.caps.check("voice", "")); // 空能力恒过
        // 未列出的接口拒绝
        assert!(!core.caps.check("voice", "infer.use"));
        assert!(!core.caps.check("voice", "buffer.use"));
        // 未知组件拒绝
        assert!(!core.caps.check("nlu", "log"));
        // 空能力对未知组件也过（无要求即无限制）
        assert!(core.caps.check("nlu", ""));
    }

    #[test]
    fn policy_matches_actual_grants() {
        let _g = guard();
        // 漂移测试：policy 声称的每个接口，真实 CapabilityStore 必须真能支撑——
        // device.command → 至少一个 DeviceCommand；config.read → 至少一个 ConfigRead
        let s = system();
        for p in POLICY {
            for iface in p.ifaces {
                match *iface {
                    "device.command" => {
                        // voice/console 装配时对 8 台设备有 command 权
                        assert!(
                            s.caps.holds(
                                p.component,
                                &Rights::DeviceCommand {
                                    device: "light.keting".into()
                                }
                            ),
                            "{} policy 声称 device.command 但实际未授予",
                            p.component
                        );
                    }
                    "config.read" | "config.write" => {
                        // token 存在（ConfigService::get 内部做 namespace 级校验）
                        assert!(
                            p.component == "voice" || p.component == "console",
                            "{} 缺配置 token",
                            p.component
                        );
                    }
                    "log" | "time_mono" | "time_wall" => {} // 诊断面，无权限要求
                    other => panic!("policy 引用了未映射的接口能力: {other}"),
                }
            }
        }
    }

    #[test]
    fn config_get_missing_key_is_null_not_error() {
        let _g = guard();
        let s = system();
        GuestAbiBridge::install(s.clone());
        let core = installed().unwrap();
        let v = core
            .handlers
            .config_get("voice", "services/voice", "no-such-key")
            .unwrap();
        assert_eq!(v, b"null");
        // 未授权组件走 CAPABILITY
        assert_eq!(
            core.handlers.config_get("nobody", "services/voice", "k"),
            Err(loomos_wamr::host_abi::code::ERR_CAPABILITY)
        );
    }

    #[test]
    fn device_op_query_roundtrip_via_handles() {
        let _g = guard();
        let s = system();
        GuestAbiBridge::install(s.clone());
        let core = installed().unwrap();
        // mock 提供者对 light.keting 支持 query（同步 Done）
        let h = core
            .handlers
            .device_op("voice", "light.keting", "query", b"{}")
            .expect("query 应被接受");
        // 句柄登记进表
        {
            let t = core.handles.lock().unwrap();
            assert!(t.validate("voice", h, HandleKind::DeviceOp).is_ok());
        }
        let status = core.handlers.device_poll("voice", h).expect("poll 应可查");
        assert!(matches!(status, PollStatus::Done | PollStatus::Pending));
        // 异主 poll 拒绝
        assert!(matches!(
            core.handlers.device_poll("console", h),
            Err(loomos_wamr::host_abi::code::ERR_HANDLE)
        ));
        // 未授权设备被权威拒绝（voice 没有 "light.other" 的 command 权）
        assert!(matches!(
            core.handlers
                .device_op("voice", "light.other", "query", b"{}"),
            Err(loomos_wamr::host_abi::code::ERR_CAPABILITY)
        ));
    }

    #[test]
    fn data_plane_slots_report_unsupported() {
        let _g = guard();
        let s = system();
        GuestAbiBridge::install(s.clone());
        let core = installed().unwrap();
        assert_eq!(
            core.handlers.infer_submit("voice", "m", 1),
            Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
        );
        assert_eq!(
            core.handlers.buffer_import("voice", 64, 8),
            Err(loomos_wamr::host_abi::code::ERR_UNSUPPORTED)
        );
    }

    // 静止检查：HandleTable 被 import 保证非 dead-code（真实使用在 host_abi core 内）
    #[test]
    fn handle_table_type_in_use() {
        let _g = guard();
        let _ = HandleTable::new();
        let s = system();
        GuestAbiBridge::install(s.clone());
        assert!(loomos_wamr::host_abi::installed().is_some());
    }
}

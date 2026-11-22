//! 错误类型（映射 loomos-wamr-sys 的 SysError + 门面层的生命周期/预算错误）。
//!
//! 命名与上报纪律（AGENTS.md §2）：不把「未链接」谎报成执行失败；
//! 不把实例创建失败谎报成模块加载失败；每类失败可定位到环节。

/// WAMR Runtime Host 错误。
#[derive(Debug, thiserror::Error)]
pub enum WamrError {
    /// 运行时未初始化。
    #[error("WAMR runtime 未初始化")]
    NotInitialized,
    /// 运行时已初始化（进程级单例）。
    #[error("WAMR runtime 已初始化（进程级单例）")]
    AlreadyInitialized,
    /// WAMR 未链接（stub 档无执行能力）。
    #[error("WAMR 未链接（stub 档；用 wamr-link feature + IWASM_LIB_DIR 启用）")]
    NotLinked,
    /// 模块未加载。
    #[error("模块未加载: {0}")]
    NoSuchModule(String),
    /// 模块已加载（同名）。
    #[error("模块已加载: {0}")]
    ModuleAlreadyLoaded(String),
    /// 模块格式非法（魔数/版本校验失败）。
    #[error("模块格式非法: {path} ({reason})")]
    InvalidFormat { path: String, reason: String },
    /// AOT 工件与当前运行时不兼容（兼容键不匹配；got=不匹配字段明细）。
    #[error("AOT 不兼容（不匹配字段: {got:?}；当前运行时键: {want}）")]
    AotIncompatible { got: Vec<String>, want: String },
    /// 生产模式缺少 AOT 兼容键 sidecar（fail closed，ADR-011 §4.4）。
    #[error("生产模式缺少 AOT 兼容键 sidecar: {0}")]
    AotCompatKeyMissing(String),
    /// 存在存活实例，不能卸载模块。
    #[error("模块 {module} 仍有 {count} 个存活实例，不能卸载")]
    ModuleBusy { module: String, count: usize },
    /// 实例不存在。
    #[error("实例不存在: {0}")]
    NoSuchInstance(String),
    /// 全局内存预算超出。
    #[error("内存预算超出: 请求 {requested} 字节, 已用 {used} 字节, 预算 {limit} 字节")]
    MemoryBudgetExceeded {
        requested: u64,
        used: u64,
        limit: u64,
    },
    /// 单实例内存上限超出。
    #[error("实例内存上限超出: 请求 {requested} 字节, 上限 {limit} 字节")]
    InstanceMemoryLimitExceeded { requested: u64, limit: u64 },
    /// 0（不限）内存在默认策略下被拒绝（production 默认有界，ADR-011 §4.3）。
    #[error("内存上限 0（不限）被拒绝——默认策略要求有界")]
    UnboundedMemoryForbidden,
    /// 运行时已停用/未就绪（shutdown 顺序守护；新调用被拒绝）。
    #[error("运行时已停用/未就绪，操作被拒绝")]
    Busy,
    /// Host ABI 版本不匹配。
    #[error("Host ABI 版本不匹配: guest v{guest} vs host v{host}")]
    AbiVersionMismatch { guest: u32, host: u32 },
    /// 能力检查失败（Host ABI 调用方无所需能力——Native Host 权威裁决）。
    #[error("能力检查失败: 组件 {component} 缺少能力 {capability}")]
    CapabilityDenied {
        component: String,
        capability: String,
    },
    /// Guest 内存访问越界。
    #[error("Guest 内存越界: offset {offset} + size {size} 超出线性内存")]
    GuestMemoryOutOfRange { offset: u64, size: u64 },
    /// 负载超出限额。
    #[error("负载超出限额: {actual} > {limit} 字节")]
    PayloadTooLarge { actual: u64, limit: u64 },
    /// 无效句柄（Host ABI 资源句柄表无此条目或属主不符）。
    #[error("无效资源句柄: {0}")]
    InvalidHandle(u64),
    /// sys 层错误（WAMR C API 失败，带 WAMR 错误文本）。
    #[error(transparent)]
    Sys(#[from] loomos_wamr_sys::SysError),
}

pub type WamrResult<T> = Result<T, WamrError>;

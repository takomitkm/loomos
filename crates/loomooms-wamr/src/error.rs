//! WAMR 运行时错误类型。

use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum WamrError {
    #[error("WAMR C API 未链接（--features wamr-link + IWASM_LIB_DIR）")]
    NotLinked,

    #[error("WAMR 运行时未初始化")]
    NotInitialized,

    #[error("WAMR 已初始化（不可重复 init）")]
    AlreadyInitialized,

    #[error("模块加载失败: {0}")]
    ModuleLoad(String),

    #[error("模块 {0} 不存在")]
    NoSuchModule(String),

    #[error("实例 {0} 不存在")]
    NoSuchInstance(String),

    #[error("实例化失败: {0}")]
    Instantiate(String),

    #[error("函数调用失败: {0}")]
    CallFailed(String),

    #[error("导出函数 {0} 不存在")]
    NoSuchExport(String),

    #[error("内存预算超限: 请求 {requested} 字节，上限 {limit} 字节")]
    MemoryBudgetExceeded { requested: u64, limit: u64 },

    #[error("Host 函数 {0} 未注册")]
    HostFunctionNotRegistered(String),

    #[error("Host ABI 版本不兼容: guest={guest}, host={host}")]
    AbiVersionMismatch { guest: u32, host: u32 },
}

pub type WamrResult<T> = Result<T, WamrError>;

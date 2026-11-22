//! WAMR 运行时生命周期：初始化 / 销毁。

use crate::error::{WamrError, WamrResult};
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub stack_size: u32,
    pub heap_size: u32,
    pub num_threads: i32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            stack_size: 64 * 1024,
            heap_size: 0,
            num_threads: 1,
        }
    }
}

static INSTANCE: Mutex<Option<RuntimeState>> = Mutex::new(None);

#[derive(Debug)]
pub struct RuntimeState {
    pub config: RuntimeConfig,
}

/// WAMR 运行时（进程级单例；重复 init 报 AlreadyInitialized）。
pub struct WamrRuntime;

impl WamrRuntime {
    pub fn init(config: RuntimeConfig) -> WamrResult<()> {
        let mut guard = INSTANCE.lock().unwrap();
        if guard.is_some() {
            return Err(WamrError::AlreadyInitialized);
        }
        #[cfg(feature = "wamr-link")]
        crate::ffi::full_init(&config)?;
        let stack = config.stack_size;
        *guard = Some(RuntimeState { config });
        tracing::info!(stack = stack, "WAMR runtime initialized");
        Ok(())
    }

    pub fn destroy() -> WamrResult<()> {
        let mut guard = INSTANCE.lock().unwrap();
        #[cfg(feature = "wamr-link")]
        crate::ffi::destroy();
        *guard = None;
        Ok(())
    }

    /// 通过闭包访问运行时状态（持有锁直到闭包完成）。
    pub fn with_state<R>(f: impl FnOnce(&RuntimeState) -> WamrResult<R>) -> WamrResult<R> {
        let guard = INSTANCE.lock().unwrap();
        match guard.as_ref() {
            Some(state) => f(state),
            None => Err(WamrError::NotInitialized),
        }
    }

    pub fn is_initialized() -> bool {
        INSTANCE.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

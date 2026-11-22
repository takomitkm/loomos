//! WAMR 运行时宿主：进程级单例 + 完整生命周期顺序（ADR-011 §4.2）。
//!
//! 所有权结构（一进程一 RuntimeHost，持有一切 WAMR 资源）：
//! ```text
//! RuntimeHost
//!   ├── ModuleManager     （模块：加载缓冲 + WAMR 模块句柄）
//!   ├── InstanceManager   （记账：预算预留/提交/释放 + 实例注册表）
//!   └── raw_insts         （WAMR 实例原始句柄，仅 wamr-link 档）
//! ```
//!
//! 关闭顺序（显式 shutdown；**不假设 Drop 保证清理**——Drop 只是兜底并大声记录）：
//! 1. 停止新调用（active=false）；
//! 2. 反实例化全部实例（执行环境随实例销毁）；
//! 3. 卸载全部模块（无实例后）；
//! 4. 释放全部预算预留（随实例销毁完成）；
//! 5. 销毁 WAMR 运行时（最后）。

use crate::error::{WamrError, WamrResult};
use crate::instance::{InstanceHandle, InstanceManager, InstanceSpec, Reservation};
use crate::module::{ExecMode, ModuleManager};
use loomos_wamr_sys as sys;
#[cfg(feature = "wamr-link")]
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// 进程级单例守护（WAMR full_init 是进程全局的，重复 init 属于编程错误）。
static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// 默认实例执行栈（字节）。
    pub stack_size: u32,
    /// 默认应用堆（字节；0 = 模块自管线性内存，bench 实测约定）。
    pub heap_size: u32,
    /// 执行档位：Production 要求 AOT+兼容键 fail closed；Development 允许 interp/sidecar 缺失。
    pub mode: ExecMode,
    /// 全局 Guest 线性内存预算（字节；production 有界）。
    pub memory_budget: u64,
    /// 单实例内存上限（字节）。
    pub per_instance_memory_limit: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            stack_size: 64 * 1024,
            heap_size: 0,
            mode: ExecMode::Development,
            // 生产默认有界（ADR-011 §4.3）：128 MiB 全局 / 32 MiB 单实例。
            // 板端 1GB RAM 的实际值由部署配置收紧。
            memory_budget: 128 * 1024 * 1024,
            per_instance_memory_limit: 32 * 1024 * 1024,
        }
    }
}

/// WAMR 运行时宿主。
pub struct RuntimeHost {
    cfg: RuntimeConfig,
    modules: ModuleManager,
    instances: InstanceManager,
    active: bool,
    /// WAMR 实例原始句柄（记账层 InstanceManager 保持纯簿记可测）。
    #[cfg(feature = "wamr-link")]
    raw_insts: BTreeMap<u64, sys::WasmModuleInst>,
}

impl RuntimeHost {
    /// 初始化（进程级单例；重复初始化 → AlreadyInitialized）。
    ///
    /// stub 档（WAMR 未链接）：宿主仍可建立（簿记/生命周期可测），
    /// 一切执行类操作在调用点返回 NotLinked。
    pub fn init(cfg: RuntimeConfig) -> WamrResult<Self> {
        if RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(WamrError::AlreadyInitialized);
        }
        if sys::linked() {
            let mut args = sys::RuntimeInitArgs::system_allocator();
            if let Err(e) = sys::full_init(&mut args) {
                RUNNING.store(false, Ordering::SeqCst);
                return Err(e.into());
            }
            // Host ABI 符号注册：**full_init 之后**（分配器就绪，表用 wasm_runtime_malloc
            // 分配、destroy 时 wasm_runtime_free 配对释放）；round-9/10 的 post-init
            // 崩溃实为 3 字段错位表（attachment 缺失），已修正
            if let Err(e) = crate::host_abi::register_runtime_natives() {
                RUNNING.store(false, Ordering::SeqCst);
                return Err(e);
            }
        }
        tracing::info!(
            linked = sys::linked(),
            budget = cfg.memory_budget,
            mode = ?cfg.mode,
            "WAMR runtime host initialized"
        );
        Ok(Self {
            instances: InstanceManager::new(cfg.memory_budget, cfg.per_instance_memory_limit),
            modules: ModuleManager::new(),
            cfg,
            active: true,
            #[cfg(feature = "wamr-link")]
            raw_insts: BTreeMap::new(),
        })
    }

    /// 当前运行时是否真实链接 WAMR。
    pub fn linked(&self) -> bool {
        sys::linked()
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.cfg
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 显式关闭（幂等；顺序见模块文档）。关闭后宿主不再可用。
    pub fn shutdown(&mut self) -> WamrResult<()> {
        if !self.active {
            tracing::debug!("shutdown 重入（幂等）");
            return Ok(());
        }
        self.active = false; // 1. 停止新调用
        // 2. 反实例化全部实例（预算随 destroy 释放）
        for h in self.instances.handles() {
            if let Err(e) = self.destroy_instance_internal(h) {
                tracing::error!(handle = h.0, error = %e, "shutdown: 实例销毁失败");
            }
        }
        // 3. 卸载全部模块（此刻必无实例）
        let names: Vec<String> = self.modules.list().iter().map(|m| m.name.clone()).collect();
        for name in names {
            if let Err(e) = self.modules.unload(&name) {
                tracing::error!(module = %name, error = %e, "shutdown: 模块卸载失败");
            }
        }
        // 4. 预算已随实例释放；5. 销毁 WAMR 运行时（最后）
        if sys::linked() {
            sys::destroy()?;
        }
        RUNNING.store(false, Ordering::SeqCst);
        tracing::info!("WAMR runtime host shutdown");
        Ok(())
    }

    // ---- 模块 ----

    /// 从文件加载模块（格式校验 + AOT 兼容键校验 + WAMR 真实加载）。
    pub fn load_module(&mut self, name: &str, path: &Path) -> WamrResult<()> {
        self.require_active()?;
        self.modules
            .load_from_file(name, path, self.cfg.mode, crate::host_abi::HOST_ABI_VERSION)
    }

    /// 卸载模块（存在存活实例 → ModuleBusy，绝不带实例卸载）。
    pub fn unload_module(&mut self, name: &str) -> WamrResult<()> {
        self.require_active()?;
        let alive = self.instances.instances_of(name);
        if alive > 0 {
            return Err(WamrError::ModuleBusy {
                module: name.into(),
                count: alive,
            });
        }
        self.modules.unload(name)
    }

    // ---- 实例 ----

    /// 实例化（原子预算预留 → WAMR 实例化 → 提交；失败自动回滚预留）。
    pub fn instantiate(
        &mut self,
        module_name: &str,
        mut spec: InstanceSpec,
    ) -> WamrResult<InstanceHandle> {
        self.require_active()?;
        spec.module_name = module_name.to_string();
        if spec.component_id.is_empty() {
            spec.component_id = module_name.to_string();
        }
        if spec.stack_size == 0 {
            spec.stack_size = self.cfg.stack_size;
        }
        let module = self
            .modules
            .get(module_name)
            .ok_or_else(|| WamrError::NoSuchModule(module_name.into()))?;
        #[cfg(not(feature = "wamr-link"))]
        let _ = module; // stub 档：模块存在性检查即用途

        let reservation: Reservation = self.instances.try_admit(&spec)?;
        #[cfg(feature = "wamr-link")]
        let inst = {
            // WASI P1 参数绑定（load 后、instantiate 前；bench 实测约定）。
            // argv[0] = 组件名；Guest std 的 WASI 导入依赖此项。
            const ARGV0: &[u8] = b"loomos\0";
            sys::set_wasi_args(
                module.handle(),
                &[ARGV0.as_ptr() as *const std::ffi::c_char],
            );
            match sys::instantiate(module.handle(), spec.stack_size, spec.heap_size) {
                Ok(inst) => inst,
                Err(e) => {
                    self.instances.rollback(reservation);
                    return Err(e.into());
                }
            }
        };
        let handle = self.instances.commit(reservation, spec);
        #[cfg(feature = "wamr-link")]
        self.raw_insts.insert(handle.0, inst);
        Ok(handle)
    }

    /// 销毁实例（反实例化 + 释放预算）。
    pub fn destroy_instance(&mut self, handle: InstanceHandle) -> WamrResult<()> {
        self.require_active()?;
        self.destroy_instance_internal(handle)
    }

    fn destroy_instance_internal(&mut self, handle: InstanceHandle) -> WamrResult<()> {
        #[cfg(feature = "wamr-link")]
        if let Some(inst) = self.raw_insts.remove(&handle.0) {
            sys::deinstantiate(inst)?;
        }
        self.instances.destroy(handle)?;
        Ok(())
    }

    pub fn instance_spec(&self, handle: InstanceHandle) -> Option<&InstanceSpec> {
        self.instances.get(handle)
    }

    // ---- 执行 ----

    /// 调用导出函数。**返回值在 argv[0]**（bench 实测约定）。
    /// argv 为参数/返回值共享缓冲；调用完成后由调用方读取。
    pub fn invoke(
        &mut self,
        handle: InstanceHandle,
        func: &str,
        argv: &mut [u32],
    ) -> WamrResult<()> {
        self.require_active()?;
        let spec = self
            .instances
            .get(handle)
            .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?
            .clone();
        #[cfg(feature = "wamr-link")]
        {
            let inst = *self
                .raw_insts
                .get(&handle.0)
                .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
            let function = sys::lookup_function(inst, func)?;
            let env = sys::create_exec_env(inst, spec.stack_size)?;
            // Host ABI 调用上下文：Guest 调用期内可识别调用方组件（ADR-011 §6.3）
            crate::host_abi::enter_call(&spec.component_id, inst);
            let result = sys::call_wasm(env, function, argv);
            crate::host_abi::exit_call();
            let _ = sys::destroy_exec_env(env); // 执行环境无论成败都销毁
            if let Err(e) = result {
                // 异常文本优先（比 call_wasm=false 更可诊断）
                let exception = sys::get_exception(inst).unwrap_or(None);
                return Err(WamrError::Sys(sys::SysError::CallFailed(
                    exception.unwrap_or_else(|| e.to_string()),
                )));
            }
            Ok(())
        }
        #[cfg(not(feature = "wamr-link"))]
        {
            let _ = (handle, func, argv, spec);
            Err(WamrError::NotLinked)
        }
    }

    /// 实例当前异常文本（无异常 = None）。
    pub fn guest_exception(&self, handle: InstanceHandle) -> WamrResult<Option<String>> {
        self.require_active()?;
        #[cfg(feature = "wamr-link")]
        {
            let inst = *self
                .raw_insts
                .get(&handle.0)
                .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
            sys::get_exception(inst).map_err(WamrError::from)
        }
        #[cfg(not(feature = "wamr-link"))]
        {
            let _ = handle;
            Err(WamrError::NotLinked)
        }
    }

    /// 读 Guest 线性内存（复制语义；native 指针不外泄）。
    pub fn read_guest_memory(
        &self,
        handle: InstanceHandle,
        offset: u64,
        len: u32,
    ) -> WamrResult<Vec<u8>> {
        self.require_active()?;
        #[cfg(feature = "wamr-link")]
        {
            let inst = *self
                .raw_insts
                .get(&handle.0)
                .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
            sys::read_guest_memory(inst, offset, len).map_err(WamrError::from)
        }
        #[cfg(not(feature = "wamr-link"))]
        {
            let _ = (handle, offset, len);
            Err(WamrError::NotLinked)
        }
    }

    /// 写 Guest 线性内存（复制语义；校验+写一步完成）。
    pub fn write_guest_memory(
        &self,
        handle: InstanceHandle,
        offset: u64,
        bytes: &[u8],
    ) -> WamrResult<()> {
        self.require_active()?;
        #[cfg(feature = "wamr-link")]
        {
            let inst = *self
                .raw_insts
                .get(&handle.0)
                .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
            sys::write_guest_memory(inst, offset, bytes).map_err(WamrError::from)
        }
        #[cfg(not(feature = "wamr-link"))]
        {
            let _ = (handle, offset, bytes);
            Err(WamrError::NotLinked)
        }
    }

    /// Guest 内存区间校验（Host ABI 数据面用；offset+size 必须落在线性内存内）。
    pub fn validate_guest_memory(
        &self,
        handle: InstanceHandle,
        offset: u64,
        size: u64,
    ) -> WamrResult<bool> {
        self.require_active()?;
        #[cfg(feature = "wamr-link")]
        {
            let inst = *self
                .raw_insts
                .get(&handle.0)
                .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
            sys::validate_app_addr(inst, offset, size).map_err(WamrError::from)
        }
        #[cfg(not(feature = "wamr-link"))]
        {
            let _ = (handle, offset, size);
            Err(WamrError::NotLinked)
        }
    }

    fn require_active(&self) -> WamrResult<()> {
        if self.active {
            Ok(())
        } else {
            Err(WamrError::Busy)
        }
    }
}

impl Drop for RuntimeHost {
    /// 兜底（不假设显式 shutdown 一定发生——AGENTS.md §2.5）。
    /// 显式 shutdown 是正道；Drop 只做尽力清理并大声记录，不吞错。
    fn drop(&mut self) {
        if self.active {
            tracing::error!("RuntimeHost 未显式 shutdown 即被 drop——执行兜底清理");
            let _ = self.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::InstanceSpec;
    use std::sync::Mutex;

    // RuntimeHost 是进程级单例——所有生命周期测试共享一把锁串行执行
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn stub_cfg() -> RuntimeConfig {
        RuntimeConfig::default() // Development 档
    }

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn init_shutdown_lifecycle_and_singleton() {
        // 真 WAMR 下 destroy→re-init 不安全（SIGSEGV 实证）——
        // 单元测试只测簿记语义（stub 档）；真实生命周期由 e2e_nlu 单次覆盖
        if sys::linked() {
            eprintln!("skip: WAMR 生命周期由 CI E2E 覆盖");
            return;
        }
        let _g = guard();
        let mut host = RuntimeHost::init(stub_cfg()).unwrap();
        assert!(host.is_active());
        // 进程级单例：二次 init 被拒
        assert!(matches!(
            RuntimeHost::init(stub_cfg()),
            Err(WamrError::AlreadyInitialized)
        ));
        host.shutdown().unwrap();
        assert!(!host.is_active());

        // shutdown 后可重新 init（RUNNING 已复位）——仅 stub 档：
        // 真 WAMR 的 destroy→re-init 循环不属于本层契约（进程一次生命周期），
        // 循环测试在真库下 SIGSEGV（CI 实证）；单次生命周期由 CI E2E 覆盖。
        if sys::linked() {
            return;
        }
        let mut host2 = RuntimeHost::init(stub_cfg()).unwrap();
        assert!(matches!(
            host2.instantiate(
                "nope",
                InstanceSpec {
                    module_name: "nope".into(),
                    component_id: "nope".into(),
                    stack_size: 0,
                    heap_size: 0,
                    memory_limit: 64
                }
            ),
            Err(WamrError::NoSuchModule(_))
        ));
        host2.shutdown().unwrap();
    }

    #[test]
    fn shutdown_is_idempotent_and_allows_reinit() {
        if sys::linked() {
            eprintln!("skip: WAMR 生命周期由 CI E2E 覆盖");
            return;
        }
        let _g = guard();
        let mut host = RuntimeHost::init(stub_cfg()).unwrap();
        host.shutdown().unwrap();
        host.shutdown().unwrap(); // 幂等
        if !sys::linked() {
            let mut again = RuntimeHost::init(stub_cfg()).unwrap();
            assert!(again.is_active());
            again.shutdown().unwrap();
        }
        drop(host); // 已停用宿主的 drop 不再触碰 RUNNING
    }

    #[test]
    fn drop_backstop_resets_singleton() {
        if sys::linked() {
            eprintln!("skip: WAMR 生命周期由 CI E2E 覆盖");
            return;
        }
        let _g = guard();
        // 不 shutdown 直接 drop：兜底清理 + 单例释放（stub 档）
        {
            let _host = RuntimeHost::init(stub_cfg()).unwrap();
        }
        // drop 后可重新 init（RUNNING 已复位）
        let mut host = RuntimeHost::init(stub_cfg()).unwrap();
        host.shutdown().unwrap();
    }

    #[test]
    fn stub_mode_execution_ops_fail_cleanly() {
        if sys::linked() {
            return; // wamr-link 档不测此路径（CI 有真实库）
        }
        let _g = guard();
        let mut host = RuntimeHost::init(stub_cfg()).unwrap();
        let err = host.instantiate(
            "nlu",
            InstanceSpec {
                module_name: "nlu".into(),
                component_id: "nlu".into(),
                stack_size: 0,
                heap_size: 0,
                memory_limit: 64,
            },
        );
        assert!(matches!(err, Err(WamrError::NoSuchModule(_)))); // stub 连模块都加载不了
        assert!(matches!(
            host.unload_module("nlu"),
            Err(WamrError::NoSuchModule(_))
        ));
        host.shutdown().unwrap();
    }

    #[test]
    fn config_defaults_are_bounded() {
        let cfg = RuntimeConfig::default();
        assert!(cfg.memory_budget > 0, "全局预算必须默认有界");
        assert!(cfg.per_instance_memory_limit > 0, "单实例上限必须默认有界");
        assert!(cfg.per_instance_memory_limit <= cfg.memory_budget);
        assert!(cfg.stack_size > 0);
    }
}

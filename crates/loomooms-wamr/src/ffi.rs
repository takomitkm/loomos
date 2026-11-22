//! WAMR C API FFI 声明（`wamr-link` feature 门控；需 IWASM_LIB_DIR）。
//!
//! 签名与 nlu-bench 验证过的调用一致（WAMR 2.x C API）。

use super::error::{WamrError, WamrResult};
use super::runtime::RuntimeConfig;

// WAMR 类型别名（不透明指针）
pub type WasmModule = *mut std::ffi::c_void;
pub type WasmModuleInst = *mut std::ffi::c_void;
pub type WasmExecEnv = *mut std::ffi::c_void;
pub type WasmFunction = *mut std::ffi::c_void;

#[repr(C)]
pub struct WasmRuntimeInitArgs {
    pub mem_alloc_type: u32,
    pub mem_alloc_option: [u8; 24],
    pub native_module_name: *const u8,
    pub native_symbols: *const u8,
    pub n_native_symbols: u32,
    pub max_thread_num: u32,
    pub ip_addr: [u8; 128],
    pub unused: i32,
    pub instance_port: i32,
    pub fast_jit_code_cache_size: u32,
    pub gc_heap_size: u32,
    pub running_mode: u32,
    pub llvm_jit_opt_level: u32,
    pub segue_flags: u32,
    pub enable_linux_perf: bool,
}

const ALLOC_WITH_SYSTEM_ALLOCATOR: u32 = 2;

#[link(name = "iwasm")]
unsafe extern "C" {
    pub fn wasm_runtime_full_init(init_args: *mut WasmRuntimeInitArgs) -> bool;
    pub fn wasm_runtime_destroy();
    pub fn wasm_runtime_load(
        buf: *mut u8,
        size: u32,
        error_buf: *mut u8,
        error_buf_size: u32,
    ) -> WasmModule;
    pub fn wasm_runtime_unload(module: WasmModule);
    pub fn wasm_runtime_instantiate(
        module: WasmModule,
        stack_size: u32,
        heap_size: u32,
        error_buf: *mut u8,
        error_buf_size: u32,
    ) -> WasmModuleInst;
    pub fn wasm_runtime_deinstantiate(inst: WasmModuleInst);
    pub fn wasm_runtime_create_exec_env(inst: WasmModuleInst, stack_size: u32) -> WasmExecEnv;
    pub fn wasm_runtime_destroy_exec_env(exec_env: WasmExecEnv);
    pub fn wasm_runtime_lookup_function(inst: WasmModuleInst, name: *const u8) -> WasmFunction;
    pub fn wasm_runtime_call_wasm(
        exec_env: WasmExecEnv,
        function: WasmFunction,
        argc: u32,
        argv: *mut u32,
    ) -> bool;
    pub fn wasm_runtime_get_exception(inst: WasmModuleInst) -> *const u8;
}

pub fn full_init(config: &RuntimeConfig) -> WamrResult<()> {
    let mut init_args = unsafe { std::mem::zeroed::<WasmRuntimeInitArgs>() };
    init_args.mem_alloc_type = ALLOC_WITH_SYSTEM_ALLOCATOR;
    // SAFETY: init_args 为栈上合法 repr(C) 结构
    let ok = unsafe { wasm_runtime_full_init(&mut init_args) };
    if !ok {
        return Err(WamrError::ModuleLoad("wasm_runtime_full_init 失败".into()));
    }
    let _ = config;
    Ok(())
}

pub fn destroy() {
    // SAFETY: WAMR 全局销毁
    unsafe { wasm_runtime_destroy() };
}

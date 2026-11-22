//! loomos-wamr-sys：WAMR C API 最小 unsafe 绑定——**Wasm 执行的可信边界**。
//!
//! **全仓第三个声明式可信边界 crate**（与 `loomos-sys` / `loomos-ort-sys` 同地位；
//! 其余 crate `#![forbid(unsafe_code)]`）。ADR-011 v1.1 §修订记录 #5：
//! raw WAMR FFI 只存在于本 crate，`loomos-wamr` 门面层不接触 unsafe。
//!
//! 来源对齐：
//! - 签名与 `loomos-nlu-bench` 已在 CI/QEMU 实测通过（WAMR interp/AOT、checksum 全一致）；
//! - 结构体布局与 WAMR main@06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf 的
//!   `wasm_export.h` 逐字段核对（RuntimeInitArgs.mem_alloc_option 联合体最大
//!   32 字节——bench 的 [u8;24] 靠零填充侥幸，本 crate 用正确 32）；
//! - AOT 魔数/版本来自 `core/config.h`：AOT_MAGIC_NUMBER=0x746f6100（"\0aot"）、
//!   AOT_CURRENT_VERSION=6。
//!
//! 两档模式：
//! - `link` feature：静态链接 libiwasm.a（IWASM_LIB_DIR 定位；nix build .#loomos-wamr 产出）；
//! - 默认：全部包装函数返回 `SysError::NotLinked`（stub 档，crate 始终可编译）。
//!
//! 所有包装函数都是**安全函数**：指针有效性由本 crate 内部保证并文档化，
//! 调用方（loomos-wamr）以不透明句柄持有返回的裸指针，永不解引用。
//!
//! 证据纪律：调用失败必带 WAMR 错误缓冲文本（error_buf），不做「成功假设」。

#![deny(unsafe_op_in_unsafe_fn)]
// -sys crate 约定：公开包装函数内部解引用裸指针（有效性由实现内部保证并文档化），
// clippy 的保守 lint 在此显式豁免——安全边界就是本 crate。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::{c_char, c_void};

/// WAMR 模块句柄（wasm_module_t；由 wasm_runtime_load 返回，永不解引用）。
pub type WasmModule = *mut c_void;
/// WAMR 模块实例句柄（wasm_module_inst_t）。
pub type WasmModuleInst = *mut c_void;
/// WAMR 执行环境句柄（wasm_exec_env_t）。
pub type WasmExecEnv = *mut c_void;
/// WAMR 函数句柄（wasm_function_inst_t）。
pub type WasmFunction = *mut c_void;

/// mem_alloc_type：Alloc_With_System_Allocator（wasm_export.h 枚举第三值 = 2）。
pub const ALLOC_WITH_SYSTEM_ALLOCATOR: u32 = 2;

/// NativeSymbol（lib_export.h）：Host ABI 原生符号注册条目（P2）。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NativeSymbol {
    pub symbol: *const c_char,
    pub func_ptr: *mut c_void,
    pub signature: *const c_char,
}

/// RuntimeInitArgs（wasm_export.h，WAMR main@06f6c8c 逐字段核对）。
/// mem_alloc_option 是联合体：pool{ptr,4} / allocator{4 ptr} → 最大 32 字节。
#[repr(C)]
pub struct RuntimeInitArgs {
    pub mem_alloc_type: u32,
    pub mem_alloc_option: [u8; 32],
    pub native_module_name: *const c_char,
    pub native_symbols: *mut NativeSymbol,
    pub n_native_symbols: u32,
    pub max_thread_num: u32,
    pub ip_addr: [u8; 128],
    pub unused: i32,
    pub instance_port: i32,
    pub fast_jit_code_cache_size: u32,
    pub gc_heap_size: u32,
    pub running_mode: u32,
    pub llvm_jit_opt_level: u32,
    pub llvm_jit_size_level: u32,
    pub segue_flags: u32,
    pub enable_linux_perf: bool,
}

impl Default for RuntimeInitArgs {
    fn default() -> Self {
        Self {
            mem_alloc_type: 0,
            mem_alloc_option: [0; 32],
            native_module_name: std::ptr::null(),
            native_symbols: std::ptr::null_mut(),
            n_native_symbols: 0,
            max_thread_num: 0,
            ip_addr: [0; 128],
            unused: 0,
            instance_port: 0,
            fast_jit_code_cache_size: 0,
            gc_heap_size: 0,
            running_mode: 0,
            llvm_jit_opt_level: 0,
            llvm_jit_size_level: 0,
            segue_flags: 0,
            enable_linux_perf: false,
        }
    }
}

impl RuntimeInitArgs {
    /// 系统分配器档（nlu-bench 实测使用；无自定义内存池）。
    pub fn system_allocator() -> Self {
        Self {
            mem_alloc_type: ALLOC_WITH_SYSTEM_ALLOCATOR,
            ..Default::default()
        }
    }
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum SysError {
    #[error("link feature 未启用（stub 档，无 WAMR 执行能力）")]
    NotLinked,
    #[error("WAMR 运行时初始化失败: {0}")]
    InitFailed(String),
    #[error("WAMR 模块加载失败: {0}")]
    LoadFailed(String),
    #[error("WAMR 实例化失败: {0}")]
    InstantiateFailed(String),
    #[error("WAMR 执行环境创建失败")]
    ExecEnvFailed,
    #[error("函数未找到: {0}")]
    FunctionNotFound(String),
    #[error("Wasm 调用失败: {0}")]
    CallFailed(String),
}

#[cfg(feature = "link")]
const ERROR_BUF: usize = 128;

#[cfg(feature = "link")]
mod raw {
    use super::{
        NativeSymbol, RuntimeInitArgs, WasmExecEnv, WasmFunction, WasmModule, WasmModuleInst,
        c_char, c_void,
    };

    #[link(name = "iwasm")]
    extern "C" {
        pub fn wasm_runtime_full_init(init_args: *mut RuntimeInitArgs) -> bool;
        pub fn wasm_runtime_destroy();
        pub fn wasm_runtime_load(
            buf: *mut u8,
            size: u32,
            error_buf: *mut u8,
            error_buf_size: u32,
        ) -> WasmModule;
        pub fn wasm_runtime_unload(module: WasmModule);
        pub fn wasm_runtime_set_wasi_args(
            module: WasmModule,
            dir_list: *const *const c_char,
            dir_count: u32,
            map_dir_list: *const *const c_char,
            map_dir_count: u32,
            env: *const *const c_char,
            env_count: u32,
            argv: *const *const c_char,
            argc: i32,
        );
        pub fn wasm_runtime_instantiate(
            module: WasmModule,
            stack_size: u32,
            heap_size: u32,
            error_buf: *mut u8,
            error_buf_size: u32,
        ) -> WasmModuleInst;
        pub fn wasm_runtime_deinstantiate(inst: WasmModuleInst);
        pub fn wasm_runtime_lookup_function(
            inst: WasmModuleInst,
            name: *const c_char,
        ) -> WasmFunction;
        pub fn wasm_runtime_create_exec_env(inst: WasmModuleInst, stack_size: u32) -> WasmExecEnv;
        pub fn wasm_runtime_destroy_exec_env(exec_env: WasmExecEnv);
        pub fn wasm_runtime_call_wasm(
            exec_env: WasmExecEnv,
            function: WasmFunction,
            argc: u32,
            argv: *mut u32,
        ) -> bool;
        pub fn wasm_runtime_get_exception(inst: WasmModuleInst) -> *const c_char;
        pub fn wasm_runtime_addr_app_to_native(
            inst: WasmModuleInst,
            app_offset: u64,
        ) -> *mut c_void;
        pub fn wasm_runtime_validate_app_addr(
            inst: WasmModuleInst,
            app_offset: u64,
            size: u64,
        ) -> bool;
        pub fn wasm_runtime_register_natives(
            module_name: *const c_char,
            native_symbols: *mut NativeSymbol,
            n_native_symbols: u32,
        ) -> bool;
    }
}

// ---------------------------------------------------------------------------
// 安全包装层（link / stub 两档同签名）
// ---------------------------------------------------------------------------

/// 是否真实链接了 WAMR（stub 档 = false，无执行能力）。
pub fn linked() -> bool {
    cfg!(feature = "link")
}

/// 进程级运行时初始化（WAMR 全局单次；重复初始化由上层 loomos-wamr 守护）。
pub fn full_init(args: &mut RuntimeInitArgs) -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: args 是调用方持有的合法 repr(C) 结构（本 crate 定义/构造），
        // wasm_runtime_full_init 只读取并内部复制，返回后指针仍有效。
        if unsafe { raw::wasm_runtime_full_init(args) } {
            Ok(())
        } else {
            Err(SysError::InitFailed(
                "wasm_runtime_full_init 返回 false".into(),
            ))
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = args;
        Err(SysError::NotLinked)
    }
}

/// 进程级运行时销毁（须先确保全部实例/模块已卸载——顺序由上层守护）。
pub fn destroy() -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: 全局销毁；调用前提（无存活实例/模块）由上层 loomos-wamr 的
        // shutdown 顺序保证。WAMR 内部依赖该前提，违者行为未定义——上层有测试。
        unsafe { raw::wasm_runtime_destroy() };
        Ok(())
    }
    #[cfg(not(feature = "link"))]
    {
        Err(SysError::NotLinked)
    }
}

/// 加载 Wasm/AOT 模块。**buf 在模块卸载前必须保持存活**
/// （WAMR runtime 可能引用其内容——nlu-bench 实测约定）。
pub fn load(buf: &mut [u8]) -> Result<WasmModule, SysError> {
    #[cfg(feature = "link")]
    {
        let mut err = [0u8; ERROR_BUF];
        // SAFETY: buf 指向调用方持有的可写缓冲，长度一致；错误缓冲栈上合法。
        // 返回句柄被上层与 buf 同生命周期持有（见 fn 契约）。
        let module = unsafe {
            raw::wasm_runtime_load(
                buf.as_mut_ptr(),
                buf.len() as u32,
                err.as_mut_ptr(),
                ERROR_BUF as u32,
            )
        };
        if module.is_null() {
            Err(SysError::LoadFailed(cstr_at(&err)))
        } else {
            Ok(module)
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = buf;
        Err(SysError::NotLinked)
    }
}

/// 卸载模块（须无存活实例；顺序由上层守护）。
pub fn unload(module: WasmModule) -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: module 来自 load 返回且上层保证无存活实例后传入。
        unsafe { raw::wasm_runtime_unload(module) };
        Ok(())
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = module;
        Err(SysError::NotLinked)
    }
}

/// WASI P1 参数绑定（load 后、instantiate 前；本版本 API 无返回值）。
pub fn set_wasi_args(module: WasmModule, argv: &[*const c_char]) {
    #[cfg(feature = "link")]
    {
        // SAFETY: module 来自 load；argv 为调用方持有的 CStr 指针数组，
        // 调用期间 WAMR 只读取不持有（内部在 instantiate 时复制）。
        unsafe {
            raw::wasm_runtime_set_wasi_args(
                module,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                argv.as_ptr(),
                argv.len() as i32,
            );
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (module, argv);
    }
}

/// 实例化模块。stack_size/heap_size 字节（heap 0 = Rust 模块自管线性内存，bench 实测约定）。
pub fn instantiate(
    module: WasmModule,
    stack_size: u32,
    heap_size: u32,
) -> Result<WasmModuleInst, SysError> {
    #[cfg(feature = "link")]
    {
        let mut err = [0u8; ERROR_BUF];
        // SAFETY: module 来自 load；错误缓冲栈上合法。
        let inst = unsafe {
            raw::wasm_runtime_instantiate(
                module,
                stack_size,
                heap_size,
                err.as_mut_ptr(),
                ERROR_BUF as u32,
            )
        };
        if inst.is_null() {
            Err(SysError::InstantiateFailed(cstr_at(&err)))
        } else {
            Ok(inst)
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (module, stack_size, heap_size);
        Err(SysError::NotLinked)
    }
}

/// 反实例化（实例句柄此后失效）。
pub fn deinstantiate(inst: WasmModuleInst) -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: inst 来自 instantiate 且上层保证此后不再使用。
        unsafe { raw::wasm_runtime_deinstantiate(inst) };
        Ok(())
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = inst;
        Err(SysError::NotLinked)
    }
}

/// 查找导出函数（2 参形态与 WAMR main@06f6c8c 的 wasm_export.h 一致）。
pub fn lookup_function(inst: WasmModuleInst, name: &str) -> Result<WasmFunction, SysError> {
    #[cfg(feature = "link")]
    {
        let name = cstr(name)?;
        // SAFETY: inst 来自 instantiate；name 以 NUL 结尾且在调用期间有效。
        let func = unsafe { raw::wasm_runtime_lookup_function(inst, name.as_ptr()) };
        if func.is_null() {
            Err(SysError::FunctionNotFound(
                name.to_string_lossy().into_owned(),
            ))
        } else {
            Ok(func)
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (inst, name);
        Err(SysError::NotLinked)
    }
}

/// 创建执行环境（每线程独立；调用结束由 destroy_exec_env 释放）。
pub fn create_exec_env(inst: WasmModuleInst, stack_size: u32) -> Result<WasmExecEnv, SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: inst 来自 instantiate。
        let env = unsafe { raw::wasm_runtime_create_exec_env(inst, stack_size) };
        if env.is_null() {
            Err(SysError::ExecEnvFailed)
        } else {
            Ok(env)
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (inst, stack_size);
        Err(SysError::NotLinked)
    }
}

/// 销毁执行环境。
pub fn destroy_exec_env(exec_env: WasmExecEnv) -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: env 来自 create_exec_env 且只销毁一次（上层单线程路径保证）。
        unsafe { raw::wasm_runtime_destroy_exec_env(exec_env) };
        Ok(())
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = exec_env;
        Err(SysError::NotLinked)
    }
}

/// 调用 Wasm 函数。**返回值在 argv[0]**（nlu-bench 实测约定，非 argv[argc]）。
/// argv 为参数/返回值的共享单元缓冲（i64/f64 占 2 单元——调用方按 WAMR 签名布局准备）。
pub fn call_wasm(
    exec_env: WasmExecEnv,
    function: WasmFunction,
    argv: &mut [u32],
) -> Result<(), SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: exec_env/function 来自本 crate 的 create/lookup；argv 合法缓冲。
        let ok = unsafe {
            raw::wasm_runtime_call_wasm(exec_env, function, argv.len() as u32, argv.as_mut_ptr())
        };
        if ok {
            Ok(())
        } else {
            Err(SysError::CallFailed(
                "wasm_runtime_call_wasm 返回 false".into(),
            ))
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (exec_env, function, argv);
        Err(SysError::NotLinked)
    }
}

/// 实例异常文本（无异常 = None）。
pub fn get_exception(inst: WasmModuleInst) -> Result<Option<String>, SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: inst 来自 instantiate；返回指针由 WAMR 拥有（生命周期至下次
        // 同实例操作——立即复制为 String，不保留）。
        let p = unsafe { raw::wasm_runtime_get_exception(inst) };
        if p.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe_cstr_to_string(p)))
        }
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = inst;
        Err(SysError::NotLinked)
    }
}

/// Guest 地址映射（app offset → native 指针）。越界返回 null——**只映射不校验长度**，
/// 长度校验用 [`validate_app_addr`]。
pub fn addr_app_to_native(inst: WasmModuleInst, app_offset: u64) -> Result<*mut c_void, SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: inst 来自 instantiate；返回指针只在本次调用语义内有效（WAMR 拥有）。
        Ok(unsafe { raw::wasm_runtime_addr_app_to_native(inst, app_offset) })
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (inst, app_offset);
        Err(SysError::NotLinked)
    }
}

/// Guest 内存区间校验（offset..offset+size 必须落在线性内存内）。
pub fn validate_app_addr(
    inst: WasmModuleInst,
    app_offset: u64,
    size: u64,
) -> Result<bool, SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: inst 来自 instantiate。
        Ok(unsafe { raw::wasm_runtime_validate_app_addr(inst, app_offset, size) })
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (inst, app_offset, size);
        Err(SysError::NotLinked)
    }
}

/// 注册原生符号表（Host ABI；module_name 用 "loomos" 命名空间，NULL = 全局注册）。
pub fn register_natives(
    module_name: *const c_char,
    symbols: &mut [NativeSymbol],
) -> Result<bool, SysError> {
    #[cfg(feature = "link")]
    {
        // SAFETY: symbols 为调用方持有的表；WAMR 在注册时复制或长期引用——
        // 上层（loomos-wamr Host ABI 注册器）保证表在运行时全程存活。
        Ok(unsafe {
            raw::wasm_runtime_register_natives(
                module_name,
                symbols.as_mut_ptr(),
                symbols.len() as u32,
            )
        })
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = (module_name, symbols);
        Err(SysError::NotLinked)
    }
}

// ---------------------------------------------------------------------------
// 内部工具（本 crate 内 unsafe；对外全部安全接口）
// ---------------------------------------------------------------------------

#[cfg(feature = "link")]
fn cstr(s: &str) -> Result<std::borrow::Cow<'_, [u8]>, SysError> {
    use std::borrow::Cow;
    let v: Cow<'_, [u8]> = Cow::Borrowed(s.as_bytes());
    if v.ends_with(&[0]) {
        Ok(v)
    } else {
        let mut w = v.into_owned();
        w.push(0);
        Ok(Cow::Owned(w))
    }
}

#[cfg(feature = "link")]
fn cstr_at(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

#[cfg(feature = "link")]
/// SAFETY: p 必须为 NUL 结尾的合法 C 字符串（WAMR 返回的错误/异常文本满足）。
fn unsafe_cstr_to_string(p: *const c_char) -> String {
    let mut len = 0usize;
    // SAFETY(前置): 契约见 fn 文档；只读扫描到 NUL 为止。
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(p.cast::<u8>(), len);
        String::from_utf8_lossy(slice).into_owned()
    }
}

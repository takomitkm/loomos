//! nlu-wasm-wrap —— loomos-nlu（业务代码，原样不动）的 C ABI 胶水层。
//!
//! 约束：loomos-nlu 本身零改动；本 crate 只做两件事
//!   1) 用默认注册表（对齐旧板 3 区域 + 6 实体）构造 RuleNlu；
//!   2) 暴露 `ab_parse(in_ptr, in_len) -> out_len`：解析 + serde_json 序列化，
//!      与 native 模式 harness 的「业务路径」完全一致（parse → JSON 字节）。
//!
//! 内存纪律：输入/输出都是常驻线性内存缓冲（一次性分配，永不 realloc），
//! 避免把分配抖动混进解析耗时；宿主通过 ab_alloc_input/ab_out_ptr 取指针。
//! ab_init_registry 必须先于 ab_alloc_input/ab_parse 调用。

#![allow(static_mut_refs)]

use loomos_nlu::{EntityRegistry, RuleNlu};

static mut NLU: Option<RuleNlu> = None;
static mut IN_BUF: Vec<u8> = Vec::new();
static mut OUT_BUF: Vec<u8> = Vec::new();
const OUT_CAP: usize = 8192;

#[unsafe(no_mangle)]
pub extern "C" fn ab_init_registry() -> i32 {
    unsafe {
        NLU = Some(RuleNlu::new(EntityRegistry::default_home()));
        if OUT_BUF.capacity() == 0 {
            OUT_BUF.resize(OUT_CAP, 0);
        }
        0
    }
}

/// 输入缓冲：按需一次性增长到 len（之后指针稳定）。
#[unsafe(no_mangle)]
pub extern "C" fn ab_alloc_input(len: i32) -> i32 {
    unsafe {
        if (len as usize) > IN_BUF.len() {
            IN_BUF.resize(len as usize, 0);
        }
        IN_BUF.as_ptr() as i32
    }
}

/// 输出缓冲指针（ab_init 之后容量固定为 OUT_CAP，指针稳定）。
#[unsafe(no_mangle)]
pub extern "C" fn ab_out_ptr() -> i32 {
    unsafe { OUT_BUF.as_ptr() as i32 }
}

/// 业务路径：parse + JSON 序列化，返回写入 OUT_BUF 的字节数；负数为错误码。
#[unsafe(no_mangle)]
pub extern "C" fn ab_parse(in_ptr: i32, in_len: i32) -> i32 {
    unsafe {
        let Some(nlu) = NLU.as_ref() else {
            return -1; // 未初始化
        };
        let bytes = std::slice::from_raw_parts(in_ptr as *const u8, in_len as usize);
        let Ok(text) = std::str::from_utf8(bytes) else {
            return -2; // 非 UTF-8
        };
        let decision: loomos_nlu::NluDecision = nlu.parse(text);
        match serde_json::to_vec(&decision) {
            Ok(json) => {
                if json.len() > OUT_BUF.len() {
                    return -3; // 超输出缓冲
                }
                OUT_BUF[..json.len()].copy_from_slice(&json);
                json.len() as i32
            }
            Err(_) => -4,
        }
    }
}

// ---------------------------------------------------------------------------
// Host ABI 客户端（ADR-011 §6）：Guest 侧唯一系统入口。
//
// 对偶关系：Native 侧 raw FFI 集中在 loomis-wamr-sys（可信边界③）；Guest 侧
// 的执行边界就是这里——`#[link(wasm_import_module = "loomos")]` 导入由 WAMR
// 在实例化时与注册符号绑定，**Guest 无法绕过**（不存在其它导入通道）。
// 错误约定与 HOST_ABI_V1 一致：≥0 成功（句柄/长度），<0 = -稳定错误码。
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod host_abi_client {
    #[link(wasm_import_module = "loomos")]
    unsafe extern "C" {
        pub fn log_v1(level: i32, ptr: i32, len: i32);
        pub fn time_mono_v1() -> i64;
        pub fn time_wall_v1() -> i64;
        pub fn config_get_v1(
            ns_ptr: i32,
            ns_len: i32,
            key_ptr: i32,
            key_len: i32,
            out_ptr: i32,
            out_cap: i32,
        ) -> i32;
        pub fn infer_submit_v1(model_ptr: i32, model_len: i32, buffer: i32) -> i32;
        pub fn infer_poll_v1(op: i32) -> i32;
        pub fn device_op_v1(
            dev_ptr: i32,
            dev_len: i32,
            op_ptr: i32,
            op_len: i32,
            params_ptr: i32,
            params_len: i32,
        ) -> i32;
        pub fn device_poll_v1(op: i32) -> i32;
        pub fn buffer_import_v1(size: i32, align: i32) -> i32;
        pub fn buffer_release_v1(h: i32) -> i32;
        pub fn event_subscribe_v1(ev_ptr: i32, ev_len: i32) -> i32;
    }
}

/// 稳定错误码镜像（与 loomis-wamr host_abi::code 一致；数字即 ABI）。
pub mod abi_code {
    pub const OK: u32 = 0;
    pub const ERR_INVALID: u32 = 1;
    pub const ERR_CAPABILITY: u32 = 2;
    pub const ERR_MEMORY: u32 = 3;
    pub const ERR_PAYLOAD: u32 = 4;
    pub const ERR_HANDLE: u32 = 5;
    pub const ERR_UNSUPPORTED: u32 = 6;
    pub const ERR_BUSY: u32 = 7;
    pub const ERR_UNLINKED: u32 = 8;
    pub const ERR_UNKNOWN: u32 = 9;
    pub const ERR_CANCELLED: u32 = 10;
}

#[cfg(target_arch = "wasm32")]
pub mod abi {
    //! Host ABI 安全封装（wasm32 目标；错误 ≥0 成功 / <0 = -码）。

    use super::abi_code;
    use super::host_abi_client as raw;

    /// 结构化日志（fire-and-forget）。
    pub fn log(level: u32, msg: &str) {
        let bytes = msg.as_bytes();
        // SAFETY: 线性内存内的缓冲 + 长度一致；WAMR 只在调用期读取。
        unsafe { raw::log_v1(level as i32, bytes.as_ptr() as i32, bytes.len() as i32) }
    }

    pub fn time_mono() -> i64 {
        unsafe { raw::time_mono_v1() }
    }

    pub fn time_wall() -> i64 {
        unsafe { raw::time_wall_v1() }
    }

    /// 配置只读：写入 out，返回长度；key 缺失 = Ok(0)（Host 返回 JSON null 4 字节？——
    /// Host 约定 miss 返回 null 字面量 4 字节；此处按实际返回长度处理）。
    pub fn config_get(ns: &str, key: &str, out: &mut [u8]) -> Result<usize, i32> {
        let (ns_ptr, ns_len) = ptr_len(ns);
        let (key_ptr, key_len) = ptr_len(key);
        // SAFETY: out 是 Guest 线性内存内的合法缓冲。
        let r = unsafe {
            raw::config_get_v1(
                ns_ptr,
                ns_len,
                key_ptr,
                key_len,
                out.as_ptr() as i32,
                out.len() as i32,
            )
        };
        decode_len(r, out.len())
    }

    /// 设备操作提交 → op 句柄。
    pub fn device_op(device: &str, op: &str, params_json: &str) -> Result<u64, i32> {
        let (dev_ptr, dev_len) = ptr_len(device);
        let (op_ptr, op_len) = ptr_len(op);
        let (params_ptr, params_len) = ptr_len(params_json);
        // SAFETY: 参数串都在线性内存内。
        let r =
            unsafe { raw::device_op_v1(dev_ptr, dev_len, op_ptr, op_len, params_ptr, params_len) };
        decode_handle(r)
    }

    pub fn device_poll(op_handle: u64) -> Result<u64, i32> {
        // 返回 (状态码语义由调用方解释)；这里透传稳定码
        let r = unsafe { raw::device_poll_v1(op_handle as i32) };
        if r >= 0 { Ok(r as u64) } else { Err(r) }
    }

    pub fn infer_submit(model_id: &str, buffer_handle: u64) -> Result<u64, i32> {
        let (ptr, len) = ptr_len(model_id);
        // SAFETY: 同上。
        let r = unsafe { raw::infer_submit_v1(ptr, len, buffer_handle as i32) };
        decode_handle(r)
    }

    pub fn infer_poll(op_handle: u64) -> Result<u64, i32> {
        let r = unsafe { raw::infer_poll_v1(op_handle as i32) };
        if r >= 0 { Ok(r as u64) } else { Err(r) }
    }

    pub fn buffer_import(size: u32, align: u32) -> Result<u64, i32> {
        let r = unsafe { raw::buffer_import_v1(size as i32, align as i32) };
        decode_handle(r)
    }

    pub fn buffer_release(handle: u64) -> Result<(), i32> {
        let r = unsafe { raw::buffer_release_v1(handle as i32) };
        if r >= 0 { Ok(()) } else { Err(r) }
    }

    pub fn event_subscribe(event: &str) -> Result<u64, i32> {
        let (ptr, len) = ptr_len(event);
        let r = unsafe { raw::event_subscribe_v1(ptr, len) };
        decode_handle(r)
    }

    fn ptr_len(s: &str) -> (i32, i32) {
        (s.as_ptr() as i32, s.len() as i32) // wasm32：线性内存指针即 i32
    }

    fn decode_len(r: i32, cap: usize) -> Result<usize, i32> {
        if r >= 0 {
            if r as usize > cap {
                return Err(-(abi_code::ERR_MEMORY as i32)); // Host 越界写（防御）
            }
            Ok(r as usize)
        } else {
            Err(r)
        }
    }

    fn decode_handle(r: i32) -> Result<u64, i32> {
        if r >= 0 { Ok(r as u64) } else { Err(r) }
    }
}

#[cfg(target_arch = "wasm32")]
const CONFIG_OUT_CAP: usize = 4096;

/// 经 Host ABI 的业务路径（奇偶校验框架）：与 ab_parse 语义一致（parse → JSON），
/// 但执行中通过 loomis.log_v1 上报调用（wamr-link 档 CI 校验两条路径输出逐字节一致）。
///
/// 输入与 ab_parse 相同（线性内存缓冲指针）；返回写入 OUT_BUF 的长度。
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn ab_parse_via_host(in_ptr: i32, in_len: i32) -> i32 {
    // 1. 复用同一业务实现（业务代码零改动纪律；ab_* 导出本身是安全函数）
    let out_len = ab_parse(in_ptr, in_len);
    // 2. 经 Host 通道上报（能力校验在 Host 侧——组件无 log 权会静默丢弃）
    if out_len > 0 {
        // SAFETY: OUT_BUF 常驻线性内存；len 由 ab_parse 保证 ≤ OUT_CAP。
        let bytes = unsafe { out_bytes(out_len as usize) };
        abi::log(2, std::str::from_utf8(bytes).unwrap_or(""));
    }
    out_len
}

// SAFETY 契约由调用方保证：OUT_BUF 常驻线性内存且 len ≤ OUT_CAP。
#[cfg(target_arch = "wasm32")]
unsafe fn out_bytes(len: usize) -> &'static [u8] {
    let ptr = ab_out_ptr() as *const u8;
    std::slice::from_raw_parts(ptr, len)
}

/// 非 wasm 目标的占位说明（宿主侧本 crate 仍可编译——ab_* 直调路径供 native harness）。
#[cfg(not(target_arch = "wasm32"))]
pub const HOST_ABI_CLIENT_TARGET_NOTE: &str =
    "Host ABI 客户端仅在 wasm32 目标编译（CI wasm job 验证导入绑定）";

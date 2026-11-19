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

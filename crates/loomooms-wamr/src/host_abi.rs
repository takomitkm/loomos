//! Host ABI v1（ADR-011 P2）。
//!
//! 版本化 Host 函数命名空间：Wasm Guest 通过 WAMR native symbol 注册
//! 调用这些函数。不允许 Guest 直接取得物理 DMA 地址或厂商 Runtime 指针。
//!
//! 设计纪律（ADR-011）：
//! - AI 推理（ONNX/sherpa/NPU）留在 Native Host，通过 `infer.submit` 暴露；
//! - 每个函数有版本号（ABI v1）——版本不匹配在导入校验时拒绝；
//! - 能力检查在调用入口（不是在 Host 函数内部——那是 Native Host 的责任）。

use serde::{Deserialize, Serialize};

/// Host ABI 版本（Guest 模块的导入段声明此版本；不匹配即拒绝加载）。
pub const HOST_ABI_VERSION: u32 = 1;

/// Host 函数命名空间前缀。
pub const HOST_NAMESPACE: &str = "loomos";

/// 推理请求（`loomos.infer.submit` 参数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferRequest {
    pub model_id: String,
    /// 输入数据在 Guest 线性内存中的偏移和长度。
    pub input_offset: u32,
    pub input_length: u32,
}

/// 推理结果（`loomos.infer.submit` 返回）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferResponse {
    pub status: String,
    /// 输出数据在 Guest 线性内存中的偏移和长度（Host 写回 Guest 内存）。
    pub output_offset: u32,
    pub output_length: u32,
}

/// 日志级别（`loomos.log` 参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

/// Host 函数签名表（文档 + 注册校验用）。
///
/// 每个条目：(命名空间.函数名, 参数类型列表, 返回类型, 说明)。
/// WAMR native symbol 注册时使用此表校验 Guest 导入段一致性。
pub static HOST_ABI_V1: &[(&str, &[&str], &str, &str)] = &[
    (
        "loomos.log_v1",
        &["level:i32", "msg_ptr:i32", "msg_len:i32"],
        "void",
        "结构化日志输出（级别+消息）",
    ),
    (
        "loomos.time_mono_v1",
        &[],
        "i64",
        "单调时钟（纳秒；不回退）",
    ),
    (
        "loomos.time_wall_v1",
        &[],
        "i64",
        "墙钟（Unix 毫秒；可跳变，禁止用于超时判定）",
    ),
    (
        "loomos_config_get_v1",
        &["ns_ptr:i32", "ns_len:i32", "key_ptr:i32", "key_len:i32"],
        "i32",
        "配置读取 → 返回值指针（Guest 内存），0=不存在",
    ),
    (
        "loomos_infer_submit_v1",
        &[
            "model_id_ptr:i32",
            "model_id_len:i32",
            "in_off:i32",
            "in_len:i32",
            "out_off_ptr:i32",
        ],
        "i32",
        "推理提交（模型 ID + 输入偏移/长度 → 输出偏移/长度写入）",
    ),
    (
        "loomos_device_op_v1",
        &[
            "dev_ptr:i32",
            "dev_len:i32",
            "op_ptr:i32",
            "op_len:i32",
            "params_ptr:i32",
        ],
        "i32",
        "设备操作提交（设备 ID + 操作 + JSON 参数）",
    ),
];

/// Host ABI 版本校验。
pub fn check_version(guest_version: u32) -> Result<(), crate::error::WamrError> {
    if guest_version != HOST_ABI_VERSION {
        return Err(crate::error::WamrError::AbiVersionMismatch {
            guest: guest_version,
            host: HOST_ABI_VERSION,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_v1() {
        assert_eq!(HOST_ABI_VERSION, 1);
        assert_eq!(HOST_NAMESPACE, "loomos");
        assert!(check_version(1).is_ok());
        assert!(check_version(2).is_err());
    }

    #[test]
    fn host_abi_table_has_required_functions() {
        let names: Vec<&str> = HOST_ABI_V1.iter().map(|e| e.0).collect();
        for required in [
            "loomos.log_v1",
            "loomos.time_mono_v1",
            "loomos_config_get_v1",
            "loomos_infer_submit_v1",
            "loomos_device_op_v1",
        ] {
            assert!(names.contains(&required), "缺少 {required}");
        }
    }
}

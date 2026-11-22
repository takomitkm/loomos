//! Host ABI v1（ADR-011 §6）——Guest 与 Native Host 权威之间的**唯一**通道。
//!
//! 安全要求（ADR-011 §6.3，逐条落实）：
//! - 调用方身份：invoke 进入时写入线程级调用上下文（[`current_call`]）；
//! - 能力校验：每次调用经 [`CapabilityChecker`]（权威在 Native Host 的
//!   CapabilityStore，由 loomosd 装配注入；Guest 永不自授权）；
//! - Guest 内存：offset/len 先经 WAMR validate（sys 层），复制语义读取——
//!   **native 指针绝不外泄、绝不跨调用保留**；
//! - 负载限额：[`PAYLOAD_LIMIT`] 之上的请求直接拒绝（防巨量拷贝）；
//! - 资源句柄：不透明 u64，[`HandleTable`] 按 owner 校验属主；
//! - 稳定错误码：[`code`] 模块的数字是 ABI 的一部分，不得变更。
//!
//! 约定：trampoline 返回 i32，**≥0 = 成功（句柄/长度）**，**<0 = -(稳定错误码)**。
//! 本文件保持 `#![forbid(unsafe_code)]`（crate 级）——trampoline 是安全的
//! `extern "C" fn`：参数直接取自 WAMR 调用约定，内存访问走 sys 层安全包装。

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Host ABI 版本（Guest 导入声明此版本；不匹配即拒绝）。
pub const HOST_ABI_VERSION: u32 = 1;

/// Host 函数命名空间（WAMR native module name）。
pub const HOST_NAMESPACE: &str = "loomos";

/// 单次调用 Guest 负载上限（字节）。控制面消息限界；大数据走 buffer 句柄。
pub const PAYLOAD_LIMIT: u32 = 64 * 1024;

/// 稳定错误码（ABI 一部分；只增不改）。
pub mod code {
    pub const OK: u32 = 0;
    pub const ERR_INVALID: u32 = 1;
    pub const ERR_CAPABILITY: u32 = 2;
    pub const ERR_MEMORY: u32 = 3;
    pub const ERR_PAYLOAD: u32 = 4;
    pub const ERR_HANDLE: u32 = 5;
    pub const ERR_UNSUPPORTED: u32 = 6;
    pub const ERR_BUSY: u32 = 7;
    pub const ERR_UNLINKED: u32 = 8;
    /// 操作结果无法确认（后端超时/挂死）——**不得映射 ERR_INVALID/Failed**（AGENTS.md §2.7）。
    pub const ERR_UNKNOWN: u32 = 9;
    /// 操作已取消（Guest 可区分取消与失败）。
    pub const ERR_CANCELLED: u32 = 10;
}

/// 接口 → 所需能力（空串 = 无能力要求）。接口标识 ≠ 权限授予。
pub static IFACE_CAPABILITY: &[(&str, &str)] = &[
    ("loomos.log_v1", "log"),
    ("loomos.time_mono_v1", ""),
    ("loomos.time_wall_v1", ""),
    ("loomos.config_get_v1", "config.read"),
    ("loomos.infer_submit_v1", "infer.use"),
    ("loomos.infer_poll_v1", "infer.use"),
    ("loomos.device_op_v1", "device.command"),
    ("loomos.device_poll_v1", "device.command"),
    ("loomos.buffer_import_v1", "buffer.use"),
    ("loomos.buffer_release_v1", "buffer.use"),
    ("loomos.event_subscribe_v1", "event.subscribe"),
];

/// Host ABI v1 接口表（名称一致性 + WAMR 签名串）。
/// 每条：(接口名, WAMR 签名, 返回约定, 说明)。
pub static HOST_ABI_V1: &[(&str, &str, &str, &str)] = &[
    (
        // void 结果签名以 ")" 结尾（不带尾部 v）——check_symbol_signature 实证：
        // result_count==0 时 ')' 后必须紧跟 NUL（CI resolve 失败→unlinked 实证）
        "loomos.log_v1",
        "(iii)",
        "void",
        "结构化日志（fire-and-forget，无返回错误路径）",
    ),
    (
        "loomos.time_mono_v1",
        "()I",
        "i64",
        "单调时钟 ns（不回退；超时判定唯一依据）",
    ),
    (
        "loomos.time_wall_v1",
        "()I",
        "i64",
        "墙钟 Unix ms（可跳变，禁止用于超时）",
    ),
    (
        "loomos.config_get_v1",
        "(iiiiii)i",
        "i32",
        "配置只读：ns_ptr/ns_len/key_ptr/key_len/out_ptr/out_cap → ≥0 写回长度，<0 = -错误码",
    ),
    (
        "loomos.infer_submit_v1",
        "(iii)i",
        "i32",
        "推理提交：model_ptr/model_len/buffer_handle → ≥0 = op 句柄，<0 = -错误码",
    ),
    (
        "loomos.infer_poll_v1",
        "(i)i",
        "i32",
        "推理轮询：op_handle → 稳定状态码（OK=完成/ERR_BUSY=进行中/…）",
    ),
    (
        "loomos.device_op_v1",
        "(iiiiii)i",
        "i32",
        "设备操作：dev_ptr/dev_len/op_ptr/op_len/params_ptr/params_len → ≥0 = op 句柄",
    ),
    (
        "loomos.device_poll_v1",
        "(i)i",
        "i32",
        "设备操作轮询：op_handle → 稳定状态码",
    ),
    (
        "loomos.buffer_import_v1",
        "(ii)i",
        "i32",
        "缓冲导入（Host 拥有分配）：size/align → ≥0 = buffer 句柄",
    ),
    (
        "loomos.buffer_release_v1",
        "(i)i",
        "i32",
        "缓冲归还：handle → 稳定状态码",
    ),
    (
        "loomos.event_subscribe_v1",
        "(ii)i",
        "i32",
        "事件订阅：event_ptr/event_len → ≥0 = subscription 句柄",
    ),
];

/// capability 名称校验：接口必须全部 `loomos.<iface>_v1` 形态。
pub fn iface_capability(iface: &str) -> Option<&'static str> {
    IFACE_CAPABILITY
        .iter()
        .find(|(n, _)| *n == iface)
        .map(|(_, c)| *c)
}

// ---------------------------------------------------------------------------
// 能力校验（Native Host 权威注入）
// ---------------------------------------------------------------------------

/// 能力检查器（由 loomosd 装配：接 loomos-runtime::capability::CapabilityStore）。
pub trait CapabilityChecker: Send + Sync {
    /// true = 该组件持有所述能力。空能力名 = 无要求，恒 true。
    fn check(&self, component: &str, capability: &str) -> bool;
}

/// 全放行检查器（仅测试/开发档使用；生产必须接真实 CapabilityStore）。
pub struct AllowAllChecker;

impl CapabilityChecker for AllowAllChecker {
    fn check(&self, _component: &str, _capability: &str) -> bool {
        true // 全放行（空能力名的跳过逻辑在 deny_or_ctx；生产必须换真实 CapabilityStore）
    }
}

// ---------------------------------------------------------------------------
// 资源句柄表（不透明句柄 + 属主校验）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleKind {
    InferOp,
    DeviceOp,
    Buffer,
    Subscription,
}

#[derive(Debug, Clone)]
pub struct HandleEntry {
    pub kind: HandleKind,
    /// Host 侧资源 id（Guest 永不见真值）。
    pub resource: u64,
}

/// 不透明资源句柄表：**Guest 只见 u64 句柄，Host 持有真资源并逐调用校验属主**。
#[derive(Default)]
pub struct HandleTable {
    next: u64,
    entries: BTreeMap<u64, (Arc<str>, HandleEntry)>,
}

impl HandleTable {
    /// 句柄起始值（非零：0 是「未初始化/空句柄」哨兵）。
    const FIRST: u64 = 0x4C4F_4D4F_0000_0001; // "LOMO" + 序号

    pub fn new() -> Self {
        Self {
            next: Self::FIRST,
            entries: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, owner: &str, kind: HandleKind, resource: u64) -> u64 {
        let h = self.next;
        self.next += 1;
        self.entries
            .insert(h, (Arc::from(owner), HandleEntry { kind, resource }));
        h
    }

    /// 校验：句柄存在 + 属主匹配 + 类型匹配。
    pub fn validate(
        &self,
        owner: &str,
        handle: u64,
        kind: HandleKind,
    ) -> Result<&HandleEntry, u32> {
        if handle == 0 {
            return Err(code::ERR_HANDLE);
        }
        let (h_owner, entry) = self.entries.get(&handle).ok_or(code::ERR_HANDLE)?;
        if h_owner.as_ref() != owner || entry.kind != kind {
            return Err(code::ERR_HANDLE);
        }
        Ok(entry)
    }

    /// 释放（一次性；重复释放/异主释放 → ERR_HANDLE）。
    pub fn release(
        &mut self,
        owner: &str,
        handle: u64,
        kind: HandleKind,
    ) -> Result<HandleEntry, u32> {
        if handle == 0 {
            return Err(code::ERR_HANDLE);
        }
        let (h_owner, entry) = self.entries.remove(&handle).ok_or(code::ERR_HANDLE)?;
        if h_owner.as_ref() != owner || entry.kind != kind {
            // 归还错属主条目：不销毁真资源（放回表），只报错
            self.entries.insert(handle, (h_owner, entry));
            return Err(code::ERR_HANDLE);
        }
        Ok(entry)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Handler 面（Host 侧实现；默认 = 不支持）
// ---------------------------------------------------------------------------

/// 轮询状态（稳定码直接对齐 [`code`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollStatus {
    /// 完成且带输出（输出经 buffer/config 通道回 Guest，不在 poll 里搬大块）。
    Done,
    /// 仍在执行。
    Pending,
    /// 执行失败（带稳定错误码）。
    Failed(u32),
    /// 结果无法确认——与 Failed 严格区分（AGENTS.md §2.7：Unknown 不谎报）。
    Unknown,
    /// 已取消。
    Cancelled,
}

impl PollStatus {
    pub fn to_code(&self) -> i32 {
        match self {
            PollStatus::Done => code::OK as i32,
            PollStatus::Pending => code::ERR_BUSY as i32,
            PollStatus::Failed(c) => -(*c as i32),
            PollStatus::Unknown => -(code::ERR_UNKNOWN as i32),
            PollStatus::Cancelled => -(code::ERR_CANCELLED as i32),
        }
    }
}

/// Host ABI 处理器（Native Host 实现；默认全部 ERR_UNSUPPORTED）。
#[allow(unused_variables)]
pub trait HostAbiHandlers: Send + Sync {
    fn log(&self, component: &str, level: u32, msg: &[u8]) -> u32 {
        code::ERR_UNSUPPORTED
    }
    fn time_mono(&self) -> Result<i64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn time_wall(&self) -> Result<i64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn config_get(&self, component: &str, namespace: &str, key: &str) -> Result<Vec<u8>, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn infer_submit(
        &self,
        component: &str,
        model_id: &str,
        buffer_handle: u64,
    ) -> Result<u64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn infer_poll(&self, component: &str, op_handle: u64) -> Result<PollStatus, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn device_op(
        &self,
        component: &str,
        device_id: &str,
        op: &str,
        params: &[u8],
    ) -> Result<u64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn device_poll(&self, component: &str, op_handle: u64) -> Result<PollStatus, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn buffer_import(&self, component: &str, size: u32, align: u32) -> Result<u64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn buffer_release(&self, component: &str, handle: u64) -> Result<(), u32> {
        Err(code::ERR_UNSUPPORTED)
    }
    fn event_subscribe(&self, component: &str, event: &str) -> Result<u64, u32> {
        Err(code::ERR_UNSUPPORTED)
    }
}

/// Host ABI 核心：处理器 + 能力检查器 + 句柄表。
pub struct HostAbiCore {
    pub handlers: Box<dyn HostAbiHandlers>,
    pub caps: Box<dyn CapabilityChecker>,
    pub handles: Mutex<HandleTable>,
}

// ---------------------------------------------------------------------------
// 静态注册表 + 调用上下文（trampoline 与门面之间的安全通道）
// ---------------------------------------------------------------------------

static CORE: OnceLock<RwLock<Option<Arc<HostAbiCore>>>> = OnceLock::new();

fn core_slot() -> &'static RwLock<Option<Arc<HostAbiCore>>> {
    CORE.get_or_init(|| RwLock::new(None))
}

/// 装配 Host ABI 核心（loomosd 启动时调用一次；重复装配覆盖）。
pub fn install(core: Arc<HostAbiCore>) {
    *core_slot().write().unwrap() = Some(core);
}

/// 当前已装配核心（trampoline 用）。
pub fn installed() -> Option<Arc<HostAbiCore>> {
    core_slot().read().unwrap().clone()
}

#[derive(Debug, Clone)]
pub struct CallCtx {
    pub component: Arc<str>,
    pub inst: loomos_wamr_sys::WasmModuleInst,
}

thread_local! {
    static CURRENT_CALL: std::cell::RefCell<Option<CallCtx>> = const { std::cell::RefCell::new(None) };
}

/// 进入 Host ABI 调用上下文（RuntimeHost::invoke 内、call_wasm 前设置）。
/// v1 不重入（Guest 调用 Host 期间不会再发起 invoke）。
pub fn enter_call(component: &str, inst: loomos_wamr_sys::WasmModuleInst) {
    CURRENT_CALL.with(|c| {
        *c.borrow_mut() = Some(CallCtx {
            component: Arc::from(component),
            inst,
        });
    });
}

/// 退出调用上下文（call_wasm 后无论成败）。
pub fn exit_call() {
    CURRENT_CALL.with(|c| {
        *c.borrow_mut() = None;
    });
}

/// 当前调用上下文（trampoline 用；直接 native 调用无 ctx → None）。
pub fn current_call() -> Option<CallCtx> {
    CURRENT_CALL.with(|c| c.borrow().clone())
}

// ---------------------------------------------------------------------------
// Trampoline（extern "C"，安全函数；签名与 HOST_ABI_V1 逐条对应）
// ---------------------------------------------------------------------------

fn read_checked(ptr: i32, len: i32) -> Result<Vec<u8>, i32> {
    if ptr < 0 || len < 0 {
        return Err(-(code::ERR_INVALID as i32));
    }
    let (ptr, len) = (ptr as u32 as u64, len as u32);
    if len > PAYLOAD_LIMIT {
        return Err(-(code::ERR_PAYLOAD as i32));
    }
    let Some(ctx) = current_call() else {
        return Err(-(code::ERR_INVALID as i32));
    };
    match loomos_wamr_sys::read_guest_memory(ctx.inst, ptr, len) {
        Ok(bytes) => Ok(bytes),
        Err(loomos_wamr_sys::SysError::NotLinked) => Err(-(code::ERR_UNLINKED as i32)),
        Err(_) => Err(-(code::ERR_MEMORY as i32)),
    }
}

fn utf8_or_invalid(bytes: &[u8]) -> Result<String, i32> {
    String::from_utf8(bytes.to_vec()).map_err(|_| -(code::ERR_INVALID as i32))
}

fn deny_or_ctx(iface: &'static str) -> Result<(Arc<HostAbiCore>, CallCtx), i32> {
    let Some(core) = installed() else {
        return Err(-(code::ERR_UNLINKED as i32));
    };
    let Some(ctx) = current_call() else {
        return Err(-(code::ERR_INVALID as i32));
    };
    let cap = iface_capability(iface).unwrap_or("");
    if !cap.is_empty() && !core.caps.check(&ctx.component, cap) {
        return Err(-(code::ERR_CAPABILITY as i32));
    }
    Ok((core, ctx))
}

pub(super) extern "C" fn tr_log(_exec_env: *mut c_void, level: i32, ptr: i32, len: i32) {
    // fire-and-forget：无返回通道，错误只能吞（记录进 handler 结果码）
    let Ok((core, _ctx)) = deny_or_ctx("loomos.log_v1") else {
        return;
    };
    let Ok(msg) = read_checked(ptr, len) else {
        return;
    };
    let _ = core.handlers.log(&_ctx.component, level as u32, &msg);
}

pub(super) extern "C" fn tr_time_mono(_exec_env: *mut c_void) -> i64 {
    match deny_or_ctx("loomos.time_mono_v1") {
        Ok((core, _)) => core.handlers.time_mono().unwrap_or(0),
        Err(_) => 0, // time 无错误通道（v1 简化：失败给 0，文档注明）
    }
}

pub(super) extern "C" fn tr_time_wall(_exec_env: *mut c_void) -> i64 {
    match deny_or_ctx("loomos.time_wall_v1") {
        Ok((core, _)) => core.handlers.time_wall().unwrap_or(0),
        Err(_) => 0,
    }
}

pub(super) extern "C" fn tr_config_get(
    _exec_env: *mut c_void,
    ns_ptr: i32,
    ns_len: i32,
    key_ptr: i32,
    key_len: i32,
    out_ptr: i32,
    out_cap: i32,
) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.config_get_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    let ns = match read_checked(ns_ptr, ns_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let key = match read_checked(key_ptr, key_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let value = match core.handlers.config_get(&ctx.component, &ns, &key) {
        Ok(v) => v,
        Err(c) => return -(c as i32),
    };
    if value.len() as i32 > out_cap {
        return -(code::ERR_PAYLOAD as i32);
    }
    if out_ptr < 0 || out_cap < 0 {
        return -(code::ERR_INVALID as i32);
    }
    match loomos_wamr_sys::write_guest_memory(ctx.inst, out_ptr as u32 as u64, &value) {
        Ok(()) => value.len() as i32,
        Err(loomos_wamr_sys::SysError::NotLinked) => -(code::ERR_UNLINKED as i32),
        Err(_) => -(code::ERR_MEMORY as i32),
    }
}

pub(super) extern "C" fn tr_infer_submit(
    _exec_env: *mut c_void,
    model_ptr: i32,
    model_len: i32,
    buffer_handle: i32,
) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.infer_submit_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    let model = match read_checked(model_ptr, model_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if buffer_handle < 0 {
        return -(code::ERR_INVALID as i32);
    }
    let handles = core.handles.lock().unwrap();
    let entry = match handles.validate(&ctx.component, buffer_handle as u64, HandleKind::Buffer) {
        Ok(e) => e.clone(),
        Err(c) => return -(c as i32),
    };
    drop(handles);
    let op = match core
        .handlers
        .infer_submit(&ctx.component, &model, entry.resource)
    {
        Ok(op) => op,
        Err(c) => return -(c as i32),
    };
    let mut handles = core.handles.lock().unwrap();
    handles.insert(&ctx.component, HandleKind::InferOp, op) as i32
}

pub(super) extern "C" fn tr_infer_poll(_exec_env: *mut c_void, op_handle: i32) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.infer_poll_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    if op_handle < 0 {
        return -(code::ERR_INVALID as i32);
    }
    let handles = core.handles.lock().unwrap();
    let entry = match handles.validate(&ctx.component, op_handle as u64, HandleKind::InferOp) {
        Ok(e) => e.clone(),
        Err(c) => return -(c as i32),
    };
    drop(handles);
    match core.handlers.infer_poll(&ctx.component, entry.resource) {
        Ok(status) => status.to_code(),
        Err(c) => -(c as i32),
    }
}

pub(super) extern "C" fn tr_device_op(
    _exec_env: *mut c_void,
    dev_ptr: i32,
    dev_len: i32,
    op_ptr: i32,
    op_len: i32,
    params_ptr: i32,
    params_len: i32,
) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.device_op_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    let dev = match read_checked(dev_ptr, dev_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let op = match read_checked(op_ptr, op_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let params = match read_checked(params_ptr, params_len) {
        Ok(b) => b,
        Err(c) => return c,
    };
    let op_id = match core.handlers.device_op(&ctx.component, &dev, &op, &params) {
        Ok(id) => id,
        Err(c) => return -(c as i32),
    };
    let mut handles = core.handles.lock().unwrap();
    handles.insert(&ctx.component, HandleKind::DeviceOp, op_id) as i32
}

pub(super) extern "C" fn tr_device_poll(_exec_env: *mut c_void, op_handle: i32) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.device_poll_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    if op_handle < 0 {
        return -(code::ERR_INVALID as i32);
    }
    let handles = core.handles.lock().unwrap();
    let entry = match handles.validate(&ctx.component, op_handle as u64, HandleKind::DeviceOp) {
        Ok(e) => e.clone(),
        Err(c) => return -(c as i32),
    };
    drop(handles);
    match core.handlers.device_poll(&ctx.component, entry.resource) {
        Ok(status) => status.to_code(),
        Err(c) => -(c as i32),
    }
}

pub(super) extern "C" fn tr_buffer_import(_exec_env: *mut c_void, size: i32, align: i32) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.buffer_import_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    if size < 0 || align < 0 {
        return -(code::ERR_INVALID as i32);
    }
    let resource = match core
        .handlers
        .buffer_import(&ctx.component, size as u32, align as u32)
    {
        Ok(r) => r,
        Err(c) => return -(c as i32),
    };
    let mut handles = core.handles.lock().unwrap();
    handles.insert(&ctx.component, HandleKind::Buffer, resource) as i32
}

pub(super) extern "C" fn tr_buffer_release(_exec_env: *mut c_void, handle: i32) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.buffer_release_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    if handle < 0 {
        return -(code::ERR_INVALID as i32);
    }
    let mut handles = core.handles.lock().unwrap();
    let entry = match handles.release(&ctx.component, handle as u64, HandleKind::Buffer) {
        Ok(e) => e,
        Err(c) => return -(c as i32),
    };
    drop(handles);
    match core.handlers.buffer_release(&ctx.component, entry.resource) {
        Ok(()) => code::OK as i32,
        Err(c) => -(c as i32),
    }
}

pub(super) extern "C" fn tr_event_subscribe(
    _exec_env: *mut c_void,
    event_ptr: i32,
    event_len: i32,
) -> i32 {
    let (core, ctx) = match deny_or_ctx("loomos.event_subscribe_v1") {
        Ok(v) => v,
        Err(c) => return c,
    };
    let event = match read_checked(event_ptr, event_len).and_then(|b| utf8_or_invalid(&b)) {
        Ok(s) => s,
        Err(c) => return c,
    };
    let sub = match core.handlers.event_subscribe(&ctx.component, &event) {
        Ok(s) => s,
        Err(c) => return -(c as i32),
    };
    let mut handles = core.handles.lock().unwrap();
    handles.insert(&ctx.component, HandleKind::Subscription, sub) as i32
}

/// 符号表构建（safe：泄漏分配使符号表/名称活到进程结束）。
/// 返回（符号表，条目数）；由 [`register_symbols`] 交给 sys 层。
fn build_symbol_table() -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    // (符号名, 签名, trampoline 地址序号)——地址通过 fn 指针表按序取
    // fn item → fn pointer 显式定型（fn item 不能直接 as 跨签名转型）
    const T_LOG: extern "C" fn(*mut c_void, i32, i32, i32) = tr_log;
    const T_MONO: extern "C" fn(*mut c_void) -> i64 = tr_time_mono;
    const T_WALL: extern "C" fn(*mut c_void) -> i64 = tr_time_wall;
    const T_CFG: extern "C" fn(*mut c_void, i32, i32, i32, i32, i32, i32) -> i32 = tr_config_get;
    const T_ISUB: extern "C" fn(*mut c_void, i32, i32, i32) -> i32 = tr_infer_submit;
    const T_IPOLL: extern "C" fn(*mut c_void, i32) -> i32 = tr_infer_poll;
    const T_DOP: extern "C" fn(*mut c_void, i32, i32, i32, i32, i32, i32) -> i32 = tr_device_op;
    const T_DPOLL: extern "C" fn(*mut c_void, i32) -> i32 = tr_device_poll;
    const T_BIM: extern "C" fn(*mut c_void, i32, i32) -> i32 = tr_buffer_import;
    const T_BREL: extern "C" fn(*mut c_void, i32) -> i32 = tr_buffer_release;
    const T_ESUB: extern "C" fn(*mut c_void, i32, i32) -> i32 = tr_event_subscribe;
    let ptrs: Vec<usize> = vec![
        T_LOG as usize,
        T_MONO as usize,
        T_WALL as usize,
        T_CFG as usize,
        T_ISUB as usize,
        T_IPOLL as usize,
        T_DOP as usize,
        T_DPOLL as usize,
        T_BIM as usize,
        T_BREL as usize,
        T_ESUB as usize,
    ];
    for (i, (name, sig, _ret, _doc)) in HOST_ABI_V1.iter().enumerate() {
        out.push((name.to_string(), sig.to_string(), ptrs[i]));
    }
    out
}

/// Host ABI 注册凭据（unregister 用；usize 存地址——裸指针非 Sync）。
static UNREGISTER_CREDENTIALS: OnceLock<(usize, usize)> = OnceLock::new();

/// shutdown 用：注销 Host ABI 符号表（**须在 wasm_runtime_destroy 之前**调用；
/// 幂等）。unregister 只 free WAMR 节点，数组（std 泄漏，进程生命周期）不被触碰——
/// wasm_native.c unregister 实证（pinned rev）。
pub fn unregister_runtime_natives() {
    if let Some((m, s)) = UNREGISTER_CREDENTIALS.get() {
        let _ = loomos_wamr_sys::unregister_natives(
            *m as *const std::ffi::c_char,
            *s as *mut loomos_wamr_sys::NativeSymbol,
        );
    }
}

/// 注册 Host ABI 符号表（**须在 RuntimeHost::init 的 full_init 之后调用**）。
/// 数组 4 字段（attachment=null，lib_export.h 实证）；std 泄漏至进程结束；
/// WAMR 只存指针不复制、不 free——shutdown 先 unregister 再 destroy。
/// 符号字段为裸函数名（module+field 分离解析）。
pub fn register_runtime_natives() -> WamrResult<()> {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    if REGISTERED.set(()).is_err() {
        return Ok(()); // 已注册（幂等）
    }
    let table = build_symbol_table();
    let mut native: Vec<loomos_wamr_sys::NativeSymbol> = Vec::with_capacity(table.len());
    for (name, sig, addr) in &table {
        let field = name
            .strip_prefix(&format!("{HOST_NAMESPACE}."))
            .unwrap_or(name);
        native.push(loomos_wamr_sys::NativeSymbol {
            symbol: leaked_cstr(field),
            func_ptr: *addr as *mut c_void,
            signature: leaked_cstr(sig),
            attachment: std::ptr::null_mut(),
        });
    }
    // **Box::leak**：WAMR 只存指针不复制——数组必须在进程生命周期内存活。
    // into_boxed_slice 后 drop（曾经的写法）= 注册指针悬垂 → resolve 读垃圾 →
    // 'unlinked import'（CI E2E 实证：直调路径 checksum 全出、via_host 才失败）。
    let native = Box::leak(native.into_boxed_slice());
    let credentials = leaked_cstr(HOST_NAMESPACE);
    let ok =
        loomos_wamr_sys::register_natives_checked(credentials, native).map_err(WamrError::from)?;
    if !ok {
        return Err(WamrError::Sys(loomos_wamr_sys::SysError::InitFailed(
            "register_natives 返回 false".into(),
        )));
    }
    UNREGISTER_CREDENTIALS
        .set((credentials as usize, native.as_mut_ptr() as usize))
        .ok();
    Ok(())
}

/// 旧独立注册入口——**已弃用**（注册统一走 RuntimeHost::init；时序与分配器
/// 语义经 CI gdb/valgrind 定案）。保留兼容签名：stub 档报 NotLinked，
/// link 档视为已由 init 注册。
pub fn register_symbols() -> WamrResult<()> {
    if loomos_wamr_sys::linked() {
        Ok(())
    } else {
        Err(WamrError::NotLinked)
    }
}

/// NUL 结尾的 C 字符串，泄漏至进程结束（符号名/签名要求 'static）。
fn leaked_cstr(s: &str) -> *const c_char {
    let mut bytes = s.as_bytes().to_vec();
    bytes.push(0);
    Box::leak(bytes.into_boxed_slice()).as_ptr() as *const c_char
}

/// WamrError → 稳定错误码映射（负值约定与 trampoline 一致）。
pub fn error_code(err: &WamrError) -> u32 {
    use WamrError::*;
    match err {
        NotInitialized | Busy => code::ERR_BUSY,
        AlreadyInitialized => code::ERR_INVALID,
        NotLinked => code::ERR_UNLINKED,
        NoSuchModule(_) | ModuleAlreadyLoaded(_) | NoSuchInstance(_) | InvalidHandle(_) => {
            code::ERR_HANDLE
        }
        InvalidFormat { .. } | AotIncompatible { .. } | AotCompatKeyMissing(_) => code::ERR_INVALID,
        ModuleBusy { .. } => code::ERR_BUSY,
        MemoryBudgetExceeded { .. }
        | InstanceMemoryLimitExceeded { .. }
        | UnboundedMemoryForbidden => code::ERR_MEMORY,
        AbiVersionMismatch { .. } => code::ERR_INVALID,
        CapabilityDenied { .. } => code::ERR_CAPABILITY,
        GuestMemoryOutOfRange { .. } => code::ERR_MEMORY,
        PayloadTooLarge { .. } => code::ERR_PAYLOAD,
        Sys(_) => code::ERR_INVALID,
    }
}

// 别名：lib.rs re-export 路径
use crate::error::{WamrError, WamrResult};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;

    // 静态 CORE 注册表是全局的——install 依赖的测试共享一把锁串行执行
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn error_codes_are_distinct_and_stable() {
        let codes = [
            code::OK,
            code::ERR_INVALID,
            code::ERR_CAPABILITY,
            code::ERR_MEMORY,
            code::ERR_PAYLOAD,
            code::ERR_HANDLE,
            code::ERR_UNSUPPORTED,
            code::ERR_BUSY,
            code::ERR_UNLINKED,
            code::ERR_UNKNOWN,
            code::ERR_CANCELLED,
        ];
        let set: std::collections::BTreeSet<u32> = codes.iter().copied().collect();
        assert_eq!(set.len(), codes.len(), "稳定错误码必须互异");
    }

    #[test]
    fn iface_names_consistent_v1_form() {
        // 接口表与能力表一一对应；全部 loomos.<iface>_v1
        assert_eq!(HOST_ABI_V1.len(), IFACE_CAPABILITY.len());
        let names: std::collections::BTreeSet<&str> = HOST_ABI_V1.iter().map(|e| e.0).collect();
        for name in &names {
            assert!(name.starts_with("loomos."), "{name} 缺命名空间前缀");
            assert!(name.ends_with("_v1"), "{name} 缺 v1 后缀");
            assert!(iface_capability(name).is_some(), "{name} 缺能力映射");
        }
        assert_eq!(names.len(), HOST_ABI_V1.len(), "接口名必须唯一");
    }

    #[test]
    fn signatures_use_known_alphabet() {
        // WAMR 签名字母表：i I f F v * $
        for (name, sig, _ret, _doc) in HOST_ABI_V1 {
            // 形态 "(参数)返回"：以 '(' 开头、')' 在中段、尾部是返回类型字符
            assert!(sig.starts_with('('), "{name}: {sig}");
            let close = sig.find(')').expect("{name}: 签名缺 ')'");
            assert!(close >= 1, "{name}: 空参签名应为 ()X 形态");
            let ret = &sig[close + 1..];
            // void 结果：')' 后必须紧跟 NUL——签名不得带尾部 v
            // （check_symbol_signature 实证；CI resolve 失败→unlinked）
            if ret == "v" {
                panic!("{name}: void 签名不得带尾部 v: {sig}");
            }
            assert!(
                matches!(ret, "" | "i" | "I" | "f" | "F"),
                "{name}: 返回类型 {ret:?} 非法（空=void）"
            );
            assert!(
                sig.chars()
                    .all(|c| matches!(c, '(' | ')' | 'i' | 'I' | 'f' | 'F' | 'v' | '*' | '$')),
                "{name}: 非法签名字符 {sig}"
            );
            assert_eq!(
                sig.matches('(').count(),
                sig.matches(')').count(),
                "{name}: 括号配平"
            );
        }
    }

    #[test]
    fn handle_table_owner_and_kind_enforced() {
        let mut t = HandleTable::new();
        let h = t.insert("nlu", HandleKind::Buffer, 0xABCD);
        assert_ne!(h, 0, "0 是空句柄哨兵");
        assert!(t.validate("nlu", h, HandleKind::Buffer).is_ok());
        // 异主
        assert!(matches!(
            t.validate("other", h, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
        // 异类型
        assert!(matches!(
            t.validate("nlu", h, HandleKind::InferOp),
            Err(code::ERR_HANDLE)
        ));
        // 释放后不可再用
        let e = t.release("nlu", h, HandleKind::Buffer).unwrap();
        assert_eq!(e.resource, 0xABCD);
        assert!(matches!(
            t.validate("nlu", h, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
        // 重复释放
        assert!(matches!(
            t.release("nlu", h, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
        // 0 句柄哨兵
        assert!(matches!(
            t.validate("nlu", 0, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
        assert!(matches!(
            t.release("nlu", 0, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
    }

    #[test]
    fn handle_table_wrong_owner_release_keeps_resource() {
        let mut t = HandleTable::new();
        let h = t.insert("nlu", HandleKind::Buffer, 42);
        assert!(matches!(
            t.release("home", h, HandleKind::Buffer),
            Err(code::ERR_HANDLE)
        ));
        assert!(
            t.validate("nlu", h, HandleKind::Buffer).is_ok(),
            "异主释放不得销毁真资源"
        );
    }

    struct RecordingHandlers {
        calls: AtomicU32,
    }
    impl HostAbiHandlers for RecordingHandlers {
        fn log(&self, _c: &str, _l: u32, _m: &[u8]) -> u32 {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            code::OK
        }
        fn time_mono(&self) -> Result<i64, u32> {
            Ok(123_456)
        }
    }

    #[test]
    fn default_handlers_report_unsupported() {
        let core = Arc::new(HostAbiCore {
            handlers: Box::new(RecordingHandlers {
                calls: AtomicU32::new(0),
            }),
            caps: Box::new(AllowAllChecker),
            handles: Mutex::new(HandleTable::new()),
        });
        assert_eq!(
            core.handlers.config_get("nlu", "ns", "k"),
            Err(code::ERR_UNSUPPORTED)
        );
        assert_eq!(
            core.handlers.infer_submit("nlu", "m", 1),
            Err(code::ERR_UNSUPPORTED)
        );
        assert_eq!(core.handlers.time_mono(), Ok(123_456));
    }

    #[test]
    fn capability_denied_maps_to_stable_code() {
        let _g = guard();
        struct DenyAll;
        impl CapabilityChecker for DenyAll {
            fn check(&self, _c: &str, cap: &str) -> bool {
                cap.is_empty()
            }
        }
        let core = Arc::new(HostAbiCore {
            handlers: Box::new(RecordingHandlers {
                calls: AtomicU32::new(0),
            }),
            caps: Box::new(DenyAll),
            handles: Mutex::new(HandleTable::new()),
        });
        install(core);
        // ctx 就绪但无能力 → tr_config_get 返回 -ERR_CAPABILITY
        enter_call("nlu", std::ptr::null_mut());
        let r = tr_config_get(std::ptr::null_mut(), 0, 0, 0, 0, 0, 0);
        exit_call();
        assert_eq!(r, -(code::ERR_CAPABILITY as i32));
    }

    #[test]
    fn trampolines_reject_without_call_ctx() {
        let _g = guard();
        // 无 ctx（宿主直接调用，非 Guest 路径）→ -ERR_INVALID
        let core = Arc::new(HostAbiCore {
            handlers: Box::new(RecordingHandlers {
                calls: AtomicU32::new(0),
            }),
            caps: Box::new(AllowAllChecker),
            handles: Mutex::new(HandleTable::new()),
        });
        install(core);
        assert_eq!(
            tr_infer_poll(std::ptr::null_mut(), 1),
            -(code::ERR_INVALID as i32)
        );
        assert_eq!(
            tr_buffer_release(std::ptr::null_mut(), 1),
            -(code::ERR_INVALID as i32)
        );
    }

    #[test]
    fn payload_over_limit_rejected() {
        let _g = guard();
        let core = Arc::new(HostAbiCore {
            handlers: Box::new(RecordingHandlers {
                calls: AtomicU32::new(0),
            }),
            caps: Box::new(AllowAllChecker),
            handles: Mutex::new(HandleTable::new()),
        });
        install(core);
        enter_call("nlu", std::ptr::null_mut());
        tr_log(std::ptr::null_mut(), 2, 0, PAYLOAD_LIMIT as i32 + 1); // void：无返回通道
        exit_call();
        // log 的拒绝路径不可观测（无返回通道）——用 config_get 验证 payload 限额
        enter_call("nlu", std::ptr::null_mut());
        let r = tr_config_get(
            std::ptr::null_mut(),
            0,
            0,
            0,
            PAYLOAD_LIMIT as i32 + 1,
            0,
            0,
        );
        exit_call();
        assert_eq!(r, -(code::ERR_PAYLOAD as i32));
    }

    #[test]
    fn stub_mode_trampolines_map_notlinked_to_unlinked_code() {
        if loomos_wamr_sys::linked() {
            return;
        }
        let _g = guard();
        let core = Arc::new(HostAbiCore {
            handlers: Box::new(RecordingHandlers {
                calls: AtomicU32::new(0),
            }),
            caps: Box::new(AllowAllChecker),
            handles: Mutex::new(HandleTable::new()),
        });
        install(core);
        enter_call("nlu", std::ptr::null_mut());
        let r = tr_config_get(std::ptr::null_mut(), 8, 2, 16, 3, 32, 64);
        exit_call();
        assert_eq!(r, -(code::ERR_UNLINKED as i32));
    }

    #[test]
    fn register_symbols_in_stub_mode_fails_cleanly() {
        if loomos_wamr_sys::linked() {
            return;
        }
        let _g = guard();
        let r = register_symbols();
        assert!(matches!(r, Err(WamrError::NotLinked)));
    }

    #[test]
    fn error_code_mapping_is_total() {
        let cases = vec![
            WamrError::Busy,
            WamrError::NotLinked,
            WamrError::NoSuchModule("m".into()),
            WamrError::CapabilityDenied {
                component: "c".into(),
                capability: "x".into(),
            },
            WamrError::PayloadTooLarge {
                actual: 9,
                limit: 8,
            },
            WamrError::GuestMemoryOutOfRange { offset: 0, size: 1 },
            WamrError::InvalidHandle(1),
        ];
        for e in cases {
            let c = error_code(&e);
            assert!(c != 0, "{e:?} 不得映射 OK");
        }
    }
}

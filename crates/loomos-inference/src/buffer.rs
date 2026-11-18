//! Buffer 所有权状态机——typestate 编码（architecture/12 §12.4）。
//!
//! 状态：CpuOwned → SyncForDevice → DeviceOwned → DeviceComplete → SyncForCpu → CpuOwned
//!
//! 四条不变量（12.4）：
//! 1. 未确认硬件完成前（device_complete），不得释放 Buffer——DeviceOwned/SyncForDevice
//!    被 Drop 即登记 leak（debug 构建直接 panic）；
//! 2. 未执行 sync_for_cpu 前，CPU 不得读取设备写入的内容——读接口只在 CpuOwned 态提供；
//! 3. 不得假定 Cache 硬件一致性——sync 是显式步骤，不是 no-op；
//! 4. 所有权转移必须有明确主体变更点——每次转移都是一次显式方法调用。
//!
//! 地址域类型（12.5）：禁止裸 u64 传递地址。

use loomos_runtime::ids::OperationId;
use std::marker::PhantomData;
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum BufferError {
    #[error("非法状态转移：{from} → {to}")]
    IllegalTransition {
        from: &'static str,
        to: &'static str,
    },
    #[error("设备完成证据缺失：不得从 DeviceOwned 直接离开")]
    MissingCompletionEvidence,
    #[error("Buffer 在 {0} 态被丢弃：未确认硬件完成前不得释放（leak 已登记）")]
    DroppedWhileDeviceHeld(&'static str),
}

/// 地址域 Newtype（12.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuVirtualAddress(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuPhysicalAddress(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDmaAddress(pub u64);

impl std::fmt::Display for CpuVirtualAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cpu-va:{:#x}", self.0)
    }
}
impl std::fmt::Display for CpuPhysicalAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cpu-phys:{:#x}", self.0)
    }
}
impl std::fmt::Display for DeviceDmaAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dev-dma:{:#x}", self.0)
    }
}

/// 完成证据：只有后端能签发，且必须与请求对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionToken {
    pub operation: OperationId,
    pub complete: bool,
}

// ---------- typestate 标记 ----------
#[derive(Debug)]
pub struct CpuOwned;
#[derive(Debug)]
pub struct SyncForDevice;
#[derive(Debug)]
pub struct DeviceOwned;
#[derive(Debug)]
pub struct DeviceComplete;
#[derive(Debug)]
pub struct SyncForCpu;

/// 状态名。
pub trait DeviceState {
    const NAME: &'static str;
    /// 设备侧是否仍可能访问该 Buffer（决定 Drop 语义）。
    const DEVICE_MAY_ACCESS: bool;
}
impl DeviceState for CpuOwned {
    const NAME: &'static str = "CpuOwned";
    const DEVICE_MAY_ACCESS: bool = false;
}
impl DeviceState for SyncForDevice {
    const NAME: &'static str = "SyncForDevice";
    const DEVICE_MAY_ACCESS: bool = true;
}
impl DeviceState for DeviceOwned {
    const NAME: &'static str = "DeviceOwned";
    const DEVICE_MAY_ACCESS: bool = true;
}
impl DeviceState for DeviceComplete {
    const NAME: &'static str = "DeviceComplete";
    const DEVICE_MAY_ACCESS: bool = false;
}
impl DeviceState for SyncForCpu {
    const NAME: &'static str = "SyncForCpu";
    const DEVICE_MAY_ACCESS: bool = false;
}

/// 带 ownership 状态的 Buffer。
///
/// 原型阶段数据本体是 `Vec<u8>`（宿主内存副本）；真实 DMA 场景下 `cpu_addr/dev_addr`
/// 由平台 Backend 填写（本机 DMA addr==phys 仅作为启动校验属性——12.6）。
#[derive(Debug)]
pub struct Buffer<S: DeviceState> {
    data: Vec<u8>,
    cpu_addr: CpuVirtualAddress,
    dev_addr: Option<DeviceDmaAddress>,
    operation: Option<OperationId>,
    _state: PhantomData<S>,
}

impl<S: DeviceState> Buffer<S> {
    /// 状态转移：字段原样移交（Drop 由目标类型接管，不触发当前态的 Drop 检查）。
    fn cast<T: DeviceState>(self) -> Buffer<T> {
        let mut me = std::mem::ManuallyDrop::new(self);
        Buffer {
            data: std::mem::take(&mut me.data),
            cpu_addr: me.cpu_addr,
            dev_addr: me.dev_addr,
            operation: me.operation,
            _state: PhantomData,
        }
    }

    fn drop_guard(&self) {
        if S::DEVICE_MAY_ACCESS {
            tracing::warn!(
                addr=%self.dev_addr.unwrap_or(DeviceDmaAddress(0)),
                op=?self.operation,
                state = S::NAME,
                "buffer dropped while device may still access; LEAK recorded"
            );
            #[cfg(debug_assertions)]
            panic!("{:?}", BufferError::DroppedWhileDeviceHeld(S::NAME));
        }
    }
}

impl<S: DeviceState> Drop for Buffer<S> {
    fn drop(&mut self) {
        self.drop_guard();
    }
}

impl Buffer<CpuOwned> {
    /// 在 CPU 侧创建 Buffer。
    pub fn new_cpu(len: usize) -> Self {
        Self {
            data: vec![0u8; len],
            cpu_addr: CpuVirtualAddress(0x1000_0000),
            dev_addr: None,
            operation: None,
            _state: PhantomData,
        }
    }

    pub fn with_data(data: Vec<u8>) -> Self {
        Self {
            cpu_addr: CpuVirtualAddress(0x1000_0000),
            dev_addr: None,
            operation: None,
            _state: PhantomData,
            data,
        }
    }

    /// CPU 读写只在 CpuOwned 合法（不变量 2）。
    pub fn cpu_read(&self) -> &[u8] {
        &self.data
    }

    pub fn cpu_write(&mut self, data: &[u8]) {
        self.data.clear();
        self.data.extend_from_slice(data);
    }

    pub fn cpu_address(&self) -> CpuVirtualAddress {
        self.cpu_addr
    }

    /// 交接给设备：先 Cache 同步（sync_for_device），再 device_take。
    pub fn sync_for_device(
        self,
        dev_addr: DeviceDmaAddress,
        operation: OperationId,
    ) -> Buffer<SyncForDevice> {
        tracing::debug!(buf_addr=%self.cpu_addr, dev=%dev_addr, op=%operation, "sync for device (cache clean)");
        let mut b = self.cast::<SyncForDevice>();
        b.dev_addr = Some(dev_addr);
        b.operation = Some(operation);
        b
    }
}

impl Buffer<SyncForDevice> {
    /// 主体变更点：设备开始访问（不变量 4）。
    pub fn device_take(self) -> Buffer<DeviceOwned> {
        self.cast()
    }
}

impl Buffer<DeviceOwned> {
    pub fn device_address(&self) -> DeviceDmaAddress {
        self.dev_addr.expect("DeviceOwned 必有设备地址")
    }

    pub fn operation(&self) -> Option<OperationId> {
        self.operation
    }

    /// 设备完成确认（不变量 1 的可观测点）。证据必须与请求对应；
    /// 证据不符时 Buffer 原样退回（仍处 DeviceOwned 保护态，不得释放）。
    pub fn device_complete(
        self,
        token: CompletionToken,
    ) -> Result<Buffer<DeviceComplete>, (BufferError, Buffer<DeviceOwned>)> {
        match self.operation {
            Some(op) if token.operation == op && token.complete => {
                tracing::debug!(op=%op, "device completion confirmed");
                Ok(self.cast())
            }
            _ => Err((BufferError::MissingCompletionEvidence, self)),
        }
    }

    /// 隔离区出口：人工确认设备已停止访问后释放。
    /// 绕过 Drop 守卫的前提是**有人工确认证据**（12.4 不变量 1 的兜底路径）。
    pub fn release_after_human_confirmation(self) {
        let mut me = std::mem::ManuallyDrop::new(self);
        // 回收宿主侧数据副本；真实 DMA 场景此处必须先取得"设备已停"证据再 unmap
        let data = std::mem::take(&mut me.data);
        tracing::warn!(
            addr=%me.dev_addr.unwrap_or(DeviceDmaAddress(0)),
            op=?me.operation,
            len = data.len(),
            "quarantined buffer released after human confirmation"
        );
    }
}

impl Buffer<DeviceComplete> {
    /// 完成 ≠ CPU 可读：必须显式 sync_for_cpu。
    /// `device_written` 是设备经 DMA 写入的数据，在 sync 之后才对 CPU 可见
    /// （真实平台由 dma_sync_single_for_cpu 语义保证；原型阶段以参数注入模拟）。
    pub fn sync_for_cpu(self, device_written: Vec<u8>) -> Buffer<SyncForCpu> {
        tracing::debug!(op=?self.operation, "sync for cpu (cache invalidate)");
        let mut b = self.cast::<SyncForCpu>();
        b.data = device_written;
        b
    }
}

impl Buffer<SyncForCpu> {
    /// 回到 CPU 所有。
    pub fn cpu_take(self) -> Buffer<CpuOwned> {
        self.cast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_cycle_preserves_device_written_data() {
        let op = OperationId::next();
        let mut buf = Buffer::new_cpu(16);
        buf.cpu_write(&[1, 2, 3, 4]);
        let buf = buf.sync_for_device(DeviceDmaAddress(0x3010_0000), op);
        let buf = buf.device_take();
        assert_eq!(buf.device_address(), DeviceDmaAddress(0x3010_0000));
        let buf = buf
            .device_complete(CompletionToken {
                operation: op,
                complete: true,
            })
            .unwrap();
        // 设备写入的结果只有 sync_for_cpu 后才可见
        let buf = buf.sync_for_cpu(vec![9, 9, 9, 9]);
        let buf = buf.cpu_take();
        assert_eq!(buf.cpu_read(), &[9, 9, 9, 9]);
    }

    #[test]
    fn completion_evidence_must_match_operation() {
        let op = OperationId::next();
        let buf = Buffer::new_cpu(8);
        let buf = buf.sync_for_device(DeviceDmaAddress(1), op).device_take();
        let wrong = CompletionToken {
            operation: OperationId::next(),
            complete: true,
        };
        let (err, back) = buf.device_complete(wrong).unwrap_err();
        assert_eq!(err, BufferError::MissingCompletionEvidence);
        // 证据不符 → Buffer 原样退回，仍处 DeviceOwned；
        // 测试收尾走人工确认出口（真实系统必须先取得"设备已停"证据）。
        back.release_after_human_confirmation();
    }

    #[test]
    #[should_panic(expected = "DroppedWhileDeviceHeld")]
    fn dropping_device_owned_panics_in_debug() {
        let op = OperationId::next();
        let buf = Buffer::new_cpu(8);
        let buf = buf.sync_for_device(DeviceDmaAddress(1), op).device_take();
        drop(buf); // 必须先 device_complete
    }

    #[test]
    #[should_panic(expected = "DroppedWhileDeviceHeld")]
    fn dropping_synced_but_not_taken_panics_in_debug() {
        let op = OperationId::next();
        let buf = Buffer::new_cpu(8);
        let buf = buf.sync_for_device(DeviceDmaAddress(1), op);
        drop(buf);
    }

    #[test]
    fn address_domains_are_distinct_newtypes() {
        // 同一数字在不同地址域是不同类型（12.5 禁止裸 u64 混用）
        let va = CpuVirtualAddress(0x1000);
        let pa = CpuPhysicalAddress(0x1000);
        let da = DeviceDmaAddress(0x1000);
        assert_ne!(format!("{va}"), format!("{pa}"));
        assert_ne!(format!("{pa}"), format!("{da}"));
    }
}

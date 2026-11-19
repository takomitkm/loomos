//! Domain Adapter 插槽（architecture/06 §6.6：未来领域只经此接入，**不依赖 ROS 2 类型**）。
//!
//! 定位：Device Core 只统一身份/权限/生命周期/租约/操作跟踪/遥测/故障；
//! 领域语义（机器人 MotionGoal、车辆 Trajectory 等）留在各自 Domain——
//! 本插槽定义「一个 Domain 如何把自己暴露成 DeviceProvider 集合」的边界，
//! 现在只有契约与一个真实在用的示例（loomos-vision::VisionProvider 即此模式）。
//!
//! 禁止：在本插槽引入 ROS 2 / MoveIt / Gazebo 类型；领域适配器实现本 trait 时
//! 自带自己的领域 crate。

use crate::core::{DeviceDescriptor, DeviceError, DeviceProvider, ProviderId};
use serde_json::Value;

/// Domain Adapter：把一个领域（机器人/车辆/安防…）的设备暴露给 DeviceRegistry。
///
/// 契约（architecture/06 §6.5/§6.6）：
/// 1. 只产出标准 `DeviceDescriptor` + 领域自有操作名——**不做 turn_on/off 万能 API**；
/// 2. 操作参数/结果一律 `serde_json::Value`，领域类型由 Domain crate 定义并在
///    适配器边界做转换（跨域只见 JSON，不见领域类型）；
/// 3. 危险操作（运动使能、断电等）必须声明 `risky: true`，由上层确认策略闸口拦截；
/// 4. 与 ROS 2 等中间件的桥接属于 Adapter 内部实现，Device Registry 感知不到。
pub trait DomainAdapter: Send {
    /// 领域标识（如 "robotics" / "vision" / "home"）。
    fn domain(&self) -> &str;

    /// 领域暴露的设备集合（每次调用重新发现，Registry 缓存由上层管）。
    fn devices(&self) -> Vec<DeviceDescriptor>;

    /// 领域操作执行（Device Registry 的 provider 层在 capability 校验后调用）。
    fn execute(&mut self, device: &str, op: &str, params: &Value) -> Result<Value, DeviceError>;

    /// 危险操作声明表（Registry 装配期检查：risky 操作必须同时声明 capability）。
    fn risky_operations(&self) -> Vec<(&'static str, &'static str)> {
        // (device_id, op) 列表；默认无
        Vec::new()
    }
}

/// DomainAdapter → DeviceProvider 的标准包装：任何领域适配器都能注册进 Registry。
pub struct DomainAdapterProvider<A: DomainAdapter> {
    adapter: A,
    id: ProviderId,
}

impl<A: DomainAdapter> DomainAdapterProvider<A> {
    pub fn new(adapter: A) -> Self {
        let id = format!("domain-{}", adapter.domain());
        Self { adapter, id }
    }
}

impl<A: DomainAdapter + 'static> DeviceProvider for DomainAdapterProvider<A> {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn discover(&mut self) -> Vec<DeviceDescriptor> {
        self.adapter.devices()
    }

    fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &Value,
    ) -> Result<crate::core::OpOutcome, DeviceError> {
        // 危险操作闸口：risky 操作在此层直接拒绝（上层确认策略通过后才放行——
        // 原型阶段的策略位，未来接 loomos-home::confirm 一次性确认）
        if self
            .adapter
            .risky_operations()
            .iter()
            .any(|(d, o)| *d == device && *o == op)
        {
            return Err(DeviceError::PermissionDenied(format!(
                "{device}/{op} 是危险操作，未经确认闸口放行不得执行"
            )));
        }
        self.adapter
            .execute(device, op, params)
            .map(crate::core::OpOutcome::Done)
    }

    fn health(&self, device: &str) -> Result<crate::core::DeviceHealth, DeviceError> {
        if self.adapter.devices().iter().any(|d| d.id == device) {
            Ok(crate::core::DeviceHealth::Ok)
        } else {
            Err(DeviceError::NoSuchDevice(device.to_string()))
        }
    }

    fn telemetry(&self, _device: &str) -> Result<Value, DeviceError> {
        Ok(Value::Object(Default::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeRobotDomain {
        armed: bool,
    }

    impl DomainAdapter for FakeRobotDomain {
        fn domain(&self) -> &str {
            "robotics"
        }

        fn devices(&self) -> Vec<DeviceDescriptor> {
            vec![DeviceDescriptor {
                id: "arm.main".into(),
                provider: "domain-robotics".into(),
                kind: "robot".into(),
                version: 0,
                capabilities: vec![],
            }]
        }

        fn execute(
            &mut self,
            device: &str,
            op: &str,
            _params: &Value,
        ) -> Result<Value, DeviceError> {
            match (device, op) {
                ("arm.main", "get_state") => Ok(json!({"armed": self.armed})),
                _ => Err(DeviceError::UnsupportedOp(device.into(), op.into())),
            }
        }

        fn risky_operations(&self) -> Vec<(&'static str, &'static str)> {
            vec![("arm.main", "arm_power_on")]
        }
    }

    #[test]
    fn domain_adapter_wraps_into_provider() {
        let mut p = DomainAdapterProvider::new(FakeRobotDomain { armed: false });
        assert_eq!(p.id(), "domain-robotics");
        let devs = p.discover();
        assert_eq!(devs[0].id, "arm.main");
        // 普通操作放行
        let out = p.execute("arm.main", "get_state", &json!({})).unwrap();
        assert!(matches!(out, crate::core::OpOutcome::Done(_)));
        // 危险操作被闸口拒绝（即使领域内部也支持）
        let err = p
            .execute("arm.main", "arm_power_on", &json!({}))
            .unwrap_err();
        assert!(err.to_string().contains("危险操作"));
    }

    #[test]
    fn health_unknown_device() {
        let p = DomainAdapterProvider::new(FakeRobotDomain { armed: false });
        assert!(p.health("ghost").is_err());
    }
}

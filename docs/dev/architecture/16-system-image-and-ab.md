# 16 — 系统镜像与 A/B 部署

> **层级**：`architecture/` 跨机型现行版（编号顺延）。裁定依据：`engineering/ADR-013`。
> 板级启动链事实在 `platforms/FMQL30TAI676H/`；本文件只写跨机型契约。

## 16.1 布局契约

```
Bootloader / U-Boot
        |
        v
   Select Boot Slot
        |
   +----+----+
   |         |
   v         v
 System A  System B      ← 两个完整、可独立引导的系统镜像（Nix 构建）
   |         |
   +----+----+
        |
        v
 Shared Persistent Data  ← /var/lib/loomos 等；与系统槽分离；迁移须回滚兼容
```

- 系统槽不可变（Nix Store 模型同构）；可变数据只进共享分区；
- Nix generations ≠ 硬件 A/B：前者同系统代际回滚，后者整槽回退，两者互补。

## 16.2 更新事务（13 步）

1. Nix 构建完整新系统镜像 → 2. RAUC bundle → 3. 校验镜像校验和 →
4. 受保护密钥签名（密钥在 Nix 之外）→ 5. 传输 → 6. 板端验签 + 板型兼容 →
7. 写非活动槽 → 8. 回读校验 → 9. 配置 bootloader 试引导 → 10. 重启 →
11. 健康检查 → 12. 全过才标记 successful → 13. 失败由 bootloader 计数回退旧槽。

数据迁移纪律：**不可逆迁移等新槽 successful 后才允许**；升级前共享数据写法
保持旧槽可读。

## 16.3 健康检查契约（槽位成功判定）

最低集（可按机型增补）：
1. loomosd systemd 单元 active ≥ 观察窗（默认 30s，含 Supervisor 正常自检）；
2. `loomos-cli smoke --quick` 通过（或其等价 IPC 探针）；
3. WAMR 运行时初始化成功 + 首个组件实例化成功（stub 档允许 Development 标记）；
4. 存储水位（StateDirectory 所在分区 < 阈值）。

健康检查失败 = 槽位不成功 → 计数回退；检查项本身必须幂等、无副作用。

## 16.4 测试矩阵（自动化 + QEMU，先行于板测）

成功更新 / 更新中断（写槽中重启）/ 非法签名 / 校验和不符 / 板型不匹配 /
新槽引导失败 / 健康检查失败 / boot 计数达上限回退 / 共享数据迁移被安全拒绝 /
旧镜像完好。板端 bootloader 能力（U-Boot bootenv/bootcount 语义）为
**Unverified**，调查后补 `platforms/<机型>/` 证据。

## 16.5 本阶段边界（2026-11-22 更新）

- **不改板端分区**；不生成真实分区表；
- RAUC 为优先评估项；bundle 组装骨架已交付（`nix/images/ab-release.nix`，
  manifest.raucs INI + loomis-ab BundleManifest 同源校验和）；
- **事务/回滚状态机已全量自动化**：`crates/loomos-ab`（25 测试覆盖 §16.4 全矩阵：
  中断/篡改/板型/计数耗尽/健康检查失败/迁移纪律/旧镜像完好）；
- 板端 U-Boot bootenv/bootcount 语义调查（Unverified）→ 证据进 platforms/ 后
  才产出真实 slot 映射；
- 签名/分发链的密钥管理（外部供给）与 SBOM 在 P5 随镜像工作展开。

---
title: "ADR-013: A/B 系统镜像部署与回滚"
version: 1.0
status: accepted
date: 2026-11-22
deciders: takomitkm
tags:
  - ab-update
  - rauc
  - uboot
  - deployment
  - rollback
  - architecture-decision
---

# ADR-013: A/B 系统镜像部署与回滚

## 状态

**Accepted**（2026-11-22）。证据等级：架构裁定；槽位/分区布局与 U-Boot 环境变量
为 **Unverified**（本阶段不改板端分区，见 §证据边界）。

## 背景

1. 现行部署是**单槽**手工安装（tarball → /opt/loomos → systemd），升级失败只能
   现场手工恢复。
2. **Nix generations ≠ 硬件 A/B**：Nix 的代际回滚发生在同一套可引导系统内；
   A/B 是两个完整、可独立引导的系统槽。两者互补，不能互相替代。
3. 板端 U-Boot 2022.10 + FSBL/BL31 启动链（见 platforms/FMQL30TAI676H）。
   **U-Boot 版本号本身不证明 boot 环境/分区布局已具备**——必须先调查现有
   boot 配置再产出真实分区方案。

## 决定

1. **双槽 + 共享持久数据**：

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
 Shared Persistent Data  ← 可变应用数据（/var/lib/loomos 等），与系统槽分离
```

2. **更新框架：优先评估 RAUC**（[rauc.readthedocs.io](https://rauc.readthedocs.io/)）。
   以 U-Boot 兼容的 boot 槽选择机制（bootenv/bootcount/altbootcmd）接入，前提是
   实际 boot 环境支持——未经调查不生成真实分区布局。RAUC 不可行时以书面替代
   评估（如自实现 updater + U-Boot bootcount 语义）另行裁定。
3. **更新事务**（Nix 构建新镜像 → RAUC bundle → 板端事务）：

   1. Nix 构建完整新系统镜像；2. 打 RAUC bundle；3. 校验全部镜像校验和；
   4. 用受保护签名密钥**签名 bundle**（密钥在 Nix 之外）；5. 传输；
   6. 板端验签 + 板型兼容校验；7. 写入非活动槽；8. 回读校验写入结果；
   9. 配置 bootloader 试引导非活动槽；10. 重启；11. 健康检查；
   12. 全部通过才标记槽位 successful；13. 失败（引导或健康检查）由 bootloader
   计数回退到旧槽。

4. **数据迁移与回滚兼容**：共享数据区的不可逆迁移必须等新槽被确认 successful
   之后才允许执行；升级前的共享数据写法必须保持旧槽可读。
5. **密钥与工件**：签名密钥不入 Nix/Store/镜像；每个 released bundle 带来源
   （源 commit / flake.lock / WAMR 版本 / Host ABI 版本 / 目标平台 / 构建旗标）
   与校验和。
6. **本阶段不改板端分区**。板端 bootloader 能力与存储限制如实记录
   `Unverified`；全部回滚逻辑先经自动化测试与 QEMU/引导状态模拟验证。

## 测试要求（自动化 + QEMU/模拟）

成功更新 / 更新中断 / 非法签名 / 校验和不符 / 板型不匹配 / 新槽引导失败 /
健康检查失败 / boot 计数达上限回退 / 旧槽完整回收 / 共享数据迁移被安全拒绝 /
旧镜像保持完好。

## 后果

### 正面

1. 升级失败自动回退，设备不再变砖。
2. 系统槽不可变 + 数据分离，与 Nix 不可变 Store 模型一致。
3. 签名 bundle 让交付链可审计。

### 负面 / 代价

1. 双槽占用约 2× 系统镜像存储——板端 flash 预算必须核算（1 GB RAM 约束之外，
   存储约束单独评估）。
2. RAUC + U-Boot 环境需要板端调查（Unverified）。
3. 数据迁移纪律（回滚可读）约束应用层 schema 演进节奏。

### 证据边界

- 槽位/分区真实布局：**Unverified**——需板端 boot 配置调查（L2 板测，不进无人值守 CI）。
- 事务/回滚状态机：可用自动化测试验证（QEMU/模拟）。
- 物理板验证：本阶段不要求。

## 相关

- `docs/dev/architecture/16-system-image-and-ab.md`：架构细则
- `docs/dev/engineering/ADR-012-Nix系统重建与包管理权威.md`：镜像由 Nix 构建
- [RAUC 文档](https://rauc.readthedocs.io/)
- `docs/dev/platforms/FMQL30TAI676H/`：板端启动链事实（U-Boot 2022.10 / FSBL / BL31）

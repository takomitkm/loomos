---
title: "ADR-012: Nix 作为 LoomOS 系统构建与包管理权威"
version: 1.0
status: accepted
date: 2026-11-22
deciders: takomitkm
tags:
  - nix
  - nixos
  - reproducibility
  - deployment
  - architecture-decision
---

# ADR-012: Nix 作为 LoomOS 系统构建与包管理权威

## 状态

**Accepted**（2026-11-22）。证据等级：架构裁定；Nix 实现尚为 **Unverified**
（本机无 Nix，验证依赖 CI 的 `nix flake check` / `nix build`，见 P0 验收门）。

## 背景

现行部署路径（`scripts/release-aarch64.sh` → `deploy/install-board.sh` → systemd）
是手工安装：解 tarball 到 `/opt/loomos`、建用户/组、装 unit。它不是可复现的系统
包生命周期：

1. 构建输入（工具链、WAMR、LLVM、依赖库版本）不受仓库声明控制。
2. Ubuntu 20.04 的 APT/dpkg 是事实上的包管理权威，与仓库无对应关系。
3. 无法在独立 CI 基础设施上重建同一系统；无法声明式回滚。
4. 板端约 1 GB RAM——在设备上构建任何东西都不可行。

同时必须明确：**WAMR 只管 Wasm 执行**。编译器、native 库、内核、驱动、固件、
systemd、udev、用户身份、系统镜像都不归 WAMR 管，必须有一个单一声明权威。

## 决定

1. **Nix 是 LoomOS 全部 WAMR 执行职责之外依赖的单一声明权威**：
   Rust 工具链、LLVM/wamrc、WAMR、Native Host（loomosd/loomos-cli）、Wasm 组件
   构建、AOT 产物、ONNX Runtime、sherpa-onnx、厂商 SDK、内核/驱动/固件工件、
   systemd/udev 配置、用户与组、文件系统布局、部署与 A/B 更新工件。
2. **最终 Linux 产品形态：NixOS 配置模块或等价的完全 Nix 构建系统镜像**；
   APT/dpkg 不再是产品镜像的包生命周期权威。厂商 bootloader/内核阻止标准 NixOS
   直接部署时，采用**过渡路径**（厂商内核 + 固定 BSP 工件，Nix 仍为系统构建权威），
   过渡不削弱最终目标。
3. **Nix ≠ NixOS**：Nix=包/构建引擎；Nixpkgs=包定义；Flakes=钉死输入的工程入口；
   NixOS=声明式 OS 配置。嵌入式目标最终用 NixOS 模块或等价全 Nix 镜像。
4. **Nix 不执行运行时权限**。Nix 只声明式**重建**权限相关系统配置（服务用户、
   组、设备节点规则、systemd 沙箱项、内核命令行、文件系统属主）；应用级能力检查
   在 Native Host（CapabilityStore），内核级强制在 Linux。内核缺少 cgroups/seccomp/
   LSM 时，如实标注 `unsupported` / `best_effort`，沿用 architecture/11 的实施等级纪律。
5. **厂商不可重建依赖如实分级**：`SourceReproducible`（可从源码重建）/
   `ArtifactReconstructible`（可由工件+哈希重建）/ `ExternallyProvisioned`（授权用户
   外部获取，哈希校验）/ `Unavailable`（缺失）。不得虚构源码可得性，不得声称闭源
   SDK「已复现编译」。厂商工件登记于 `nix/vendor/sources.nix`，凭证/私钥/可变运行
   数据一律不进 Flake 与不可变 Store。
6. **可复现性控制**：钉死 Flake 输入（flake.lock 入库）、Cargo.lock 入库、Rust 工具链
   钉死、WAMR commit 钉死、wamrc/LLVM 版本钉死、固定构建旗标、构建期无未声明网络
   访问；并以「两个干净 builder 独立重建 + 哈希比对」为复现验收——单次 `nix build`
   成功不是复现证明。
7. **构建在开发机/CI，不在板端**：目标机只接收预构建、经哈希验证的 Nix 闭包或
   完整系统镜像；不可变（Nix Store）与可变（`/var/lib/loomos`）分离；规划存储
   GC 防止 flash 写满。

## 职责矩阵

| 领域 | Nix 职责 |
|---|---|
| OS 用户态 | 构建与打包 |
| Rust 工具链 / LLVM / Clang | 钉死并供给 |
| WAMR / wamrc | 构建并打包 |
| Native Host（loomosd / loomos-cli） | 构建并打包 |
| Wasm 组件 / AOT 产物 | 构建 / 按目标生成 |
| ONNX Runtime / sherpa-onnx | 构建或打包钉死工件 |
| 厂商 SDK | 钉死来源/版本/哈希/许可证，按 §决定 5 分级 |
| 内核 / 驱动 / 固件 | 构建或钉死工件（BSP 不假设可被通用 NixOS 内核替代） |
| systemd / udev / 用户组 / 文件系统 | NixOS 模块声明 |
| 部署 / A/B 更新 | 构建镜像与更新工件（机制见 ADR-013） |
| 模型 | 适当的不可变模型版本打包 |
| 可变运行态 / 秘密 | 留在 Store 之外（运行时外部供给） |

## 后果

### 正面

1. 系统可从仓库输入独立重建；版本与来源全程可追溯。
2. A/B、回滚、镜像生成全部声明化（与 ADR-013 衔接）。
3. 厂商依赖有登记与哈希纪律，可审计、可追责、可替换。
4. 开发环境（`nix develop`）与生产构建同一套钉死输入。

### 负面 / 代价

1. 仓库引入 Nix 语言与闭包管理的学习/维护成本。
2. Ubuntu 板的过渡期是**双轨**：APT 仍是旧板的兜底安装路径，直到 Nix 路径通过
   验收门（P5）才退役 `deploy/install-board.sh`。在过渡路径上**不得声称**包管理
   权威已被替换。
3. 复现验收需要双 builder 基础设施（CI 承担）。

### 证据等级

- 架构裁定：Accepted（本 ADR）。
- P0 交付物（flake.nix / nix/ 树）：`Unverified`——本机无 Nix，等 CI `nix flake check`。
- 板端镜像 / APT 退役：未开始（P5）。

## 替代方案考虑

| 方案 | 为什么不选 |
|---|---|
| 维持 tarball+脚本部署 | 不可复现、不可审计、回滚靠手工 |
| 只在 Ubuntu 上加装 Nix 当开发工具 | 不等于包管理权威替换——正是本 ADR 禁止的自欺终点 |
| 嵌入式 Yocto / Buildroot | 同样可复现，但与仓库 Rust/CI 生态及 NixOS 声明模型契合度低；且 A/B 与闭包管理需另建 |
| Debian 容器化交付 | 宿主 OS 与容器层仍是双权威；不是系统级声明 |

## 相关

- `docs/dev/architecture/14-nix-system-reconstruction.md`：架构细则与仓库 Nix 结构
- `docs/dev/engineering/ADR-011-统一Wasm执行.md` v1.1：WAMR 执行职责边界
- `docs/dev/engineering/ADR-013-AB系统镜像与回滚.md`：A/B 与回滚
- `nix/vendor/sources.nix`：厂商工件登记正本
- `deploy/README.md`：过渡期部署路径与验收门

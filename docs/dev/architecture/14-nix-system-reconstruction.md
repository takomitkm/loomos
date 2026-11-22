# 14 — Nix 系统重建与包管理

> **层级**：`architecture/` 跨机型现行版（编号顺延 11/12/13 → 14）。裁定依据见
> `engineering/ADR-012`；机型落地与证据进 `platforms/<机型>/`。

## 14.1 职责三分（与 03 的衔接）

| 层 | 管 | 不管 |
|---|---|---|
| Nix/NixOS | 构建输入、native 包、系统配置、镜像、A/B 工件、包生命周期 | 运行时授权（那是 Native Host） |
| WAMR（loomos-wamr） | Wasm 执行与其运行时资源 | 编译器/LLVM/内核/驱动/OS 包 |
| Native Host（loomosd） | 权威组件（CapabilityStore/Lease/Supervisor/Lifecycle/配置权威）+ 平台适配 + 硬件 | 业务逻辑执行（那是 Wasm 组件） |
| Linux | 内核级强制 | — |

**Nix 不执行运行时权限**：NixOS 模块声明式**重建**权限相关配置（服务用户/组、
DeviceAllow、systemd 沙箱项、内核命令行）；应用级能力检查在 CapabilityStore；
内核级强制在 Linux。内核缺机制（cgroups/seccomp/LSM）→ 如实 `unsupported` /
`best_effort`（architecture/11 纪律），Nix 无法凭配置创造内核能力。

## 14.2 仓库 Nix 结构（P0 已交付，Unverified 待 CI）

```
flake.nix                    # 钉死 nixpkgs(nixos-26.05) / rust-overlay(1.99.1+wasm32-wasip1) / WAMR 源码
nix/
├── overlays/default.nix     # loomos-host / loomos-wamr / loomos-wamrc / loomos-components / vendor-sdk
├── packages/*.nix           # 各 derivation；loomos-host 构建即跑 workspace cargo test（Linux 验收门）
├── modules/{base,loomos}.nix# services.loomos NixOS 模块（沙箱/用户组/DeviceAllow 声明式重建）
├── systems/*.nix            # generic x86_64 / aarch64 / fmql30tai676h 过渡占位
└── vendor/sources.nix       # 厂商工件登记正本（四级分类 + 哈希纪律）
```

## 14.3 钉死与复现

- 钉死清单：nixpkgs 分支（flake.lock 入库即锁）、Rust 1.99.1（≡ rust-toolchain.toml，
  防漂移门 `checks.toolchain-version-check`）、WAMR main@06f6c8c（flake input）、
  LLVM 17（nixpkgs）、Cargo.lock（cargoLock.lockFile）。
- 复现门：**单次 `nix build` 成功不是复现证明**——两个干净 builder 独立重建 +
  产物哈希比对（CI 承担）。
- 构建**不在板端**（~1GB RAM）：设备只收预构建闭包/镜像；不可变（Store）与可变
  （`/var/lib/loomos`，StateDirectory）分离；秘密不入 Store。

## 14.4 厂商工件分级

| 分类 | 语义 | 本仓实例 |
|---|---|---|
| SourceReproducible | 源码可重建 | sherpa-onnx 2.0.2 |
| ArtifactReconstructible | 工件+哈希重建 | onnxruntime v20（307 槽位）、silero_vad.onnx |
| ExternallyProvisioned | 授权获取，哈希校验 | FMQL BSP 内核 5.4.52 / 厂商 NPU Runtime / U-Boot 2022.10 |
| Unavailable | 缺失 | —（出现即登记） |

登记正本 `nix/vendor/sources.nix`；未提供工件在 `nix build .#loomos-vendor-sdk`
时 throw 明细（获取方式+哈希填入指引）。**不虚构源码可得性、不声称闭源 SDK 已复现编译。**

## 14.5 过渡路径（Ubuntu 板 → Nix 管理）

1. **过渡期**：APT 兜底保留（`deploy/install-board.sh` 退役前不删），但**不得宣称**
   包管理权威已替换；
2. **P5 验收门**：Nix 镜像在 QEMU 可引导 + loomosd 健康检查 + 回滚演练通过后，
   板端切换 Nix 闭包/镜像交付；
3. 厂商内核接入按 ExternallyProvisioned 工件（哈希校验）替换 `boot.kernelPackages`，
   不在 Nix 里虚构编译。

## 14.6 开放问题（进 10 的引用）

- flake.lock 首次生成与 CI 接入（P0 收尾）；
- aarch64 交叉构建（`loomos-host-aarch64`）在 CI 的可用性；
- 板端 flash 预算下的 Store GC 策略（P5）。

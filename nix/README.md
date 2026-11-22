# nix/ — loomos 系统重建与包管理（ADR-012）

> 证据等级：**Unverified（待 CI）**。本机（Windows）无 Nix；`nix flake check` /
> `nix build` 在 Linux CI 执行。flake.lock 由首次 `nix flake lock` 生成后入库。

## 结构

```
flake.nix                  # 入口：钉死 nixpkgs(nixos-26.05) / rust-overlay / WAMR 源码
nix/
├── overlays/default.nix   # 产物挂入 pkgs：loomos-host / wamr / wamrc / components / vendor-sdk
├── packages/
│   ├── loomos-host.nix        # loomosd + loomos-cli（构建时跑 workspace cargo test）
│   ├── wamr.nix              # libiwasm.a + iwasm（AOT+INTERP+WASI P1 最小集）
│   ├── wamrc.nix             # AOT 编译器（build 机运行，--target 决定产物平台）
│   ├── loomos-components.nix # wasm32-wasip1 组件（.wasm 正本）
│   ├── vendor-sdk.nix        # 厂商工件校验入口（未提供 → throw 明细）
│   ├── check-fmt.nix         # cargo fmt --check
│   └── check-toolchain-version.nix # 防漂移：Nix 工具链 ≡ rust-toolchain.toml
├── modules/
│   ├── base.nix              # 跨机型公共（stateVersion / 服务默认开）
│   └── loomos.nix            # services.loomos：用户/组/systemd 沙箱/DeviceAllow
├── systems/
│   ├── x86_64-linux.nix      # nixosConfigurations.generic-x86_64
│   ├── aarch64-linux.nix     # nixosConfigurations.generic-aarch64
│   └── fmql30tai676h.nix     # nixosConfigurations.fmql30tai676h（过渡期占位）
└── vendor/sources.nix        # 厂商工件登记正本（四级分类 + 哈希纪律）

nix/images/                  # A/B 发布工件层（P5；Unverified 板端）
├── ab-release.nix           # RAUC bundle 组装（manifest.raucs + loomis-ab 清单同源校验和）
├── rauc-system-conf.nix     # system.conf 模板（bootenv 语义待板端调查）
└── README.md                # 状态机在 crates/loomis-ab（25 测试）；真实镜像/分区 = P5
```

## 命令

```bash
nix flake check              # 评估 nixosConfigurations + 构建四个包 + fmt + 工具链防漂移
nix build                    # = .#loomos-host（含 workspace cargo test —— Linux 侧验收门）
nix build .#loomos-wamr      # libiwasm.a（wamr-link 开发档链接用）
nix build .#loomos-wamrc     # wamrc
nix build .#loomos-components  # wasm32-wasip1 组件
nix build .#loomos-host-aarch64    # 交叉构建 aarch64 Host（Unverified，待 CI）
nix develop                  # 钉死工具链开发环境（rust 1.99.1 + wasm32-wasip1）
nix run .#smoke              # agent/run-prototype.sh（钉死工具链）
```

## 职责边界（与 ADR-011 v1.1 衔接）

| 层 | 管 |
|---|---|
| Nix（本目录） | 构建输入、native 包、系统配置、镜像、A/B 工件 |
| WAMR（loomos-wamr） | 只管 Wasm 执行与其运行时资源 |
| Native Host（loomosd） | 权威组件：CapabilityStore / Lease / Supervisor / Lifecycle / 配置权威 |
| Linux | 内核级强制 |

Nix **不执行**运行时权限，只**声明式重建**权限相关配置（services.loomos 的
沙箱项 / DeviceAllow / 用户组）；内核缺机制时在机型文档记 `unsupported`，
不虚报。秘密/可变数据不进 Store（StateDirectory=/var/lib/loomos）。

## 复现验收（ADR-012 §决定 6）

单次 `nix build` 成功不是复现证明。复现门：两个干净 builder 从同一
flake.lock 独立重建 + 产物哈希比对（CI 承担，P0 后接入）。

## 当前钉死清单

| 输入 | 值 |
|---|---|
| nixpkgs | github:NixOS/nixpkgs/nixos-26.05（flake.lock 生成后锁定） |
| rust | 1.99.1（= rust-toolchain.toml；+ wasm32-wasip1 target） |
| WAMR | main @ 06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf（2026-11-20） |
| LLVM/wamrc | llvmPackages_17（nixpkgs，随 flake.lock 锁定） |
| Cargo.lock | 仓库入库，cargoLock.lockFile 校验 |

## 已知限制（Unverified 明细）

- flake.lock 未生成（需 Linux/CI 首跑 `nix flake lock` 后入库）。
- rust-overlay 的 1.99.1 钉死、makeRustPlatform 组合、aarch64 交叉：待 CI 验证。
- loomos-host 的 Linux 全量 cargo test：Windows 侧 154+ 测试全绿；Linux 侧以
  `nix build .#loomos-host` 首跑结果为准。
- 板端镜像 / A/B 分区：P5，不在 P0（fmql30tai676h.nix 为可评估过渡占位）。

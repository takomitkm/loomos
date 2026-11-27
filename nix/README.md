# nix/ — loomos 系统重建与包管理（ADR-012）

> 证据等级：**已验证（CI）**。`nix flake check -L` + 四包 `nix build` 于 Linux CI
> 全绿（run 46030198570，2026-11-27，commit 25fd929）。flake.lock 已入库（736a3bd）。
> 板端镜像/A/B 分区仍 Unverified（P5，见 nix/images/README.md）。

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
nix build .#loomos-host-aarch64    # 交叉构建 aarch64 Host（flake check 评估通过）
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
| nixpkgs | github:NixOS/nixpkgs/nixos-26.05（flake.lock 已锁定：736a3bd） |
| rust | 1.99.1（= rust-toolchain.toml；+ wasm32-wasip1 target） |
| WAMR | main @ 06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf（2026-11-20） |
| LLVM/wamrc | llvmPackages_18（wamrc.nix；WAMR_BUILD_WITH_CUSTOM_LLVM=1） |
| Cargo.lock | 仓库入库，cargoLock.lockFile 校验 |

## 已知限制（如实标注）

- 板端镜像 / A/B 分区 / bootenv：P5 板端调查，不在 P0（fmql30tai676h.nix 为可评估过渡占位）。
- 复现门（双干净 builder 独立重建 + 哈希比对）：CI 尚未接入（ADR-012 §决定 6）。
- home/executor/audio/vision 业务组件迁移：N3 剩余项（接入点已留）。

## NiX 实证教训（多轮 CI 实锤，写新 derivation 前必读）

1. **`buildRustPackage` 没有 `buildPhaseCargoCommand` 属性**——写了被静默忽略，
   钩子按默认跑全 workspace host 构建，cdylib 产出 `.so` 而非 `.wasm`。
   正解：显式设 `buildPhase` 完全接管（cargo-build-hook 仅在 buildPhase 未设时
   才接管，且硬编码 `--target @rustcTargetSpec@`=host；`cargoBuildFlags` 只能追加
   旗标，追加第二个 `--target` 报重复）。参见 loomos-components.nix。
2. **'' 块内「美元符加花括号」恒被 NiX 求值期插值，注释里也不例外**；
   `\${` 转义在本 NiX 版本不可靠（官方文档称合法，实测仍报 syntax error）。
   需要引用 `$out` 等时用无花括号 `$out`，彻底不出现该组合。
3. **CMake 产物 wamrc 是符号链接**（指向 wamrc-<version>）：`find -type f` 永远
   miss，须允许 `-type l` 且用精确名（`wamrc*` 会先命中 CMakeFiles 目录）+ `cp -L`。
4. **wamrc 的 LLVM**：须传 `-DWAMR_BUILD_WITH_CUSTOM_LLVM=1` +
   `-DLLVM_DIR=<llvmPackages_18.llvm>/lib/cmake/llvm`，否则上游探测捆绑目录报
   'Cannot find LLVM dir'。
5. **NixOS 配置可引导性断言**（`nix flake check` 硬检查）：必须声明
   `fileSystems."/"`（device+fsType）与 `boot.loader.grub.devices`（**list of
   string**，单字符串报 TypeError）。
6. 本机（Windows）无 Nix；本地防线用 `python -I scripts/check-nix-interpolation.py`
   静态扫描 '' 块插值（启发式，不替代 `nix flake check`）。

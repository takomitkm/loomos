# nix/images/ — 系统镜像与 A/B 发布工件（P5 层）

> 证据等级：**Unverified（板端）**。本层是 ADR-013/architecture-16 的 Nix 侧骨架；
> 真实镜像生成（squashfs/ext4 rootfs、厂商内核接入、分区表）在板端 boot 环境
> 调查（U-Boot bootenv/bootcount 语义）之后展开——**本阶段不生成真实分区方案**。

## 已交付（框架层）

- **`ab-release.nix`**：RAUC bundle 组装 derivation——输入任意 rootfs 镜像文件，
  产出 `manifest.raucs`（RAUC INI 语义）+ `manifest.toml`（loomis-ab BundleManifest，
  校验和由 loomis-ab::Sha256Verifier 同源计算）+ 镜像文件。板端 RAUC 消费此 bundle。
- **`rauc-system-conf.nix`**：`system.conf` 模板（bootloader backend=uboot、
  slot 布局占位——真实 block-device 映射等板端调查）。

## 状态机侧（已全量自动化）

更新事务/回退逻辑不在 Nix——在 `crates/loomos-ab`（25 测试全绿，覆盖
ADR-013 §16.4 全矩阵：中断/篡改/板型/计数耗尽/健康检查失败/迁移纪律）。

## 待办（P5，按依赖顺序）

1. 板端 boot 环境调查（U-Boot 2022.10 bootenv 变量、bootcount 语义）→
   `platforms/FMQL30TAI676H/` 证据回填；
2. rootfs 镜像生成（NixOS `nixos-generate` 或 makeSquaredsquashfs 路线，
   厂商内核 ExternallyProvisioned 工件接入）；
3. 分区表 + RAUC slot 映射（真实 block device）；
4. QEMU 引导验证 + 健康检查联调；
5. 签名链（密钥外部供给）+ SBOM（flake.lock + Cargo.lock + WAMR rev + compat 键）。

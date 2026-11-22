# FMQL30TAI676H（悟净）过渡期系统配置 —— nixosConfigurations.fmql30tai676h 正本。
#
# 板级事实基线（docs/dev/platforms/FMQL30TAI676H/07-platform-baseline.md）：
#   Ubuntu 20.04.4 / aarch64 / 内核 5.4.52 / U-Boot 2022.10 + FSBL + BL31 / ~1GB RAM
#
# 过渡路径（ADR-012 §决定 2）：厂商 bootloader/内核暂阻标准 NixOS 直接上板——
# 本配置先用标准 aarch64 内核保证【可评估（nix flake check）/可在 QEMU 引导验证】；
# 厂商 BSP 内核是 ExternallyProvisioned 工件（nix/vendor/sources.nix → vendor-kernel-5.4.52），
# 上板替换 boot.kernelPackages 时按登记哈希校验工件，不在 Nix 里虚构编译。
# APT/dpkg 在旧板 Ubuntu 上的兜底路径在其通过 P5 验收门之前保留（deploy/README.md）。
#
# A/B 槽位分区：待板端 boot 环境调查（ADR-013 Unverified）——本文件不生成真实分区方案。
{ lib, ... }: {
  imports = [ ./aarch64-linux.nix ];

  # 1GB RAM：关闭无谓重量级组件
  documentation.nixos.enable = false;

  # 板端设备节点授权示例（按实际 DT 枚举补齐；引脚/设备树事实在机型文档）
  services.loomos.deviceNodes = lib.mkDefault [ ];

  # TODO(P5)：厂商内核 + DTB 接入、U-Boot bootenv 槽位选择、存储 GC 预算
}

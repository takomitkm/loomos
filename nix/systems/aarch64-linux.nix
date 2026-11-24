# 通用 aarch64-linux NixOS 配置（QEMU 交叉验证目标 / 通用 ARM 主机）。
# nixosConfigurations.generic-aarch64 的正本。
# FMQL30TAI676H 过渡期配置通过 import 本文件继承（NixOS 可引导性声明同源）。
{ lib, ... }: {
  services.loomos.enable = true;
  services.loomos.audioPlayback = lib.mkDefault false;

  # NixOS 可引导性声明（nix flake check 硬断言，CI 实证）：
  # 必须声明根文件系统（/）与 GRUB 引导设备。此处为开发/QEMU 评估占位，
  # P5 镜像生成时替换为真实设备路径。
  fileSystems."/" = {
    device = "/dev/disk/by-uuid/loomos-root-dev";
    fsType = "ext4";
  };
  boot.loader.grub.devices = "/dev/sda";
}

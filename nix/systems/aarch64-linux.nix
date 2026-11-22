# 通用 aarch64-linux NixOS 配置（QEMU 交叉验证目标 / 通用 ARM 主机）。
# nixosConfigurations.generic-aarch64 的正本。
{ lib, ... }: {
  services.loomos.enable = true;
  services.loomos.audioPlayback = lib.mkDefault false;
}

# 通用 x86_64-linux NixOS 配置（开发/CI 参考目标）。
# nixosConfigurations.generic-x86_64 的正本。
{ lib, ... }: {
  # 基础系统最小集：够 QEMU 引导 + loomosd 运行即可；镜像形态裁剪见 P5
  services.loomos.enable = true;
  services.loomos.audioPlayback = lib.mkDefault false;

  # NixOS 可引导性声明（nix flake check 硬断言，CI 实证）：
  # 必须声明根文件系统（/）与 GRUB 引导设备。此处为开发/QEMU 评估占位，
  # P5 镜像生成时替换为真实设备路径。
  fileSystems."/" = {
    device = "/dev/disk/by-uuid/loomos-root-dev";
    fsType = "ext4";
  };
  boot.loader.grub.devices = [ "/dev/sda" ];

  # SSH 留空（部署方按需启用）；不预置任何秘密
  # 网络策略 networking.nix 在 P5 随镜像工作展开
}

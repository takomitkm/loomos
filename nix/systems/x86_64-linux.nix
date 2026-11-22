# 通用 x86_64-linux NixOS 配置（开发/CI 参考目标）。
# nixosConfigurations.generic-x86_64 的正本。
{ lib, ... }: {
  # 基础系统最小集：够 QEMU 引导 + loomosd 运行即可；镜像形态裁剪见 P5
  services.loomos.enable = true;
  services.loomos.audioPlayback = lib.mkDefault false;

  # SSH 留空（部署方按需启用）；不预置任何秘密
  # 网络策略 networking.nix 在 P5 随镜像工作展开
}

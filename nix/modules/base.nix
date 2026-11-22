# 公共基础模块：所有 loomos 系统配置共享的最小集。
# 仅放跨机型公共项；机型差异进 nix/systems/<机型>.nix；板级事实进 docs/dev/platforms/。
{ lib, ... }: {
  system.stateVersion = lib.mkDefault "26.05"; # 与钉死的 nixpkgs 分支一致
  services.loomos.enable = lib.mkDefault true;

  # 声明式重建权限相关系统配置的原则（ADR-012 §决定 4）：
  # 这里只"声明配置"；应用级能力检查在 Native Host，内核级强制在 Linux。
  # 缺失的内核机制如实标注 unsupported/best_effort（architecture/11 纪律），不虚报。
}

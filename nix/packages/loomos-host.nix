# Native Host 包：loomosd + loomos-cli（ADR-011 v1.1：权威组件/平台适配/WAMR 宿主）。
# buildRustPackage 默认 doCheck=true → cargo test --workspace 随构建执行（Linux 侧验收门）。
# 构建在开发机/CI，不在板端（约 1 GB RAM——ADR-012 §决定 7）。
{ lib, loomos-rust-platform }:
loomos-rust-platform.buildRustPackage {
  pname = "loomos-host";
  version = "0.1.0";

  src = lib.cleanSource ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  # 默认 feature：全部 wamr/ort 真引擎档关闭——stub 档自包含可编译（IWASM_LIB_DIR 不需要）。
  # 生产档（wamr-link）由 loomos-wamr 产物链接，接入在 P1 收尾后切换。
  doCheck = true;

  meta = with lib; {
    description = "loomos Native Host：loomosd（系统装配）+ loomos-cli（验收/烟囱工具）";
    license = licenses.lgpl3Only;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}

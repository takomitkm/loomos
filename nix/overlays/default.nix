# loomos overlay：把本仓库产物挂进 pkgs。
# 由 flake.nix 注入 wamrSrc（钉死源码）；rust 工具链经 rust-overlay 的 rust-bin 精确钉死。
{ wamrSrc }:
final: prev: {
  # 钉死工具链（与 rust-toolchain.toml 同版本；一致性由 checks.toolchain-version-check 守护）
  loomosRustBuild = final.rust-bin.stable."1.99.1".minimal.override {
    targets = [ "wasm32-wasip1" ];
  };
  loomosRustDev = final.rust-bin.stable."1.99.1".default.override {
    targets = [ "wasm32-wasip1" ];
    extensions = [ "rust-src" ];
  };
  loomos-rust-platform = final.makeRustPlatform {
    cargo = final.loomosRustBuild.cargo;
    rustc = final.loomosRustBuild;
  };

  # WAMR 运行时（libiwasm.a + iwasm）
  loomos-wamr = final.callPackage ../packages/wamr.nix { wamrSrc = wamrSrc; };
  # wamrc AOT 编译器（构建机运行，产出目标平台 .aot）
  loomos-wamrc = final.callPackage ../packages/wamrc.nix { wamrSrc = wamrSrc; };
  # Native Host（loomosd + loomos-cli）
  loomos-host = final.callPackage ../packages/loomos-host.nix { };
  # Wasm 组件产物（wasm32-wasip1）
  loomos-components = final.callPackage ../packages/loomos-components.nix { };
  # 厂商工件入口（ExternallyProvisioned 未提供时 throw 明细——只在显式请求时构建）
  loomos-vendor-sdk = final.callPackage ../packages/vendor-sdk.nix { };
}

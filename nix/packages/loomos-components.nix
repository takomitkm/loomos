# Wasm 组件包：wasm32-wasip1 构建的 cdylib 组件。
# 当前组件：loomos-nlu-wasm-wrap（NLU C ABI 胶水，业务代码零改动——ADR-011 迁移规则）。
# .wasm 是正本分发产物；.aot 由部署层用 loomos-wamrc 按目标平台生成（ADR-011 §决定 7）。
{ lib, loomos-rust-platform }:
loomos-rust-platform.buildRustPackage {
  pname = "loomos-components";
  version = "0.1.0";

  src = lib.cleanSource ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  # 只构建组件 crate（workspace 其余成员是 Native Host / 基准，另行打包）。
  # 权威实证（nixpkgs@94c3211 pkgs/build-support/rust/build-rust-package/default.nix
  #           + hooks/cargo-build-hook.sh）：
  # - buildRustPackage **没有** buildPhaseCargoCommand 属性——写了会被静默
  #   忽略，钩子按默认行为跑全 workspace host 构建，cdylib 产出 .so 而非
  #   .wasm（CI 实证：编译列表含 loomosd/loomos-cli 等非依赖 crate）；
  # - cargo-build-hook.sh 硬编码 --target @rustcTargetSpec@（host 目标），
  #   仅当 buildPhase 未设时才接管为 cargoBuildHook——显式设 buildPhase
  #   即可完全接管构建命令，指定 -p 与 --target wasm32-wasip1；
  # - --offline：cargoLock 依赖已 vendor，与钩子行为保持一致。
  # 产物落点：cargo 默认 ./target（src 根）→
  #   ./target/wasm32-wasip1/release/loomos_nlu_wasm_wrap.wasm
  buildPhase = ''
    runHook preBuild
    cargo build --release -p loomos-nlu-wasm-wrap --target wasm32-wasip1 --offline
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p $out/components
    # 定位唯一产物：先搜已知落点 target/，再兜底跨挂载点全盘
    #（剪除 /nix store 避免误抓其他包的 wasm；剪除伪文件系统避免误扫耗时）
    found=$(find target -name 'loomos_nlu_wasm_wrap.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find / \( -path /nix -o -path /proc -o -path /sys -o -path /dev -o -path /run \) -prune -o -name 'loomos_nlu_wasm_wrap.wasm' -type f -print 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件（target/ 与全盘搜索均无 loomos_nlu_wasm_wrap.wasm）"; find target -name '*.wasm' -type f 2>/dev/null | head; exit 1; }
    cp "$found" $out/components/
    runHook postInstall
  '';

  # 组件行为等价由宿主侧基准（loomos-nlu-bench）与 CI QEMU 验证，不在本包内跑 wasm 单测
  doCheck = false;

  meta = with lib; {
    description = "loomos Wasm 组件产物（wasm32-wasip1，.wasm 正本）";
    license = licenses.lgpl3Only;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}

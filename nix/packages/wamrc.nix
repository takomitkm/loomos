# wamrc：AOT 编译器。
# 平台语义（易混点，勿混淆）：
#   - wamrc 本体构建在 build 机（本 derivation 的 system）；
#   - 它产出哪个平台的 .aot 由运行期 --target=<isa> 决定（host/target 平台）；
#   - 一个 x86_64 构建机的 wamrc 即可为 aarch64 产出 .aot（--target=aarch64）。
# AOT 兼容键（ADR-011 §4.4：WAMR 版本/构建配置/wamrc 版本/OS/ISA/CPU 基线/ABI/旗标）
# 由部署层在调用 wamrc 时记录；本 derivation 提供钉死的 wamrc 本体。
{ lib, stdenv, cmake, llvmPackages_18, wamrSrc }:
stdenv.mkDerivation {
  pname = "loomos-wamrc";
  version = "2026.11.20"; # 与 wamr.nix 同源同版本

  src = wamrSrc;
  sourceRoot = "source/wamr-compiler"; # 上游已改名（tools/wamrc → wamr-compiler，钉死 rev 实证）
  # WAMR build-scripts/version.cmake 写源码树（core/version.h，在 sourceRoot 之外）——
  # postPatch 覆盖不到，postUnpack 放开整树（CI -L 实证 Permission denied）
  postUnpack = ''
    chmod -R u+w source
  '';
  nativeBuildInputs = [ cmake ];
  buildInputs = [ llvmPackages_18.llvm ];

  cmakeFlags = [
    # 使用 Nix 提供的外部 LLVM（buildInputs），不探测上游捆绑的 core/deps/llvm/build
    # （CI 实证：不设此旗标时 llvm_env.cmake:31 报 'Cannot find LLVM dir'）
    "-DWAMR_BUILD_WITH_CUSTOM_LLVM=1"
    "-DLLVM_DIR=${llvmPackages_18.llvm}/lib/cmake/llvm"
  ];

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin
    # CMake 构建目录可能是 sourceRoot 下的 build/ 或 Nix 全局 /build。
    # 宽范围搜索（当前目录 + /build），失败时打印诊断。
    found=$(find . /build -name 'wamrc' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "wamrc 未产出（搜索范围: . /build）"; ls -R . 2>/dev/null | head -20; exit 1; }
    cp "$found" $out/bin/wamrc
    runHook postInstall
  '';

  meta = with lib; {
    description = "wamrc AOT 编译器（loomos 钉死构建；build 机运行，按 --target 产出 .aot）";
    license = licenses.asl20;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}

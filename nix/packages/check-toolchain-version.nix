# 防漂移门：Nix 钉死工具链版本必须与 rust-toolchain.toml 的 channel 一致。
# 漂移 = 构建不可比 → flake check 立即失败并给出两侧版本。
{ lib, stdenv, loomosRustDev }:
let
  lines = lib.splitString "\n" (builtins.readFile ../../rust-toolchain.toml);
  chanLines = lib.filter (l: builtins.match ".*channel[[:space:]]*=[[:space:]]*\"[^\"]+\".*" l != null)
    lines;
  want = if chanLines == [ ] then
    throw "rust-toolchain.toml 缺少 channel 行"
  else
    lib.head (builtins.match
      ''.*channel[[:space:]]*=[[:space:]]*"([^"]+)".*''
      (lib.head chanLines));
in
stdenv.mkDerivation {
  name = "loomos-toolchain-version-check";
  dontUnpack = true;
  nativeBuildInputs = [ loomosRustDev ];
  buildPhase = ''
    got=$(${loomosRustDev}/bin/rustc --version | sed -E 's/^rustc ([0-9]+\.[0-9]+\.[0-9]+).*/\1/')
    echo "rust-toolchain.toml channel = ${want}; nix rustc = $got"
    test "$got" = "${want}" || {
      echo "TOOLCHAIN DRIFT：rust-toolchain.toml(${want}) 与 Nix 工具链($got) 不一致"
      exit 1
    }
  '';
  installPhase = ''
    runHook preInstall
    touch $out
    runHook postInstall
  '';
}

# cargo fmt --all --check（钉死工具链；flake checks 项）
{ lib, stdenv, loomosRustDev }:
stdenv.mkDerivation {
  name = "loomos-cargo-fmt-check";
  src = lib.cleanSource ../..;
  nativeBuildInputs = [ loomosRustDev ];
  dontConfigure = true;
  doCheck = true;
  checkPhase = ''
    runHook preCheck
    cargo fmt --all --check
    runHook postCheck
  '';
  buildPhase = "";
  installPhase = ''
    runHook preInstall
    touch $out
    runHook postInstall
  '';
}

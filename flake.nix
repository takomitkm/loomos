# loomos Flake — Native Host + WAMR + Nix/NixOS 可复现平台（ADR-011 v1.1 / ADR-012 / ADR-013）
#
# 职责：Nix 管全部 WAMR 执行职责之外的构建输入与包生命周期。
# 结构见 nix/README.md；命令：
#   nix flake check          # 评估 + 构建 + 测试（Linux）
#   nix build .#loomos-host  # Native Host（含 workspace cargo test）
#   nix build .#loomos-components  # wasm32-wasip1 组件产物
#   nix develop              # 钉死工具链开发环境
#
# 注意：flake.lock 由首次 `nix flake lock`/build 生成后必须入库（P0 验收门）。
# 本机（Windows）无 Nix——以下全部产物在 CI/Linux 验证，证据等级 Unverified。
{
  description = "loomos — Native Host + WAMR + Nix/NixOS 可复现平台";

  inputs = {
    # 当期受支持稳定分支（2026-11：26.05；26.11 发布后可评估切换）。
    # 锁定发生在 flake.lock 入库时，不使用浮动引用。
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

    # 精确钉 rust-toolchain.toml（1.99.1）+ wasm32-wasip1 target。
    # 用它而非 nixpkgs rust 的原因：工具链版本必须与本地/CI rust-toolchain.toml 完全一致，
    # 且 wasm target 需要显式纳入（哈希由 overlay 内置管理，不手填）。
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    # WAMR 源码钉死（main @ 06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf，2026-11-20）。
    # 升级 = 改这里的 rev + flake.lock 重生成 + 重跑验收（ADR-012 §决定 6）。
    wamr-src = {
      url = "github:bytecodealliance/wasm-micro-runtime/06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, rust-overlay, wamr-src }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      overlays = [ rust-overlay.overlays.default self.overlays.default ];
      pkgsFor = system: import nixpkgs { inherit system overlays; };

      # 交叉：x86_64 构建机 → aarch64 目标（build/host/target 语义见 nix/packages/wamrc.nix）
      pkgsCrossAarch64FromX86 = import nixpkgs {
        localSystem.system = "x86_64-linux";
        crossSystem.config = "aarch64-unknown-linux-gnu";
        inherit overlays;
      };
    in
    {
      overlays.default = import ./nix/overlays { wamrSrc = wamr-src; };

      legacyPackages = forAllSystems pkgsFor;

      packages = forAllSystems (system:
        let pkgs = pkgsFor system; in
        {
          default = pkgs.loomos-host;
          inherit (pkgs)
            loomos-host loomos-wamr loomos-wamrc loomos-components;
        } // nixpkgs.lib.optionalAttrs (system == "x86_64-linux") {
          # 交叉构建的 aarch64 Native Host（板端只收预构建闭包，不在设备上构建——ADR-012 §决定 7）
          loomos-host-aarch64 = pkgsCrossAarch64FromX86.loomos-host;
        });

      checks = forAllSystems (system:
        let pkgs = pkgsFor system; in
        {
          inherit (pkgs)
            loomos-host loomos-wamr loomos-wamrc loomos-components;
          cargo-fmt = pkgs.callPackage ./nix/packages/check-fmt.nix { };
          toolchain-version-check =
            pkgs.callPackage ./nix/packages/check-toolchain-version.nix { };
        });

      devShells = forAllSystems (system:
        let pkgs = pkgsFor system; in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              loomosRustDev
              cmake
              python3
              pkg-config
              git
            ];
            shellHook = ''
              echo "loomos dev shell: rust=$(rustc --version)"
              echo "wamr-link 开发档：IWASM_LIB_DIR 指向 nix build .#loomos-wamr 产物的 lib/"
            '';
          };
        });

      # 开发者常用命令（等价本地脚本，但使用钉死工具链）
      apps = forAllSystems (system:
        let pkgs = pkgsFor system; in
        {
          test = {
            type = "app";
            program = pkgs.writeShellScript "loomos-test" ''
              export PATH="${pkgs.loomosRustDev}/bin:$PATH"
              cargo test --workspace
            '';
          };
          fmt = {
            type = "app";
            program = pkgs.writeShellScript "loomos-fmt" ''
              export PATH="${pkgs.loomosRustDev}/bin:$PATH"
              cargo fmt --all
            '';
          };
          clippy = {
            type = "app";
            program = pkgs.writeShellScript "loomos-clippy" ''
              export PATH="${pkgs.loomosRustDev}/bin:$PATH"
              cargo clippy --workspace --all-targets -- -D warnings
            '';
          };
          smoke = {
            type = "app";
            program = pkgs.writeShellScript "loomos-smoke" ''
              export PATH="${pkgs.loomosRustDev}/bin:$PATH"
              exec bash ${self}/agent/run-prototype.sh "$@"
            '';
          };
        });

      nixosModules.default = {
        imports = [ ./nix/modules/base.nix ./nix/modules/loomos.nix ];
      };

      nixosConfigurations = {
        generic-x86_64 = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [{
            nixpkgs.overlays = overlays;
          }] ++ [ self.nixosModules.default ./nix/systems/x86_64-linux.nix ];
        };
        generic-aarch64 = nixpkgs.lib.nixosSystem {
          system = "aarch64-linux";
          modules = [{
            nixpkgs.overlays = overlays;
          }] ++ [ self.nixosModules.default ./nix/systems/aarch64-linux.nix ];
        };
        # FMQL30TAI676H 过渡期配置（厂商 BSP 工件 ExternallyProvisioned，见配置内注释）
        fmql30tai676h = nixpkgs.lib.nixosSystem {
          system = "aarch64-linux";
          modules = [{
            nixpkgs.overlays = overlays;
          }] ++ [ self.nixosModules.default ./nix/systems/fmql30tai676h.nix ];
        };
      };
    };
}

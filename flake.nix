{
  description = "skill-pool — self-hosted Claude Code skill/agent/command registry for teams";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        commonBuildInputs = with pkgs; [ openssl pkg-config ];

        skill-pool-server = pkgs.rustPlatform.buildRustPackage {
          pname = "skill-pool-server";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--bin" "skill-pool-server" ];
          nativeBuildInputs = commonBuildInputs;
          buildInputs = commonBuildInputs;
          # sqlx offline metadata not yet committed; queries are runtime-checked.
          SQLX_OFFLINE = "true";
          doCheck = false;
        };

        skill-pool-cli = pkgs.rustPlatform.buildRustPackage {
          pname = "skill-pool-cli";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--bin" "skill-pool" ];
          nativeBuildInputs = commonBuildInputs;
          buildInputs = commonBuildInputs;
          doCheck = false;
        };
      in {
        packages = {
          inherit skill-pool-server skill-pool-cli;
          default = skill-pool-cli;
        };

        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Phase 0 — shell-only validation
            bash shellcheck shfmt jq yq-go

            # Rust toolchain
            rustc cargo rustfmt clippy rust-analyzer
            pkg-config openssl

            # Database + storage
            postgresql_17 sqlx-cli minio
            caddy

            # General tooling
            git direnv just
          ];

          shellHook = ''
            echo "skill-pool dev shell"
            echo "  cargo check --workspace"
            echo "  docker compose -f server/compose.yaml up    # local stack"
            echo "  scripts/install.sh --help                    # Phase 0 installer"
          '';
        };
      });
}

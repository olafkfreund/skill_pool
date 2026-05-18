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
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Phase 0 — shell-only validation
            bash
            shellcheck
            shfmt
            jq
            yq-go

            # Phase 1 staging — Rust toolchain
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            openssl

            # Database (Phase 1)
            postgresql_17

            # General tooling
            git
            direnv
            just
          ];

          shellHook = ''
            echo "skill-pool dev shell"
            echo "  Phase 0 — run: scripts/install.sh --help"
          '';
        };
      });
}

# Project-side NixOS / Home Manager example.
#
# Pin your `.skill-pool/manifest.toml` from your flake instead of
# committing the generated TOML. See `docs/nixos-integration.md` for
# the full reference.
#
# This file shows both flavours: a per-host NixOS module wiring and a
# per-user Home Manager module wiring. Pick whichever matches how you
# manage your dev box.

{
  description = "dev box with declaratively-pinned skill-pool manifest";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    skill-pool.url = "github:olafkfreund/skill_pool";
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, skill-pool, home-manager, ... }:
    let
      system = "x86_64-linux";
    in
    {
      # -------------------------------------------------------------
      # Option A — NixOS module (system-scope)
      #
      # Renders to /etc/skill-pool/manifest.toml. Symlink into your
      # repo with `ln -s /etc/skill-pool/manifest.toml .skill-pool/manifest.toml`.
      # -------------------------------------------------------------
      nixosConfigurations.dev-box = nixpkgs.lib.nixosSystem {
        inherit system;
        modules = [
          skill-pool.nixosModules.skill-pool
          ({ ... }: {
            services.skill-pool = {
              enable = true;
              package = skill-pool.packages.${system}.skill-pool-cli;
              projectManifest = {
                project.stack = [ "rust" "axum" "postgres" ];
                skills = [
                  { slug = "rust-axum-handler"; version = "^1.2"; scope = "project"; }
                  { slug = "sqlx-migrations"; }
                  { slug = "github-actions-cookbook"; }
                ];
                agents = [
                  { slug = "sqlx-migration-reviewer"; }
                ];
              };
            };
          })
        ];
      };

      # -------------------------------------------------------------
      # Option B — Home Manager module (per-user)
      #
      # Renders to ~/.skill-pool/manifest.toml. Most users want this
      # — the manifest is per-developer config, not per-host.
      # -------------------------------------------------------------
      homeConfigurations."alice@dev-box" = home-manager.lib.homeManagerConfiguration {
        pkgs = import nixpkgs { inherit system; };
        modules = [
          skill-pool.homeManagerModules.skill-pool
          ({ pkgs, ... }: {
            home.username = "alice";
            home.homeDirectory = "/home/alice";
            home.stateVersion = "25.05";

            services.skill-pool = {
              enable = true;
              package = skill-pool.packages.${system}.skill-pool-cli;
              # Default: ~/.skill-pool/manifest.toml. Override to put it
              # somewhere project-specific if you keep multiple repos in
              # ~/src/ — but most users keep the default and symlink
              # from individual repos.
              manifestPath = ".skill-pool/manifest.toml";
              projectManifest = {
                project.stack = [ "rust" "axum" ];
                skills = [
                  { slug = "rust-axum-handler"; version = "^1.2"; }
                  { slug = "sqlx-migrations"; }
                ];
              };
            };
          })
        ];
      };
    };
}

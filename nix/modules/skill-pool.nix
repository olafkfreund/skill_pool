# skill-pool — project-side declarative manifest module.
#
# This is the *project-side* counterpart to:
#   - `nixosModules.skill-pool-server`   — the production deploy module.
#   - `nixosModules.skill-pool-capturer` — the per-user capture daemon.
#
# Where those run a service, this module *renders* a developer's
# `.skill-pool/manifest.toml` from a typed Nix expression and installs
# the `skill-pool` CLI alongside it.
#
# Dual-purpose: works under both `nixosModules.skill-pool`
# (system-scope; renders into `/etc/<manifestPath>` and adds the CLI to
# `environment.systemPackages`) and `homeManagerModules.skill-pool`
# (per-user; renders into `~/<manifestPath>` and adds the CLI to
# `home.packages`). The module detects which evaluator is loading it
# by looking at the option tree.
#
# Example (NixOS):
#
#   imports = [ inputs.skill-pool.nixosModules.skill-pool ];
#
#   services.skill-pool = {
#     enable  = true;
#     package = inputs.skill-pool.packages.${system}.skill-pool-cli;
#     projectManifest = {
#       project.stack = [ "rust" "axum" "postgres" ];
#       skills = [
#         { slug = "rust-axum-handler"; version = "^1.2"; scope = "project"; }
#         { slug = "sqlx-migrations"; }
#       ];
#       agents = [
#         { slug = "sqlx-migration-reviewer"; }
#       ];
#     };
#   };
#
# Example (Home Manager):
#
#   imports = [ inputs.skill-pool.homeManagerModules.skill-pool ];
#
#   services.skill-pool = {
#     enable  = true;
#     package = inputs.skill-pool.packages.${pkgs.system}.skill-pool-cli;
#     manifestPath = ".skill-pool/manifest.toml";  # written under $HOME
#     projectManifest = { ... };                   # same shape
#   };
#
# See docs/nixos-integration.md for the full reference.

{ config, options, lib, pkgs, ... }:

let
  cfg = config.services.skill-pool;
  tomlFormat = pkgs.formats.toml { };
  rendered = tomlFormat.generate "skill-pool-manifest.toml" cfg.projectManifest;

  # `home.file` is a declared option only when the Home Manager module
  # system is loading this file. Look at the *options* tree (not config)
  # so the discriminator works before any definitions are checked —
  # otherwise both branches' definitions would be evaluated together
  # and one would always hit an unknown-option error in the other host.
  isHomeManager = (options ? home) && (options.home ? file);
in
{
  options.services.skill-pool = {
    enable = lib.mkEnableOption ''
      declarative skill-pool project manifest. Renders `projectManifest`
      to TOML at `manifestPath` and (when `package` is set) installs the
      skill-pool CLI so `ensure`, `bootstrap`, and friends Just Work in
      the developer's shell.
    '';

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      example = lib.literalExpression
        ''inputs.skill-pool.packages.''${system}.skill-pool-cli'';
      description = ''
        The skill-pool CLI package. Must expose a `skill-pool` binary on
        its bin path. When `null`, the manifest is rendered but no CLI
        is installed — useful when the user manages the CLI through
        another channel (e.g. their own overlay or shell devShell).
      '';
    };

    projectManifest = lib.mkOption {
      type = lib.types.attrsOf lib.types.anything;
      default = { };
      description = ''
        Contents of the rendered `manifest.toml`. The shape mirrors
        `docs/manifest-schema.md`:

        ```nix
        {
          project.stack = [ "rust" "axum" ];
          skills = [
            { slug = "rust-axum-handler"; version = "^1.2"; scope = "project"; }
          ];
          agents   = [ { slug = "sqlx-migration-reviewer"; } ];
          commands = [ ];
        }
        ```

        Freeform `attrsOf anything` so future manifest fields don't
        require module bumps. The TOML serialiser
        (`pkgs.formats.toml`) rejects shapes it can't encode, which is
        the right validation layer for a freeform manifest.
      '';
      example = lib.literalExpression ''
        {
          project.stack = [ "rust" "axum" "postgres" ];
          skills = [
            { slug = "rust-axum-handler"; version = "^1.2"; }
            { slug = "sqlx-migrations"; }
          ];
        }
      '';
    };

    manifestPath = lib.mkOption {
      type = lib.types.str;
      default = ".skill-pool/manifest.toml";
      description = ''
        Path where the rendered TOML is written.

        - **Home Manager:** relative to `$HOME`. Default lands at
          `~/.skill-pool/manifest.toml`. Symlink it into your project
          root, or override this to a project-specific absolute path
          using `home.file`'s own semantics.
        - **NixOS:** written under `/etc/<manifestPath>` (the leading
          `etc/` is stripped if present). Symlink it from your repo with
          `ln -s /etc/skill-pool/manifest.toml .skill-pool/manifest.toml`.

        Most users want the Home Manager flavour — this manifest is
        per-developer config, not per-host. See `docs/nixos-integration.md`.
      '';
      example = ".skill-pool/manifest.toml";
    };
  };

  # Host-discriminator gating uses `optionalAttrs` (a *static* Nix
  # condition, evaluated at module-parse time from the options tree).
  # That keeps the foreign branch's top-level keys (`home`,
  # `environment`) out of the result entirely, instead of merely
  # deferred — `mkIf` would leave the key visible and trip
  # "option `home` does not exist" on NixOS (and vice versa).
  # The `enable` gate stays config-time inside each branch via `mkIf`.
  config = lib.mkMerge [
    (lib.optionalAttrs isHomeManager (lib.mkIf cfg.enable {
      home.file.${cfg.manifestPath} = {
        source = rendered;
      };
      home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];
    }))
    (lib.optionalAttrs (!isHomeManager) (lib.mkIf cfg.enable {
      environment.systemPackages =
        lib.mkIf (cfg.package != null) [ cfg.package ];
      environment.etc.${
        if lib.hasPrefix "etc/" cfg.manifestPath
        then lib.removePrefix "etc/" cfg.manifestPath
        else cfg.manifestPath
      } = {
        source = rendered;
        mode = "0644";
      };
    }))
  ];
}

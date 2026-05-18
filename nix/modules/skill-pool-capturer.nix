# skill-pool capturer — declarative user systemd unit.
#
# Wires the Phase 4.6 LLM capturer pipeline as a per-user systemd timer
# without copying files into ~/.config/systemd/user/ by hand.
#
# Usage in a NixOS flake:
#
#   inputs.skill-pool.url = "github:org/skill_pool";
#   ...
#   imports = [ inputs.skill-pool.nixosModules.default ];
#   home-manager.users.alice = {
#     services.skill-pool-capturer = {
#       enable = true;
#       # API key sourced from agenix / sops / pass-through env file.
#       environmentFile = "/run/user/1000/skill-pool-capturer.env";
#     };
#   };
#
# Two paths are supported here on purpose:
#
#  - `users.users.<name>.systemd.user.{services,timers}` — when this
#    module is imported into a NixOS configuration directly.
#  - Compatible with home-manager (same option names live there).
#
# The capturer is purely client-side and per-developer, so a user-scope
# unit is the right granularity. There is no system-scope counterpart.

{ config, lib, ... }:

with lib;

let
  cfg = config.services.skill-pool-capturer;
in {
  options.services.skill-pool-capturer = {
    enable = mkEnableOption "skill-pool capturer (Phase 4.6 LLM draft generator)";

    package = mkOption {
      type = types.package;
      defaultText = literalExpression "skill-pool-cli";
      description = ''
        The skill-pool CLI package. Must expose a `skill-pool` binary
        on its bin path.
      '';
    };

    limit = mkOption {
      type = types.ints.positive;
      default = 5;
      description = ''
        Maximum sessions processed per timer firing. Soft cost cap —
        keeps a backlog from running away on the Anthropic bill.
      '';
    };

    onCalendar = mkOption {
      type = types.str;
      default = "hourly";
      example = "*-*-* 09,13,17:00:00";
      description = ''
        systemd.time(7) calendar spec for the firing schedule. Default
        is hourly with jitter (see `randomizedDelaySec`).
      '';
    };

    randomizedDelaySec = mkOption {
      type = types.str;
      default = "10min";
      description = ''
        Random delay applied on top of `onCalendar` to spread load if
        many machines share an Anthropic org.
      '';
    };

    memoryMax = mkOption {
      type = types.str;
      default = "512M";
      description = ''
        Soft memory cap for the capturer. LLM responses + a few hundred
        KB of transcript per session shouldn't approach this, but it
        bounds runaway cases.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Path to a file containing `KEY=VALUE` env vars for the unit.
        Most useful for `ANTHROPIC_API_KEY` — keep it out of the world-
        readable Nix store via agenix / sops-nix / a pass-through file
        under /run.
      '';
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = {};
      example = literalExpression ''
        {
          SKILL_POOL_REGISTRY = "https://acme.skill-pool.example.com";
        }
      '';
      description = ''
        Additional environment for the unit. Use `environmentFile`
        instead for secrets.
      '';
    };
  };

  config = mkIf cfg.enable {
    systemd.user.services.skill-pool-capturer = {
      description = "skill-pool capturer (Phase 4.6 LLM draft generator)";
      after = [ "network-online.target" ];
      wantedBy = [ "default.target" ];

      environment = cfg.extraEnvironment;

      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/skill-pool capture-run --limit ${toString cfg.limit}";
        MemoryMax = cfg.memoryMax;
        TimeoutStartSec = "300";
      } // (lib.optionalAttrs (cfg.environmentFile != null) {
        EnvironmentFile = cfg.environmentFile;
      });
    };

    systemd.user.timers.skill-pool-capturer = {
      description = "skill-pool capturer schedule";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.onCalendar;
        Persistent = true;
        RandomizedDelaySec = cfg.randomizedDelaySec;
        Unit = "skill-pool-capturer.service";
      };
    };
  };
}

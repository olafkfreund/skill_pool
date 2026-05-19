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

    daemon = mkEnableOption ''
      Run the capturer as a long-lived daemon (`skill-pool-capturer`)
      instead of the hourly timer-driven single-shot. The daemon polls
      ~/.skill-pool/queue and ~/.skill-pool/sessions every
      `pollSecs` seconds and drafts new candidates within one cycle.

      The two modes are mutually exclusive: when `daemon = true`, the
      timer is suppressed and a `Type=simple` user service is emitted
      instead. When `daemon = false` (default), behaviour matches the
      previous releases — hourly oneshot timer
    '';

    package = mkOption {
      type = types.package;
      defaultText = literalExpression "skill-pool-cli";
      description = ''
        The skill-pool CLI package. Must expose a `skill-pool` binary
        on its bin path (and, when `daemon = true`, also a
        `skill-pool-capturer` binary).
      '';
    };

    pollSecs = mkOption {
      type = types.ints.positive;
      default = 30;
      description = ''
        Daemon-mode poll interval. Ignored when `daemon = false`.
      '';
    };

    noNotify = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Suppress the per-draft desktop notification. Useful when the
        unit runs in a context without `DBUS_SESSION_BUS_ADDRESS` (e.g.
        a headless dev box) so the daemon doesn't try and log a libdbus
        error. Equivalent to setting `SKILL_POOL_CAPTURE_NO_NOTIFY=1`.
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

  config = mkIf cfg.enable (lib.mkMerge [
    # ---- Shared environment between the two modes ----------------------
    {
      # Make sure the binary the user picked is on PATH in the unit env.
      # Both modes need it; the unit's ExecStart resolves via the package
      # path, but downstream `systemctl --user status` is friendlier when
      # the bin is also discoverable interactively. Harmless extra cost.
    }

    # ---- Mode A: hourly timer-driven single-shot (default) -------------
    (lib.mkIf (!cfg.daemon) {
      systemd.user.services.skill-pool-capturer = {
        description = "skill-pool capturer (Phase 4.6 LLM draft generator)";
        after = [ "network-online.target" ];
        wantedBy = [ "default.target" ];

        environment = cfg.extraEnvironment
          // (lib.optionalAttrs cfg.noNotify {
            SKILL_POOL_CAPTURE_NO_NOTIFY = "1";
          });

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
    })

    # ---- Mode B: long-lived daemon (opt-in via daemon = true) ----------
    (lib.mkIf cfg.daemon {
      systemd.user.services.skill-pool-capturer-daemon = {
        description = "skill-pool capturer daemon (Phase 4.6, long-lived)";
        after = [ "network-online.target" ];
        wantedBy = [ "default.target" ];
        # Belt-and-braces: if a user flips between modes mid-cycle, the
        # systemd unit conflict prevents both running at once. The
        # `daemon` switch already suppresses the timer at evaluation
        # time; this is the runtime safety net.
        conflicts = [ "skill-pool-capturer.timer" "skill-pool-capturer.service" ];

        environment = cfg.extraEnvironment
          // {
            SKILL_POOL_CAPTURER_POLL_SECS = toString cfg.pollSecs;
          }
          // (lib.optionalAttrs cfg.noNotify {
            SKILL_POOL_CAPTURE_NO_NOTIFY = "1";
          });

        serviceConfig = {
          Type = "simple";
          ExecStart = "${cfg.package}/bin/skill-pool-capturer";
          Restart = "on-failure";
          RestartSec = "10s";
          MemoryMax = cfg.memoryMax;
        } // (lib.optionalAttrs (cfg.environmentFile != null) {
          EnvironmentFile = cfg.environmentFile;
        });
      };
    })
  ]);
}

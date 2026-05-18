# skill-pool-server — declarative NixOS module.
#
# Wraps the systemd unit shipped in packaging/systemd/ with NixOS option
# typing, so a deployment looks like:
#
#   inputs.skill-pool.url = "github:olafkfreund/skill_pool";
#   ...
#   imports = [ inputs.skill-pool.nixosModules.skill-pool-server ];
#
#   services.skill-pool-server = {
#     enable = true;
#     bind = "127.0.0.1:8080";
#     databaseUrl = "postgres://skillpool@localhost/skillpool";
#     storageUri  = "fs:///var/lib/skill-pool/bundles";
#     defaultTenant = "acme";
#     environmentFile = "/run/keys/skill-pool.env";   # OIDC secrets, SMTP creds, etc.
#     openFirewall = false;                            # reverse-proxy in front
#   };
#
# The module:
#   - creates `skillpool` system user + group
#   - creates /var/lib/skill-pool with correct ownership
#   - runs sqlx migrations on every start (idempotent)
#   - installs a hardened systemd unit (NoNewPrivileges, ProtectSystem, etc.)
#
# Secrets must come from `environmentFile`. Inline-passing `databaseUrl`
# is convenient for dev only — anything credential-bearing goes via
# the env file and agenix/sops/etc.

{ config, lib, pkgs, ... }:

let
  cfg = config.services.skill-pool-server;
in
{
  options.services.skill-pool-server = {
    enable = lib.mkEnableOption "skill-pool registry HTTP server";

    package = lib.mkOption {
      type = lib.types.package;
      description = ''
        The skill-pool-server package. Must expose `bin/skill-pool-server`.
        Typically `inputs.skill-pool.packages.''${system}.skill-pool-server`.
      '';
    };

    bind = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1:8080";
      example = "0.0.0.0:8080";
      description = ''
        HTTP bind address. Use 127.0.0.1 when fronted by a reverse proxy
        on the same host; 0.0.0.0 when exposing the server directly.
      '';
    };

    databaseUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "postgres://skillpool@localhost/skillpool";
      description = ''
        Postgres DSN. Convenient for dev; for production set this via
        `environmentFile` (SKILL_POOL_DATABASE_URL=…) so the password
        isn't in the world-readable Nix store.
      '';
    };

    databaseReadUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "postgres://skillpool@replica.internal/skillpool";
      description = ''
        Optional read-replica DSN. When set, read-only handlers route
        to this pool; writes still go to `databaseUrl`. Like the primary
        DSN, prefer setting this via `environmentFile` in production.
      '';
    };

    dbPoolSize = lib.mkOption {
      type = lib.types.ints.positive;
      default = 20;
      description = ''
        sqlx connection-pool max size. The read pool (if configured)
        uses the same cap. Rough rule: (peak RPS × p95-seconds) + 20%.
      '';
    };

    storageUri = lib.mkOption {
      type = lib.types.str;
      default = "fs:///var/lib/skill-pool/bundles";
      example = "s3://my-bucket?region=us-east-1";
      description = ''
        Bundle storage backend URI. `fs://<absolute-path>` for local;
        `s3://...` with reqsign-compatible query params for object stores.
      '';
    };

    defaultTenant = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "acme";
      description = ''
        Fallback tenant slug when the request Host header carries no
        recognisable subdomain. Leave null in multi-tenant production
        with proper wildcard DNS.
      '';
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "info,skill_pool=info";
      example = "debug,skill_pool=trace";
      description = ''
        Value for the `RUST_LOG` environment variable. See the
        `tracing_subscriber::EnvFilter` syntax.
      '';
    };

    logFormat = lib.mkOption {
      type = lib.types.enum [ "json" "pretty" ];
      default = "json";
      description = ''
        Output format for tracing. `json` is line-delimited and is what
        Loki/Splunk/CloudWatch expect; `pretty` is human-readable for
        local debugging only.
      '';
    };

    otlpEndpoint = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "http://otel-collector.internal:4318";
      description = ''
        OTLP collector URL. Only meaningful if `package` was built with
        the `otlp` Cargo feature. Setting this here populates
        `OTEL_EXPORTER_OTLP_ENDPOINT`.
      '';
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = "/run/keys/skill-pool.env";
      description = ''
        Path to a file containing `KEY=value` pairs loaded by systemd
        with `EnvironmentFile=`. The right place for any secret-bearing
        variable: `SKILL_POOL_DATABASE_URL`, OIDC client secrets, SMTP
        credentials, etc. Use agenix / sops-nix to populate it.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "skillpool";
      description = "System user that runs the registry server.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "skillpool";
      description = "System group for the registry server.";
    };

    stateDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/skill-pool";
      description = ''
        Writable state directory. Bundle storage when `storageUri` is
        `fs:///var/lib/skill-pool/...` also lives here.
      '';
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Open the bind port in the system firewall. Leave false when a
        reverse proxy on the same host is fronting the server.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      group = cfg.group;
      home = cfg.stateDir;
      description = "skill-pool registry server";
    };
    users.groups.${cfg.group} = { };

    systemd.tmpfiles.rules = [
      "d ${cfg.stateDir} 0750 ${cfg.user} ${cfg.group} -"
    ];

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [
      (lib.toInt (lib.elemAt (lib.splitString ":" cfg.bind) 1))
    ];

    systemd.services.skill-pool-server = {
      description = "skill-pool registry server";
      documentation = [ "https://github.com/olafkfreund/skill_pool" ];
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" "postgresql.service" ];
      wants = [ "network-online.target" ];

      environment = lib.filterAttrs (_: v: v != null) {
        SKILL_POOL_BIND = cfg.bind;
        SKILL_POOL_STORAGE_URI = cfg.storageUri;
        SKILL_POOL_DATABASE_URL = cfg.databaseUrl;
        SKILL_POOL_DATABASE_READ_URL = cfg.databaseReadUrl;
        SKILL_POOL_DB_POOL_SIZE = toString cfg.dbPoolSize;
        SKILL_POOL_DEFAULT_TENANT = cfg.defaultTenant;
        RUST_LOG = cfg.logLevel;
        RUST_LOG_FORMAT = cfg.logFormat;
        OTEL_EXPORTER_OTLP_ENDPOINT = cfg.otlpEndpoint;
      };

      serviceConfig = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/skill-pool-server serve";
        User = cfg.user;
        Group = cfg.group;
        WorkingDirectory = cfg.stateDir;

        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;

        KillSignal = "SIGTERM";
        TimeoutStopSec = "30s";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening — mirrors packaging/systemd/skill-pool-server.service.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.stateDir ];
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectProc = "invisible";
        ProcSubset = "pid";
        RestrictSUIDSGID = true;
        RestrictRealtime = true;
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged @resources" ];
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        LimitNOFILE = 65536;
        TasksMax = 1024;
        MemoryMax = "2G";
      };
    };
  };
}

# NixOS deploy

Declarative single-binary deployment. The flake exposes a
`nixosModules.skill-pool-server` module that wires the systemd unit and
state directory with type-checked options.

## Flake input

```nix
{
  inputs.skill-pool.url = "github:olafkfreund/skill_pool";
  outputs = { self, nixpkgs, skill-pool, ... }: {
    nixosConfigurations.registry = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        skill-pool.nixosModules.skill-pool-server
        ./registry-config.nix
      ];
    };
  };
}
```

## Minimal configuration

```nix
# registry-config.nix
{ pkgs, skill-pool, ... }:
{
  services.skill-pool-server = {
    enable = true;
    package = skill-pool.packages.${pkgs.system}.skill-pool-server;

    bind = "127.0.0.1:8080";                # behind a reverse proxy
    storageUri = "fs:///var/lib/skill-pool/bundles";
    defaultTenant = "acme";

    # Secrets — everything credential-bearing comes from here.
    environmentFile = "/run/keys/skill-pool.env";
  };

  services.postgresql = {
    enable = true;
    package = pkgs.postgresql_17;
    ensureDatabases = [ "skillpool" ];
    ensureUsers = [{
      name = "skillpool";
      ensureDBOwnership = true;
    }];
    extraPlugins = ps: [ ps.pgvector ];
  };

  # Caddy fronting the server + web.
  services.caddy = {
    enable = true;
    virtualHosts."skill-pool.example.com".extraConfig = ''
      reverse_proxy 127.0.0.1:3000
    '';
    virtualHosts."*.skill-pool.example.com".extraConfig = ''
      @api path /v1/* /metrics
      reverse_proxy @api 127.0.0.1:8080
      reverse_proxy 127.0.0.1:3000
    '';
  };
}
```

## Secrets with `agenix`

```nix
{
  age.secrets."skill-pool.env" = {
    file = ./secrets/skill-pool.env.age;
    owner = config.services.skill-pool-server.user;
    group = config.services.skill-pool-server.group;
    mode = "0400";
  };

  services.skill-pool-server.environmentFile =
    config.age.secrets."skill-pool.env".path;
}
```

Decrypted file contents:

```
SKILL_POOL_DATABASE_URL=postgres://skillpool:CHANGE@localhost/skillpool
# Add OIDC / SAML / SMTP credentials here as needed.
```

## All options

| Option                | Type            | Default                              | Purpose                          |
|-----------------------|-----------------|--------------------------------------|----------------------------------|
| `enable`              | bool            | `false`                              | Master switch                    |
| `package`             | package         | —                                    | Server binary                    |
| `bind`                | string          | `"127.0.0.1:8080"`                   | HTTP bind                        |
| `databaseUrl`         | nullable string | `null`                               | Postgres DSN (prefer env file)   |
| `storageUri`          | string          | `"fs:///var/lib/skill-pool/bundles"` | Backend URI                      |
| `defaultTenant`       | nullable string | `null`                               | Host-fallback tenant slug        |
| `logLevel`            | string          | `"info,skill_pool=info"`             | `RUST_LOG` value                 |
| `logFormat`           | enum            | `"json"`                             | `json` or `pretty`               |
| `otlpEndpoint`        | nullable string | `null`                               | OTLP collector URL               |
| `environmentFile`     | nullable path   | `null`                               | EnvironmentFile= for secrets     |
| `user` / `group`      | string          | `"skillpool"`                        | Service user/group               |
| `stateDir`            | path            | `/var/lib/skill-pool`                | Writable dir                     |
| `openFirewall`        | bool            | `false`                              | Open `bind` port in firewall     |

## Rebuild + verify

```bash
sudo nixos-rebuild switch --flake .#registry
systemctl status skill-pool-server
journalctl -u skill-pool-server -f
curl -s http://127.0.0.1:8080/v1/healthz | jq
```

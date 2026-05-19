# NixOS / Home Manager integration

skill-pool ships three flake modules. Pick the one that matches the
problem you're solving.

| Module                                 | Scope         | When to reach for it                                                                                        |
|----------------------------------------|---------------|-------------------------------------------------------------------------------------------------------------|
| `nixosModules.skill-pool-server`       | system, prod  | You're deploying the registry server itself onto NixOS.                                                     |
| `nixosModules.skill-pool-capturer`     | per-user      | You want the Phase 4.6 LLM capturer daemon turned on for a developer account.                               |
| `nixosModules.skill-pool` *(new)*      | system / user | You'd rather **pin your project manifest declaratively** than commit a generated `.skill-pool/manifest.toml`. |

The third module is what this doc covers in depth. The first two have
their own references — links below.

## What `nixosModules.skill-pool` gives you

A typed Nix expression that **renders your `.skill-pool/manifest.toml`
contents** and (optionally) installs the `skill-pool` CLI. The shape
mirrors `docs/manifest-schema.md`:

```nix
services.skill-pool = {
  enable = true;
  package = inputs.skill-pool.packages.${system}.skill-pool-cli;
  projectManifest = {
    project.stack = [ "rust" "axum" "postgres" ];
    skills = [
      { slug = "rust-axum-handler"; version = "^1.2"; scope = "project"; }
      { slug = "sqlx-migrations"; }
    ];
    agents   = [ { slug = "sqlx-migration-reviewer"; } ];
    commands = [ ];
  };
};
```

That gets serialised by `pkgs.formats.toml` and placed where
`skill-pool ensure` will find it (see `manifestPath`).

The module is **dual-purpose**: import it from `nixosModules.skill-pool`
under a NixOS configuration, or from `homeManagerModules.skill-pool`
under a Home Manager configuration. Detection is automatic — it adds
the right `home.file` / `environment.etc` definitions for whichever host
loaded it.

## Adding the flake input

```nix
{
  inputs.skill-pool.url = "github:olafkfreund/skill_pool";
  inputs.nixpkgs.url    = "github:NixOS/nixpkgs/nixos-unstable";
}
```

`skill-pool`'s own flake does not depend on a specific nixpkgs revision
for the modules (only the prebuilt packages need a pinned nixpkgs). You
can safely follow your own pin:

```nix
inputs.skill-pool = {
  url = "github:olafkfreund/skill_pool";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

## Project-side recipe

Full copy-pasteable example: [`docs/examples/nixos-project.nix`](examples/nixos-project.nix).

### NixOS (system-scope)

```nix
{ inputs, system, ... }: {
  imports = [ inputs.skill-pool.nixosModules.skill-pool ];

  services.skill-pool = {
    enable = true;
    package = inputs.skill-pool.packages.${system}.skill-pool-cli;
    projectManifest = {
      project.stack = [ "rust" "axum" ];
      skills = [
        { slug = "rust-axum-handler"; version = "^1.2"; }
        { slug = "sqlx-migrations"; }
      ];
    };
  };
}
```

This writes the rendered TOML to `/etc/skill-pool/manifest.toml`. From
each repo you want to use it:

```bash
ln -s /etc/skill-pool/manifest.toml .skill-pool/manifest.toml
skill-pool ensure
```

### Home Manager (per-user)

```nix
{ pkgs, inputs, ... }: {
  imports = [ inputs.skill-pool.homeManagerModules.skill-pool ];

  services.skill-pool = {
    enable = true;
    package = inputs.skill-pool.packages.${pkgs.system}.skill-pool-cli;
    projectManifest = {
      project.stack = [ "rust" "axum" ];
      skills = [
        { slug = "rust-axum-handler"; version = "^1.2"; }
        { slug = "sqlx-migrations"; }
      ];
    };
  };
}
```

This writes `~/.skill-pool/manifest.toml`. Symlink it into project
roots the same way (`ln -s ~/.skill-pool/manifest.toml
.skill-pool/manifest.toml` inside the repo).

## Server-side recipe

If you're deploying the registry, see [`docs/deploy/nixos.md`](deploy/nixos.md)
— that module is `nixosModules.skill-pool-server`, completely separate
from this one.

## Capturer recipe

Per-user systemd timer that periodically runs the Phase 4.6 LLM
capturer. Lives in `nix/modules/skill-pool-capturer.nix`. Wire it
under either `nixosModules.skill-pool-capturer` or
`homeManagerModules.skill-pool-capturer`:

```nix
services.skill-pool-capturer = {
  enable = true;
  package = inputs.skill-pool.packages.${pkgs.system}.skill-pool-cli;
  # API key from agenix/sops/etc. — never in the Nix store.
  environmentFile = "/run/user/1000/skill-pool-capturer.env";
  limit = 5;
  onCalendar = "hourly";
};
```

See `docs/capture.md` for what the pipeline does and the cost model.

## Option reference — `services.skill-pool`

| Option            | Type                       | Default                          | Description                                                                                                              |
|-------------------|----------------------------|----------------------------------|--------------------------------------------------------------------------------------------------------------------------|
| `enable`          | bool                       | `false`                          | Master switch.                                                                                                           |
| `package`         | nullable package           | `null`                           | The skill-pool CLI package (`bin/skill-pool` on its path). When `null` the manifest is rendered but no CLI is installed. |
| `projectManifest` | freeform `attrsOf anything`| `{ }`                            | Manifest contents — see `docs/manifest-schema.md`. Serialised to TOML via `pkgs.formats.toml`.                           |
| `manifestPath`    | string                     | `".skill-pool/manifest.toml"`    | Where the rendered TOML is written. HM: relative to `$HOME`. NixOS: relative to `/etc/`.                                 |

### `projectManifest` shape

The freeform attrs translate 1-to-1 to the canonical
`.skill-pool/manifest.toml`:

- `project.stack` — list of strings; the fingerprint tags
  (`skill-pool detect` populates these in dynamic mode).
- `skills` — list of `{ slug, version, scope }` records.
  `version` defaults to `"*"` (latest). `scope` is `"project"` (under
  `./.claude/skills/`) or `"personal"` (under `~/.claude/skills/`).
- `agents` — same record shape as `skills`, written into the
  `[[agents]]` array.
- `commands` — same record shape, written into the `[[commands]]`
  array (Phase 5 slash-commands).

Unknown keys round-trip through unchanged. That keeps this module
compatible with manifest fields that ship in later phases without
requiring a module bump.

## Caveats

- **`manifestPath` is per-user, not project-checked-in.** This module
  is for users who *prefer* declarative — if you check
  `.skill-pool/manifest.toml` into the repo (the default workflow),
  direnv / `skill-pool bootstrap` / the SessionStart hook all keep
  working without this module. Pick one source of truth per project.
- **`pkgs.formats.toml` reorders keys** alphabetically. The rendered
  TOML is deterministic but the order may not match what a human would
  write by hand. `skill-pool` parses both orders, so this is cosmetic.
- **System-scope rendering means rollbacks.** If you switch from this
  module to a checked-in `manifest.toml` mid-stream, remove the
  symlink first so a `nixos-rebuild switch` rollback doesn't reinstate
  an out-of-date file.
- **Stack detection still runs by default.** This module pins the
  manifest, but `skill-pool ensure` honours `.skill-pool/detected.json`
  for stack-driven recommendations. Wipe that cache if you switch
  stacks via the manifest (or set `project.stack` here and let the CLI
  trust your declaration).

## Cross-links

- [`docs/bootstrap.md`](bootstrap.md) — the auto-bootstrap flow
  (`skill-pool bootstrap`, direnv, SessionStart hook).
- [`docs/manifest-schema.md`](manifest-schema.md) — canonical
  `.skill-pool/manifest.toml` schema (the same shape `projectManifest`
  serialises to).
- [`docs/deploy/nixos.md`](deploy/nixos.md) — server-side
  `nixosModules.skill-pool-server` reference.
- [`docs/capture.md`](capture.md) — capture / scoring / LLM drafter
  pipeline; pairs with `nixosModules.skill-pool-capturer`.
- [`docs/examples/nixos-project.nix`](examples/nixos-project.nix) —
  full working flake example.

{
  description = "skill-pool — self-hosted Claude Code skill/agent/command registry for teams";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }: {
    # NixOS / home-manager module — declarative wrapper around the systemd
    # units shipped in packaging/systemd/. Lives outside the per-system
    # wrapper because modules are evaluated by the consumer's NixOS, not
    # by `nix build`.
    nixosModules.default = ./nix/modules/skill-pool-capturer.nix;
    nixosModules.skill-pool-capturer = ./nix/modules/skill-pool-capturer.nix;
    # Server (system-scope, single-node deploys + cluster nodes).
    nixosModules.skill-pool-server = ./nix/modules/skill-pool-server.nix;
    # Project-side declarative manifest module (Phase 3 §B). Lets the
    # NixOS / Home Manager user pin their `.skill-pool/manifest.toml`
    # contents from their flake instead of committing the generated TOML.
    nixosModules.skill-pool = ./nix/modules/skill-pool.nix;
    # Home Manager users import this at the same path; same module shape.
    homeManagerModules.default = ./nix/modules/skill-pool-capturer.nix;
    homeManagerModules.skill-pool-capturer = ./nix/modules/skill-pool-capturer.nix;
    homeManagerModules.skill-pool = ./nix/modules/skill-pool.nix;
  } // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        commonBuildInputs = with pkgs; [ openssl pkg-config ];

        # Extra deps the server crate needs that the CLI doesn't.
        # samael (SAML SP) pulls libxml2 + xmlsec + libxslt at link time and
        # libclang via bindgen at build time. Without these the standalone
        # `nix build .#skill-pool-server` fails with `cannot find -lxmlsec1`
        # even though the devShell builds the same crate cleanly (because the
        # devShell carries them implicitly). Tracked: previously called out
        # in line ~96 of this file as "not yet wired into commonBuildInputs".
        serverBuildInputs = with pkgs; commonBuildInputs ++ [
          xmlsec libxml2 libxml2.dev libxslt libtool
        ];
        # samael's build.rs runs `xmlsec1-config --cflags` at build time;
        # that binary lives in the `.dev` output of nixpkgs xmlsec, not the
        # default. pkg-config files there are also needed for the `xmlsec1`
        # `.pc` discovery. The bindgenHook puts libclang on the right env
        # vars for samael's bindgen invocation.
        serverNativeBuildInputs = with pkgs; commonBuildInputs ++ [
          xmlsec.dev libxml2.dev rustPlatform.bindgenHook
        ];

        skill-pool-server = pkgs.rustPlatform.buildRustPackage {
          pname = "skill-pool-server";
          version = "0.3.3";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--bin" "skill-pool-server" ];
          nativeBuildInputs = serverNativeBuildInputs;
          buildInputs = serverBuildInputs;
          # sqlx offline metadata not yet committed; queries are runtime-checked.
          SQLX_OFFLINE = "true";
          doCheck = false;
        };

        skill-pool-cli = pkgs.rustPlatform.buildRustPackage {
          pname = "skill-pool-cli";
          version = "0.3.3";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [ "--bin" "skill-pool" ];
          nativeBuildInputs = commonBuildInputs;
          buildInputs = commonBuildInputs;
          doCheck = false;
        };

        skill-pool-web = pkgs.buildNpmPackage {
          pname = "skill-pool-web";
          version = "0.3.3";
          src = ./web;
          npmDepsHash = "sha256-9PGuX5IrU/97FENZJDn1ILdJ4eA2OwS1tPBrUzUELm8=";
          npmBuildScript = "build";
          # adapter-node emits build/ as the server bundle.  Copy everything
          # the runtime needs: the compiled output, the production node_modules
          # that adapter-node embeds, and the package.json manifest (used by
          # Node to resolve the entry point and ESM type).
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r build/* $out/
            cp package.json $out/package.json
            runHook postInstall
          '';
          meta = with pkgs.lib; {
            description = "skill-pool SvelteKit portal (adapter-node bundle)";
            license = licenses.mit;
            platforms = platforms.linux;
          };
        };
      in {
        packages = {
          inherit skill-pool-server skill-pool-cli skill-pool-web;
          default = skill-pool-cli;
        };

        # `nix run .#skill-pool-web` — boot the adapter-node bundle.
        # Set PORT / HOST / ORIGIN env vars as needed before running.
        apps.skill-pool-web = {
          type = "app";
          program = "${pkgs.writeShellScript "skill-pool-web" ''
            exec ${pkgs.nodejs_22}/bin/node ${skill-pool-web}/index.js "$@"
          ''}";
        };

        # Build-smoke check: ensures the web derivation stays buildable in CI.
        # Rust packages are not included here because the server requires
        # additional native libraries (libxml2, xmlsec) not yet wired into
        # commonBuildInputs — that is tracked separately.
        checks = {
          skill-pool-web-build = skill-pool-web;
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

            # Web (Phase 2)
            nodejs_22

            # SAML XML signature validation (samael -> xmlsec1 + libxml2 + bindgen).
            # xmlsec1 dynamically loads modules via libltdl, so libtool is required
            # at link time even though we don't call it directly.
            xmlsec libxml2 libxml2.dev libxslt libtool
            clang
            llvmPackages.libclang
            openssl.dev

            # General tooling
            git direnv just

            # Terminal recording for showcase demos (scripts/demo.tape).
            # vhs renders deterministic .gif/.webm output from a tape script
            # so the README/docs demo stays reproducible across machines.
            vhs

            # Playwright-driven portal screenshots (scripts/screenshots/).
            # Use the Nix-provided Chromium to keep the renderer pinned and
            # reproducible; PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD avoids the
            # npm post-install browser fetch.
            chromium
            # ImageMagick for assembling the hero collage from individual
            # screenshots (scripts/screenshots → docs/images/hero.webp).
            imagemagick
          ];

          shellHook = ''
            # samael's libxml binding uses bindgen → needs LIBCLANG_PATH.
            export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
            # Make sure pkg-config finds Nix-provided headers, not /usr/.
            unset OPENSSL_DIR
            # Pin Playwright to the Nix-provided Chromium so screenshot
            # capture is reproducible and doesn't try to download a
            # platform-specific browser at npm install time.
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
            export PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=${pkgs.chromium}/bin/chromium
            echo "skill-pool dev shell"
            echo "  cargo check --workspace"
            echo "  docker compose -f server/compose.yaml up    # local stack"
            echo "  scripts/install.sh --help                    # Phase 0 installer"
          '';
        };
      });
}

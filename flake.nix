{
  description = "STAR/WIRE — a stack-based terminal news reader";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # Home-manager module, so STAR/WIRE can be installed and configured
      # declaratively the way the rest of a NixOS setup is.
      hmModule = { config, lib, pkgs, ... }:
        let cfg = config.programs.starwire;
        in {
          options.programs.starwire = {
            enable = lib.mkEnableOption "STAR/WIRE terminal news reader";
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The starwire package to use.";
            };
            theme = lib.mkOption {
              type = lib.types.str;
              default = "catppuccin-mocha";
              description = ''
                A built-in theme id, or the id of a file in
                ~/.local/starwire/themes. Ignored when stylix.enable is set.
              '';
            };
            stylix.enable = lib.mkEnableOption ''
              deriving a STAR/WIRE theme from the active Stylix base16 scheme,
              so the window matches the rest of the desktop automatically
            '';
            settings = lib.mkOption {
              type = lib.types.attrs;
              default = { };
              example = { fetch.refresh_minutes = 30; reading.width = 100; };
              description = ''
                Extra config.toml settings, merged last and table by table, so
                setting one key in [fetch] leaves the rest of [fetch] alone.
              '';
            };
          };

          config = lib.mkIf cfg.enable (lib.mkMerge [
            {
              home.packages = [ cfg.package ];
              # STAR/WIRE keeps everything under one directory rather than
              # spreading it across the XDG roots, so this is not
              # xdg.configFile.
              home.file.".local/starwire/config.toml".source =
                (pkgs.formats.toml { }).generate "starwire-config.toml" (
                  lib.recursiveUpdate
                    { ui.theme = if cfg.stylix.enable then "stylix" else cfg.theme; }
                    cfg.settings
                );
            }
            (lib.mkIf cfg.stylix.enable {
              home.file.".local/starwire/themes/stylix.toml".text =
                let c = config.lib.stylix.colors;
                in ''
                  # Generated from the active Stylix scheme.
                  [meta]
                  name = "Stylix"
                  id = "stylix"
                  variant = "${config.stylix.polarity}"

                  [base16]
                '' + lib.concatMapStringsSep "\n"
                  (n: ''base${n} = "#${c."base${n}"}"'')
                  [ "00" "01" "02" "03" "04" "05" "06" "07"
                    "08" "09" "0A" "0B" "0C" "0D" "0E" "0F" ]
                  + "\n";
            })
          ]);
        };
    in
    {
      homeManagerModules.starwire = hmModule;
      homeManagerModules.default = hmModule;
      overlays.default = final: prev: {
        starwire = self.packages.${final.stdenv.hostPlatform.system}.default;
      };
    }
    # Explicit rather than eachDefaultSystem, which would also claim systems
    # nobody has built this on. aarch64-darwin only, as in STAR/AMP: nixpkgs
    # 26.11 dropped x86_64-darwin outright, and naming it fails *evaluation*
    # with a release note rather than merely failing to build.
    // flake-utils.lib.eachSystem [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # One version, read rather than repeated. scripts/check-version.sh
        # asserts the copies that cannot be derived (Cargo.lock).
        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

        # There are no buildInputs and no nativeBuildInputs, which is worth
        # saying out loud because a program that fetches over TLS and keeps a
        # SQLite file invites the guess that it needs OpenSSL and libsqlite3:
        # TLS is rustls through `ureq`, and SQLite is the amalgamation
        # `rusqlite`'s `bundled` feature compiles from source with the cc
        # crate. Nothing in the tree runs bindgen. `yt-dlp` and `mpv` are not
        # build inputs either -- they are the reader's own programs, looked up
        # on PATH at the moment they are wanted, and STAR/WIRE works without
        # them for everything but video.
        #
        # If that changes -- a dependency switching to a `-sys` crate, most
        # likely -- this is where pkg-config and the library go, and CI needs
        # the matching apt line.
        mkStarwire = { pkgsFor ? pkgs }:
          pkgsFor.rustPlatform.buildRustPackage {
            pname = "starwire";
            version = cargoToml.package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            # STAR/KIT comes from a git revision rather than from crates.io,
            # and `cargoLock.lockFile` alone cannot fetch it: nix wants a hash
            # for every source it downloads. Two ways to give it one.
            #
            # `outputHashes` is the reproducible one, and it means a new hash
            # to compute and commit on every STAR/KIT revision -- a second
            # place the version lives, which is exactly the kind of copy that
            # goes stale between the bump and the person who notices.
            #
            # This asks nix's builtin `fetchGit` for it instead. The revision
            # is immutable and `Cargo.lock` records it, so what is fetched is
            # still pinned; what is given up is the fixed-output hash, which
            # means this fetch happens outside the sandbox and a build with no
            # network cannot do it. That is the right trade here: the lockfile
            # is the pin, and a stale hash nobody bumped is a worse failure
            # than a build that needs the network it was already going to use.
            cargoLock.allowBuiltinFetchGit = true;

            # freedesktop assets, which mean nothing on macOS.
            postInstall = pkgsFor.lib.optionalString pkgsFor.stdenv.hostPlatform.isLinux ''
              install -Dm644 packaging/starwire.desktop \
                $out/share/applications/starwire.desktop
              install -Dm644 packaging/starwire.png \
                $out/share/icons/hicolor/256x256/apps/starwire.png
              install -Dm644 packaging/starwire.svg \
                $out/share/icons/hicolor/scalable/apps/starwire.svg
            '';

            meta = with pkgsFor.lib; {
              description = "A stack-based terminal news reader in the STAR family";
              homepage = "https://github.com/bstar/starwire";
              license = licenses.mit;
              mainProgram = "starwire";
              platforms = platforms.linux ++ platforms.darwin;
            };
          };
      in
      {
        packages.default = mkStarwire { };
        packages.starwire = mkStarwire { };

        # buildRustPackage runs `cargo test` as part of building the package,
        # so naming it here makes `nix flake check` cover the test suite too.
        checks = {
          inherit (self.packages.${system}) default;

          fmt = pkgs.runCommand "cargo-fmt"
            { nativeBuildInputs = [ pkgs.rustfmt ]; }
            ''
              cd ${./.}
              find src tests -name '*.rs' -print0 \
                | xargs -0 rustfmt --check --edition 2021
              touch $out
            '';
        };

        formatter = pkgs.nixpkgs-fmt;

        apps.default = flake-utils.lib.mkApp {
          drv = self.packages.${system}.default;
        };

        devShells.default = pkgs.mkShell {
          packages = (with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            # STAR/KIT is a git dependency, and cargo fetches it with the
            # git on PATH (`CARGO_NET_GIT_FETCH_WITH_CLI=true`, which is what
            # lets an `insteadOf` rewrite to ssh work). On a Mac with Xcode
            # selected, `/usr/bin/git` is a shim that asks xcrun where git
            # is, and inside this shell xcrun is nix's, which answers "tool
            # 'git' not found". Nix's own git, first on PATH, is the fix.
            git
            # scripts/check-version.sh reads `cargo metadata`.
            jq
            # The licence and advisory gate, so it is run before CI runs it.
            # `deny.toml` has one allowed git source, and the check that the
            # list still has exactly what it should is this command.
            cargo-deny
            # Not build inputs: the player and the resolver STAR/WIRE hands a
            # video to, here so that `youtube sync` and the player can be
            # tried from the devshell without installing them system-wide.
            # The tests that use yt-dlp skip when it is not on PATH.
            yt-dlp
            mpv
          ]);

          shellHook = ''
            echo "STAR/WIRE devshell · rustc $(rustc --version | cut -d' ' -f2)"
          '';
        };
      });
}

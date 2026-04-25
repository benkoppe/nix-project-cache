{ inputs, lib, ... }:
{
  perSystem =
    {
      config,
      pkgs,
      self',
      ...
    }:
    let
      craneLib = inputs.crane.mkLib pkgs;
      unfilteredRoot = ../.;

      cargoSrc = lib.fileset.toSource {
        root = unfilteredRoot;
        fileset = craneLib.fileset.commonCargoSources unfilteredRoot;
      };

      workspaceSrc = lib.fileset.toSource {
        root = unfilteredRoot;
        fileset = lib.fileset.unions [
          (craneLib.fileset.commonCargoSources unfilteredRoot)
          ../crates/cache-db/migrations
          ../.sqlx
        ];
      };

      commonBuildArgs = {
        strictDeps = true;

        nativeBuildInputs = [
          pkgs.sqlite
        ]
        ++ lib.optionals pkgs.stdenv.isDarwin [
          pkgs.libiconv
        ];
      };

      cargoArtifacts = craneLib.buildDepsOnly (
        commonBuildArgs
        // {
          src = cargoSrc;
        }
      );

      cacheAppArgs = commonBuildArgs // {
        src = workspaceSrc;
        inherit cargoArtifacts;
        doCheck = false;
      };

      mkWorkspacePackage =
        pname:
        craneLib.buildPackage (
          cacheAppArgs
          // {
            inherit pname;
            cargoExtraArgs = "-p ${pname}";

            meta.mainProgram = pname;
          }
        );

      packages = rec {
        default = cache-app;

        cache-app = mkWorkspacePackage "cache-app";
        cache-ctl = mkWorkspacePackage "cache-ctl";
        cache-push = mkWorkspacePackage "cache-push";
      };

      checks = {
        clippy = craneLib.cargoClippy (
          commonBuildArgs
          // {
            src = workspaceSrc;
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          }
        );

        rust-fmt = craneLib.cargoFmt {
          src = workspaceSrc;
        };

        rust-tests = craneLib.cargoNextest (
          commonBuildArgs
          // {
            src = workspaceSrc;
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
            cargoNextestPartitionsExtraArgs = "--no-tests=pass";
          }
        );

        cargo-doc = craneLib.cargoDoc (
          commonBuildArgs
          // {
            src = workspaceSrc;
            inherit cargoArtifacts;
            env.RUSTDOCFLAGS = "--deny warnings";
          }
        );
      };

      devShellPackages = [
        pkgs.cargo-audit
        pkgs.cargo-udeps
        pkgs.bacon
        pkgs.sqlx-cli
        pkgs.pkg-config
        pkgs.sqlite
        pkgs.rust-analyzer # lsp for agents
      ]
      ++ config.cache-db.devShellPackages
      ++ lib.optionals pkgs.stdenv.isDarwin [
        pkgs.libiconv
      ];
    in
    rec {
      inherit packages checks;

      devShells.default = craneLib.devShell {
        inherit checks;
        packages = devShellPackages;

        shellHook = ''
          ${config.cache-db.installationScript}
          ${config.pre-commit.installationScript}
        '';
      };

      apps = {
        cache-app = {
          type = "app";
          program = lib.getExe self'.packages.cache-app;
        };
        default = apps.cache-app;
      };
    };
}

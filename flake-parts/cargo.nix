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
          ../crates/depot-db/migrations
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

      depotServerArgs = commonBuildArgs // {
        src = workspaceSrc;
        inherit cargoArtifacts;
        doCheck = false;
      };

      mkWorkspacePackage =
        pname:
        craneLib.buildPackage (
          depotServerArgs
          // {
            inherit pname;
            cargoExtraArgs = "-p ${pname}";

            meta.mainProgram = pname;
          }
        );

      packages = rec {
        default = depot-server;

        depot-server = mkWorkspacePackage "depot-server";
        depot-ctl = mkWorkspacePackage "depot-ctl";
        depot-push = mkWorkspacePackage "depot-push";
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

            nativeBuildInputs = commonBuildArgs.nativeBuildInputs ++ [
              # reqwest/rustls needs an explicit CA bundle in the Nix sandbox.
              pkgs.cacert
            ];
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
      ++ config.depot-db.devShellPackages
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
          ${config.depot-db.installationScript}
          ${config.pre-commit.installationScript}
        '';
      };

      apps = {
        depot-server = {
          type = "app";
          program = lib.getExe self'.packages.depot-server;
        };
        default = apps.depot-server;
      };
    };
}

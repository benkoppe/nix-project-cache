{ lib, flake-parts-lib, ... }:
let
  inherit (flake-parts-lib) mkPerSystemOption;
  inherit (lib) mkOption types;
in
{
  options.perSystem = mkPerSystemOption (_: {
    options.depot-db = {
      installationScript = mkOption {
        type = types.lines;
        readOnly = true;
        description = "Shell code that configures the depot DB development environment.";
      };

      devShellPackages = mkOption {
        type = types.listOf types.package;
        readOnly = true;
        description = "Helper commands for working with the depot DB.";
      };
    };
  });

  config.perSystem =
    { pkgs, ... }:
    let
      depotDbInstallationScript = ''
        repo_root="$(${lib.getExe pkgs.git} rev-parse --show-toplevel 2>/dev/null || pwd)"
        export REPO_ROOT="$repo_root"
        export DEPOT_DB_DIR="$REPO_ROOT/dev"
        export DEPOT_DB_PATH="$DEPOT_DB_DIR/depot.db"
        export DEPOT_DB_MIGRATIONS_DIR="$REPO_ROOT/crates/depot-db/migrations"
        export DATABASE_URL="sqlite://$DEPOT_DB_PATH"
        mkdir -p "$DEPOT_DB_DIR"
      '';

      mkDepotDbCommand =
        {
          name,
          body,
        }:
        pkgs.writeShellApplication {
          inherit name;

          runtimeInputs = [
            pkgs.coreutils
            pkgs.git
            pkgs.sqlx-cli
          ];

          text = ''
            set -euo pipefail

            ${depotDbInstallationScript}

            ${body}
          '';
        };

      depotDbCreate = mkDepotDbCommand {
        name = "depot-db-create";
        body = ''
          if [ -f "$DEPOT_DB_PATH" ]; then
            exit 0
          fi

          exec sqlx database create "$@"
        '';
      };

      depotDbMigrate = mkDepotDbCommand {
        name = "depot-db-migrate";
        body = ''
          if [ ! -f "$DEPOT_DB_PATH" ]; then
            sqlx database create
          fi

          exec sqlx migrate run --source "$DEPOT_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      depotDbInfo = mkDepotDbCommand {
        name = "depot-db-info";
        body = ''
          exec sqlx migrate info --source "$DEPOT_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      depotDbAdd = mkDepotDbCommand {
        name = "depot-db-add";
        body = ''
          if [ "$#" -eq 0 ]; then
            printf '%s\n' "usage: depot-db-add <migration-name>" >&2
            exit 1
          fi

          exec sqlx migrate add --source "$DEPOT_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      depotDbReset = mkDepotDbCommand {
        name = "depot-db-reset";
        body = ''
          rm -f "$DEPOT_DB_PATH" "$DEPOT_DB_PATH-wal" "$DEPOT_DB_PATH-shm"
          sqlx database create
          exec sqlx migrate run --source "$DEPOT_DB_MIGRATIONS_DIR" "$@"
        '';
      };
    in
    {
      packages = {
        depot-db-create = depotDbCreate;
        depot-db-migrate = depotDbMigrate;
        depot-db-info = depotDbInfo;
        depot-db-add = depotDbAdd;
        depot-db-reset = depotDbReset;
      };

      depot-db = {
        installationScript = depotDbInstallationScript;
        devShellPackages = [
          depotDbCreate
          depotDbMigrate
          depotDbInfo
          depotDbAdd
          depotDbReset
        ];
      };
    };
}

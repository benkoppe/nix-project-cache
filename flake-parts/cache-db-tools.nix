{ lib, flake-parts-lib, ... }:
let
  inherit (flake-parts-lib) mkPerSystemOption;
  inherit (lib) mkOption types;
in
{
  options.perSystem = mkPerSystemOption (_: {
    options.cache-db = {
      installationScript = mkOption {
        type = types.lines;
        readOnly = true;
        description = "Shell code that configures the cache DB development environment.";
      };

      devShellPackages = mkOption {
        type = types.listOf types.package;
        readOnly = true;
        description = "Helper commands for working with the cache DB.";
      };
    };
  });

  config.perSystem =
    { pkgs, ... }:
    let
      cacheDbInstallationScript = ''
        repo_root="$(${lib.getExe pkgs.git} rev-parse --show-toplevel 2>/dev/null || pwd)"
        export REPO_ROOT="$repo_root"
        export CACHE_DB_DIR="$REPO_ROOT/dev"
        export CACHE_DB_PATH="$CACHE_DB_DIR/cache.db"
        export CACHE_DB_MIGRATIONS_DIR="$REPO_ROOT/crates/cache-db/migrations"
        export DATABASE_URL="sqlite://$CACHE_DB_PATH"
        mkdir -p "$CACHE_DB_DIR"
      '';

      mkCacheDbCommand =
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

            ${cacheDbInstallationScript}

            ${body}
          '';
        };

      cacheDbCreate = mkCacheDbCommand {
        name = "cache-db-create";
        body = ''
          if [ -f "$CACHE_DB_PATH" ]; then
            exit 0
          fi

          exec sqlx database create "$@"
        '';
      };

      cacheDbMigrate = mkCacheDbCommand {
        name = "cache-db-migrate";
        body = ''
          if [ ! -f "$CACHE_DB_PATH" ]; then
            sqlx database create
          fi

          exec sqlx migrate run --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      cacheDbInfo = mkCacheDbCommand {
        name = "cache-db-info";
        body = ''
          exec sqlx migrate info --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      cacheDbAdd = mkCacheDbCommand {
        name = "cache-db-add";
        body = ''
          if [ "$#" -eq 0 ]; then
            printf '%s\n' "usage: cache-db-add <migration-name>" >&2
            exit 1
          fi

          exec sqlx migrate add --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
        '';
      };

      cacheDbReset = mkCacheDbCommand {
        name = "cache-db-reset";
        body = ''
          rm -f "$CACHE_DB_PATH" "$CACHE_DB_PATH-wal" "$CACHE_DB_PATH-shm"
          sqlx database create
          exec sqlx migrate run --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
        '';
      };
    in
    {
      packages = {
        cache-db-create = cacheDbCreate;
        cache-db-migrate = cacheDbMigrate;
        cache-db-info = cacheDbInfo;
        cache-db-add = cacheDbAdd;
        cache-db-reset = cacheDbReset;
      };

      cache-db = {
        installationScript = cacheDbInstallationScript;
        devShellPackages = [
          cacheDbCreate
          cacheDbMigrate
          cacheDbInfo
          cacheDbAdd
          cacheDbReset
        ];
      };
    };
}

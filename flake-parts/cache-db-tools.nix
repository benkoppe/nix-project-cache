{ lib, ... }:
{
  perSystem =
    { pkgs, ... }:
    let
      cacheDbShellEnv = pkgs.writeTextFile {
        name = "cache-db-shell-env";
        destination = "/share/cache-db/env.sh";
        text = ''
          repo_root="$(${lib.getExe pkgs.git} rev-parse --show-toplevel 2>/dev/null || pwd)"
          export REPO_ROOT="$repo_root"
          export CACHE_DB_DIR="$REPO_ROOT/dev"
          export CACHE_DB_PATH="$CACHE_DB_DIR/cache.db"
          export CACHE_DB_MIGRATIONS_DIR="$REPO_ROOT/crates/cache-db/migrations"
          export DATABASE_URL="sqlite://$CACHE_DB_PATH"
          mkdir -p "$CACHE_DB_DIR"
        '';
      };

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

            # shellcheck disable=SC1091
            . "${cacheDbShellEnv}/share/cache-db/env.sh"

            ${body}
          '';
        };
    in
    {
      packages = {
        cache-db-shell-env = cacheDbShellEnv;

        cache-db-create = mkCacheDbCommand {
          name = "cache-db-create";
          body = ''
            if [ -f "$CACHE_DB_PATH" ]; then
              exit 0
            fi

            exec sqlx database create "$@"
          '';
        };

        cache-db-migrate = mkCacheDbCommand {
          name = "cache-db-migrate";
          body = ''
            if [ ! -f "$CACHE_DB_PATH" ]; then
              sqlx database create
            fi

            exec sqlx migrate run --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
          '';
        };

        cache-db-info = mkCacheDbCommand {
          name = "cache-db-info";
          body = ''
            exec sqlx migrate info --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
          '';
        };

        cache-db-add = mkCacheDbCommand {
          name = "cache-db-add";
          body = ''
            if [ "$#" -eq 0 ]; then
              printf '%s\n' "usage: cache-db-add <migration-name>" >&2
              exit 1
            fi

            exec sqlx migrate add --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
          '';
        };

        cache-db-reset = mkCacheDbCommand {
          name = "cache-db-reset";
          body = ''
            rm -f "$CACHE_DB_PATH" "$CACHE_DB_PATH-wal" "$CACHE_DB_PATH-shm"
            sqlx database create
            exec sqlx migrate run --source "$CACHE_DB_MIGRATIONS_DIR" "$@"
          '';
        };
      };
    };
}

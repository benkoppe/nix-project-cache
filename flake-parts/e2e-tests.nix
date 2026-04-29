{ lib, ... }:
{
  perSystem =
    { pkgs, self', ... }:
    let
      depotServer = self'.packages.depot-server;
      depotCtl = self'.packages.depot-ctl;
      depotPush = self'.packages.depot-push;

      serverUrl = "http://127.0.0.1:8080";
      stateDir = "/var/lib/depot";
      signingSecret = "${stateDir}/depot.sec";
      signingPublic = "${stateDir}/depot.pub";

      s3Bucket = "repo-depot-test";
      s3AccessKey = "minioadmin";
      s3SecretKey = "minioadmin";
      s3Endpoint = "http://127.0.0.1:9000";

      toml = pkgs.formats.toml { };

      mkMinioNodeConfig =
        {
          bucket ? s3Bucket,
          accessKey ? s3AccessKey,
          secretKey ? s3SecretKey,
          endpointHost ? "127.0.0.1",
          port ? 9000,
        }:
        {
          users.groups.minio-test = { };

          users.users.minio-test = {
            isSystemUser = true;
            group = "minio-test";
          };

          systemd.tmpfiles.rules = [
            "d /var/lib/minio 0750 minio-test minio-test -"
            "d /var/lib/minio/data 0750 minio-test minio-test -"
          ];

          systemd.services = {
            minio = {
              description = "MinIO S3-compatible object storage";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];

              path = [
                pkgs.getent
              ];

              environment = {
                MINIO_ROOT_USER = accessKey;
                MINIO_ROOT_PASSWORD = secretKey;
              };

              serviceConfig = {
                ExecStart = "${lib.getExe pkgs.minio} server --address ${endpointHost}:${toString port} /var/lib/minio/data";
                User = "minio-test";
                Group = "minio-test";
                Restart = "on-failure";
              };
            };

            minio-setup = {
              description = "Setup MinIO bucket";
              after = [ "minio.service" ];
              requires = [ "minio.service" ];
              before = [ "depot-server.service" ];
              wantedBy = [ "multi-user.target" ];

              environment = {
                S3_ENDPOINT_URL = "http://${endpointHost}:${toString port}";
                AWS_ACCESS_KEY_ID = accessKey;
                AWS_SECRET_ACCESS_KEY = secretKey;
              };

              path = [ pkgs.s5cmd ];

              script = ''
                set -euo pipefail

                ready=0
                for i in $(seq 60); do
                  if s5cmd ls 2>/dev/null; then
                    ready=1
                    break
                  fi

                  echo "Waiting for MinIO to start... ($i/60)"
                  sleep 2
                done

                if [ "$ready" -eq 0 ]; then
                  echo "ERROR: MinIO did not become ready after 60 attempts" >&2
                  exit 1
                fi

                s5cmd mb s3://${bucket} || true
              '';

              serviceConfig = {
                Type = "oneshot";
                RemainAfterExit = true;
              };
            };

            depot-server = {
              after = [ "minio-setup.service" ];
              requires = [ "minio-setup.service" ];
            };
          };
        };

      mkDepotSubstituterTest =
        {
          name,
          appConfig ? { },
          extraNodeConfig ? { },
          extraTestScript ? "",
        }:
        let
          baseAppConfig = {
            server.bind_address = "127.0.0.1:8080";
            database.path = "${stateDir}/depot.db";
            auth.write_token = "test-token";
            signing.aggregate_key_file = signingSecret;
          };

          depotServerConfig = toml.generate "${name}-depot-server.toml" (
            lib.recursiveUpdate baseAppConfig appConfig
          );
        in
        pkgs.testers.nixosTest {
          inherit name;

          nodes.machine =
            {
              pkgs,
              lib,
              ...
            }:
            lib.mkMerge [
              {
                environment.systemPackages = [
                  pkgs.curl
                  pkgs.nix
                  depotServer
                  depotCtl
                  depotPush
                ];

                nix.settings = {
                  experimental-features = [
                    "nix-command"
                    "flakes"
                  ];

                  trusted-users = [
                    "root"
                  ];

                  substituters = lib.mkForce [ ];
                };

                networking.firewall.enable = false;

                systemd.services.depot-server = {
                  wantedBy = [ "multi-user.target" ];
                  after = [ "network.target" ];

                  preStart = ''
                    set -euo pipefail

                    mkdir -p ${stateDir}

                    if [ ! -f ${signingSecret} ]; then
                      ${lib.getExe depotCtl} keys generate \
                        --name depot.example.com-1 \
                        --secret-file ${signingSecret} \
                        --public-file ${signingPublic}
                      fi
                  '';

                  serviceConfig = {
                    ExecStart = "${lib.getExe depotServer} --config ${depotServerConfig}";
                    Restart = "on-failure";
                    StateDirectory = "depot";
                  };
                };
              }

              extraNodeConfig
            ];

          testScript = ''
            machine.start()
            machine.wait_for_unit("multi-user.target")
            machine.wait_for_unit("depot-server.service")

            machine.wait_until_succeeds(
              "[ \"$(curl -sS -o /dev/null -w '%{http_code}' ${serverUrl}/__ready__)\" = 404 ]"
            )

            machine.succeed(
              "depot-ctl "
              "--server ${serverUrl} "
              "--auth-token test-token "
              "projects create example_repo "
              "--display-name 'Example Repo' "
              "--public "
              "--if-not-exists"
            )

            machine.succeed("rm -rf /tmp/e2e-source")
            machine.succeed("mkdir -p /tmp/e2e-source")
            machine.succeed("printf 'hello from e2e depot\\n' > /tmp/e2e-source/hello.txt")

            store_path = machine.succeed(
              "nix-store --add-fixed --recursive sha256 /tmp/e2e-source"
            ).strip()

            machine.succeed(
              "depot-push "
              "--server ${serverUrl} "
              "--auth-token test-token "
              "--project example_repo "
              "--ref refs/heads/main "
              "--revision e2e-test "
              "--max-concurrent-uploads 1 "
              f"{store_path}"
            )

            ${extraTestScript}

            store_hash = store_path.split("/")[-1].split("-")[0]

            narinfo = machine.succeed(
              f"curl -fsS ${serverUrl}/{store_hash}.narinfo"
            )

            assert f"StorePath: {store_path}" in narinfo
            assert "Sig: depot.example.com-1:" in narinfo

            # Force Nix to need repo-depot. The path was only created to publish it.
            machine.succeed(f"nix-store --delete {store_path}")
            machine.fail(f"test -e {store_path}")

            public_key = machine.succeed("cat ${signingPublic}").strip()

            machine.succeed(
              "nix-store "
              f"--realise {store_path} "
              "--option substituters ${serverUrl} "
              f"--option trusted-public-keys '{public_key}'"
            )

            machine.succeed(f"test -f {store_path}/hello.txt")
            restored = machine.succeed(f"cat {store_path}/hello.txt").strip()
            assert restored == "hello from e2e depot"
          '';
        };

      e2eChecks = lib.optionalAttrs pkgs.stdenv.isLinux {
        e2e-depot-substituter-fs = mkDepotSubstituterTest {
          name = "e2e-depot-substituter-fs";

          appConfig = {
            storage = {
              backends.main = {
                type = "filesystem";
                root = "${stateDir}/objects";
              };
            };
          };
        };

        e2e-depot-substituter-s3 = mkDepotSubstituterTest {
          name = "e2e-depot-substituter-s3";

          appConfig = {
            storage = {
              backends.main = {
                type = "s3";
                endpoint = s3Endpoint;
                bucket = s3Bucket;
                region = "us-east-1";
                access_key_id = s3AccessKey;
                secret_access_key = s3SecretKey;
                force_path_style = true;
                prefix = "objects";
              };
            };
          };

          extraNodeConfig = mkMinioNodeConfig { };

          extraTestScript = ''
            machine.succeed(
              "AWS_ACCESS_KEY_ID=${s3AccessKey} "
              "AWS_SECRET_ACCESS_KEY=${s3SecretKey} "
              "S3_ENDPOINT_URL=${s3Endpoint} "
              "${lib.getExe pkgs.s5cmd} ls 's3://${s3Bucket}/objects/nar/*'"
            )
          '';
        };
      };
    in
    {
      checks = e2eChecks;
    };
}

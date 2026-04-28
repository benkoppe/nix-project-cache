_: {
  perSystem = _: {
    pre-commit = {
      check.enable = false; # only run on pre-commit, not in CI

      settings = {
        src = ../.;

        default_stages = [ "pre-push" ];

        hooks = {
          nixfmt.enable = true;
          rustfmt.enable = true;

          # sqlx check custom hook
          sql-prepare = {
            enable = true;
            entry = "cargo sqlx prepare --workspace -- --all-targets";
            # add `--check` to check only. Without it the file will be updated when the hook is run
            pass_filenames = false;
          };
        };
      };
    };
  };
}

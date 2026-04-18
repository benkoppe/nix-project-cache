_: {
  perSystem = _: {
    pre-commit = {
      check.enable = false; # only run on pre-commit, not in CI

      settings = {
        src = ../.;
        hooks = {
          nixfmt.enable = true;
          rustfmt.enable = true;
        };
      };
    };
  };
}

# Repo formatter: a single self-contained `treefmt` wrapper (`repofmt`).
#
# Only the formatters this repo uses are wired in. Add another as new file types
# appear — one block under `formatter`: a `command` (a Nix-provided binary) plus
# the `includes` globs it owns.
pkgs:
let
  lib = pkgs.lib;

  # rustfmt from the repo toolchain (rust-toolchain.toml via rust-overlay), so the formatter
  # and the build agree on edition.
  rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml;

  # mdformat formats fenced code through `mdformat.codeformatter` entry points, one per language.
  # nixpkgs ships one for bash (mdformat-beautysh) but none for Rust, hence this: rustfmt reached by
  # absolute store path, so nothing depends on what happens to be on PATH at format time.
  rustfmtPluginPyproject = pkgs.writeText "pyproject.toml" ''
    [build-system]
    requires = ["setuptools"]
    build-backend = "setuptools.build_meta"

    [project]
    name = "mdformat-rustfmt-local"
    version = "0.1.0"

    [project.entry-points."mdformat.codeformatter"]
    rust = "mdformat_rustfmt_local:format_rust"

    [tool.setuptools]
    py-modules = ["mdformat_rustfmt_local"]
  '';

  rustfmtPluginModule = pkgs.writeText "mdformat_rustfmt_local.py" ''
    """Format Rust fences in Markdown with this repo's pinned rustfmt."""

    import subprocess

    RUSTFMT = "${lib.getExe' rustToolchain "rustfmt"}"


    def format_rust(unformatted: str, _info_str: str) -> str:
        done = subprocess.run(
            [RUSTFMT, "--edition", "2024", "--emit", "stdout", "--quiet"],
            input=unformatted,
            capture_output=True,
            text=True,
            check=False,
        )
        # Raise rather than hand back the input: mdformat swallows a codeformatter's
        # exception into a warning naming the file and line, and that warning is the only
        # signal a fence stopped being valid Rust. Returning it unchanged would be silent.
        if done.returncode:
            raise ValueError(done.stderr.strip() or "rustfmt rejected this block")
        return done.stdout
  '';

  mdformatRustfmt = pkgs.python3Packages.buildPythonPackage {
    pname = "mdformat-rustfmt-local";
    version = "0.1.0";
    pyproject = true;
    build-system = [ pkgs.python3Packages.setuptools ];
    doCheck = false;
    src = pkgs.runCommand "mdformat-rustfmt-local-src" { } ''
      mkdir -p $out
      cp ${rustfmtPluginPyproject} $out/pyproject.toml
      cp ${rustfmtPluginModule} $out/mdformat_rustfmt_local.py
    '';
  };

  # Same shape as the Rust plugin above. `mdformat-config` on PyPI would cover TOML,
  # but it isn't in nixpkgs and also brings JSON and YAML fence formatters this repo
  # hasn't asked for; going local keeps fences on the very taplo that formats the `.toml` files.
  taploPluginPyproject = pkgs.writeText "pyproject.toml" ''
    [build-system]
    requires = ["setuptools"]
    build-backend = "setuptools.build_meta"

    [project]
    name = "mdformat-taplo-local"
    version = "0.1.0"

    [project.entry-points."mdformat.codeformatter"]
    toml = "mdformat_taplo_local:format_toml"

    [tool.setuptools]
    py-modules = ["mdformat_taplo_local"]
  '';

  taploPluginModule = pkgs.writeText "mdformat_taplo_local.py" ''
    """Format TOML fences in Markdown with the taplo that formats this repo's .toml files."""

    import subprocess

    TAPLO = "${lib.getExe pkgs.taplo}"


    def format_toml(unformatted: str, _info_str: str) -> str:
        done = subprocess.run(
            [TAPLO, "format", "--colors", "never", "-"],
            input=unformatted,
            capture_output=True,
            text=True,
            check=False,
        )
        # Raise rather than hand back the input, as in the Rust plugin: the warning mdformat
        # makes of it is the only sign a fence stopped being valid TOML.
        if done.returncode:
            raise ValueError(done.stderr.strip() or "taplo rejected this block")
        return done.stdout
  '';

  mdformatTaplo = pkgs.python3Packages.buildPythonPackage {
    pname = "mdformat-taplo-local";
    version = "0.1.0";
    pyproject = true;
    build-system = [ pkgs.python3Packages.setuptools ];
    doCheck = false;
    src = pkgs.runCommand "mdformat-taplo-local-src" { } ''
      mkdir -p $out
      cp ${taploPluginPyproject} $out/pyproject.toml
      cp ${taploPluginModule} $out/mdformat_taplo_local.py
    '';
  };

  treefmtConfig = pkgs.treefmt.buildConfig {
    on-unmatched = "debug";
    formatter = {
      nix = {
        command = lib.getExe pkgs.nixpkgs-fmt;
        includes = [ "*.nix" ];
      };

      shell = {
        command = lib.getExe pkgs.shfmt;
        options = [ "--simplify" "--write" "--binary-next-line" "--indent" "4" ];
        includes = [ "*.sh" "*.bash" "*.envrc" "*.envrc.*" ];
      };

      python = {
        command = lib.getExe pkgs.ruff;
        options = [ "format" ];
        includes = [ "*.py" ];
      };

      markdown = {
        command = lib.getExe (pkgs.mdformat.withPlugins (p: [
          p.mdformat-gfm
          p.mdformat-frontmatter
          p.mdformat-simple-breaks
          p.mdformat-beautysh
          mdformatRustfmt
          mdformatTaplo
        ]));
        # Naming the code formatters makes mdformat *require* them: a plugin that stops loading
        # is an error rather than fences silently going unformatted.
        options = [
          "--number"
          "--wrap=120"
          "--codeformatters"
          "rust"
          "--codeformatters"
          "bash"
          "--codeformatters"
          "toml"
        ];
        includes = [ "*.md" "*.markdown" ];
      };

      # `just --fmt` takes one file at a time (so we loop) and is idempotent.
      # `--fmt` is upstream-unstable, hence `--unstable`.
      justfile = {
        command = pkgs.writeShellScript "just-format" ''
          for file in "$@"; do
            ${lib.getExe pkgs.just} --unstable --fmt --justfile "$file"
          done
        '';
        includes = [ "justfile" "**/justfile" "Justfile" "**/Justfile" "*.just" "*.justfile" ];
      };

      rust = {
        command = lib.getExe' rustToolchain "rustfmt";
        options = [ "--edition" "2024" ];
        includes = [ "*.rs" ];
      };

      # svgo rewrites files unconditionally, and treefmt's `--fail-on-change` compares mtime.
      # So a cache-cold run (the `nix flake check` sandbox) flags every SVG on the mtime bump alone,
      # byte-identical or not. The wrapper rewrites only on a real diff, so mtime is kept.
      svg = {
        command = lib.getExe (pkgs.writeShellApplication {
          name = "svgo-fmt";
          runtimeInputs = [ pkgs.svgo pkgs.coreutils pkgs.diffutils ];
          text = ''
            for f in "$@"; do
              tmp=$(mktemp)
              svgo --quiet --config ${../svgo.config.js} --input "$f" --output "$tmp"
              if cmp -s "$tmp" "$f"; then rm -f "$tmp"; else mv "$tmp" "$f"; fi
            done
          '';
        });
        includes = [ "*.svg" ];
      };

      # Not `Cargo.toml`: cargo owns its manifests — `cargo add` writes its own shape,
      # and taplo restructuring a dependency entry only starts a fight neither side wins.
      toml = {
        command = lib.getExe pkgs.taplo;
        options = [ "format" ];
        includes = [ "*.toml" ];
        excludes = [ "Cargo.toml" "**/Cargo.toml" ];
      };

      yaml = {
        command = lib.getExe pkgs.yamlfmt;
        includes = [ "*.yml" "*.yaml" ];
      };
    };
  };
in
pkgs.writeShellApplication {
  name = "repofmt";
  # git: detect the tree root in a dev checkout; without git (e.g. the
  # `nix flake check` sandbox) fall back to a filesystem walk.
  # diffutils: backs the CI assert-zero-changes gate — CI runs the formatters,
  # then fails if the tree changed (`repofmt --fail-on-change`).
  runtimeInputs = with pkgs; [ gitMinimal diffutils ];
  text = ''
    if tree_root=$(git rev-parse --show-toplevel 2>/dev/null); then
      walk=git
    else
      walk=filesystem
      tree_root=.
    fi

    exec ${lib.getExe pkgs.treefmt} \
      --config-file ${treefmtConfig} \
      --tree-root "$tree_root" \
      --walk "$walk" \
      "$@"
  '';
}

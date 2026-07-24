# Repo formatter: a single self-contained `treefmt` wrapper (`repofmt`).
#
# Only the formatters this repo uses are wired in. Add another as new file types
# appear — one block under `formatter`: a `command` (a Nix-provided binary) plus
# the `includes` globs it owns.
pkgs:
let
  lib = pkgs.lib;

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
        ]));
        options = [ "--number" "--wrap=120" ];
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

      # rustfmt from the repo toolchain (rust-toolchain.toml via rust-overlay), so the formatter
      # and the build agree on edition.
      rust = {
        command = lib.getExe' (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml) "rustfmt";
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

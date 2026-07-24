# Repo checker (`repochk`): lints shell scripts
# with shellcheck and the justfile with `just --fmt --check`.
#
# Enumeration is git-based, and deliberately covers untracked-but-not-ignored
# files too: a checker that only sees committed ones passes a brand-new script
# it never read.
#
# Add a checker by dropping another entry into `checkers`
# (a `command` that exits non-zero on failure, plus the `includes` globs it owns).
pkgs:
let
  lib = pkgs.lib;

  checkers = [
    {
      name = "shell";
      includes = [ "*.sh" "*.bash" ];
      command = pkgs.writeShellScript "shell-check" ''
        exec ${lib.getExe pkgs.shellcheck} -x "$1"
      '';
    }
    {
      name = "python";
      includes = [ "*.py" ];
      command = pkgs.writeShellScript "python-check" ''
        exec ${lib.getExe pkgs.ruff} check "$1"
      '';
    }
    {
      name = "justfile";
      includes = [ "justfile" "**/justfile" "Justfile" "**/Justfile" "*.just" "*.justfile" ];
      # `just --fmt --check` parses the justfile and verifies canonical formatting
      # without modifying it. `--fmt` is upstream-unstable, hence `--unstable`.
      command = pkgs.writeShellScript "justfile-check" ''
        exec ${lib.getExe pkgs.just} --unstable --fmt --check --justfile "$1"
      '';
    }
  ];
in
pkgs.writeShellApplication {
  name = "repochk";
  runtimeInputs = with pkgs; [ gitMinimal ];
  text = ''
    if ! tree_root=$(git rev-parse --show-toplevel 2>/dev/null); then
      echo "repochk: not a git repository" >&2
      exit 1
    fi

    echo "checker root: $tree_root"
    failed=0
    checked=0

    ${lib.concatMapStringsSep "\n" (checker: ''
      # ${checker.name}
      while IFS= read -r file; do
        if [ -n "$file" ]; then
          checked=$((checked + 1))
          if ${checker.command} "$tree_root/$file" 2>&1; then
            echo "  PASS: $file"
          else
            echo "  FAIL: $file" >&2
            failed=$((failed + 1))
          fi
        fi
      done < <(git -C "$tree_root" ls-files --cached --others --exclude-standard -- ${lib.concatMapStringsSep " " (p: ''"${p}"'') checker.includes})
    '') checkers}

    echo ""
    echo "checked $checked files, $failed failed"
    [ "$failed" -eq 0 ]
  '';
}

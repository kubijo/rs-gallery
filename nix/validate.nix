# `validate`: the full local gate — formatting, repo lint, clippy, and the tests under coverage
# (text + HTML for humans, lcov + cobertura for CI, into target/llvm-cov).
{ pkgs, formatter, checker }:
pkgs.writeShellApplication {
  name = "validate";
  runtimeInputs = [
    (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml)
    pkgs.cargo-llvm-cov
    pkgs.cargo-nextest
    pkgs.uv
    pkgs.ty
    formatter
    checker
  ];
  text = ''
    repofmt --fail-on-change
    repochk
    cargo clippy --all-targets -- -D warnings
    # tools/ is its own uv project, and both of these resolve imports
    # from its root — run them there rather than through repochk,
    # which lints file by file and would see no project at all.
    (cd tools && ty check && uv run --frozen pytest -q)
    # `--no-report` accumulates into the target dir by design,
    # so without this the reports merge every earlier run:
    # records whose structural hash has since changed surface
    # as "N functions have mismatched data", and the totals
    # count code that no longer exists.
    cargo llvm-cov clean --workspace
    cargo llvm-cov --no-report nextest
    cargo llvm-cov report
    cargo llvm-cov report --html
    cargo llvm-cov report --lcov --output-path target/llvm-cov/lcov.info
    cargo llvm-cov report --cobertura --output-path target/llvm-cov/cobertura.xml
  '';
}

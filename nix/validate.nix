# `validate`: the full local gate — formatting, repo lint, clippy, and the tests under coverage
# (text + HTML for humans, lcov + cobertura for CI, into .tmp/coverage).
{ pkgs, formatter, checker, test }:
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
    test
  ];
  text = ''
    repofmt --fail-on-change
    repochk
    cargo clippy --workspace --all-targets -- -D warnings
    # Release too: egui gates `Style::debug` on `debug_assertions`, so code touching
    # it compiles in dev and fails in release — a dev-only gate never sees that.
    cargo clippy --workspace --release --all-targets -- -D warnings
    # Once with every feature on: the forwarded `egui_extras` gates are off by default,
    # so a default-only run never builds the code they pull in.
    cargo clippy --workspace --all-targets --all-features -- -D warnings
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
    # Through the wrapper, which carries the GL stack the capture tests render on.
    gallery-test
    cargo llvm-cov report
    # Coverage is a report to read rather than build output,
    # so it sits with the other scratch instead of under the target dir it would default into.
    cargo llvm-cov report --html --output-dir .tmp/coverage
    cargo llvm-cov report --lcov --output-path .tmp/coverage/lcov.info
    cargo llvm-cov report --cobertura --output-path .tmp/coverage/cobertura.xml
  '';
}

# `validate`: the full local gate — formatting, repo lint, clippy, and the tests under coverage
# (text + HTML for humans, lcov + cobertura for CI, into target/llvm-cov).
{ pkgs, formatter, checker }:
pkgs.writeShellApplication {
  name = "validate";
  runtimeInputs = [
    (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml)
    pkgs.cargo-llvm-cov
    pkgs.cargo-nextest
    formatter
    checker
  ];
  text = ''
    repofmt --fail-on-change
    repochk
    cargo clippy --all-targets -- -D warnings
    cargo llvm-cov --no-report nextest
    cargo llvm-cov report
    cargo llvm-cov report --html
    cargo llvm-cov report --lcov --output-path target/llvm-cov/lcov.info
    cargo llvm-cov report --cobertura --output-path target/llvm-cov/cobertura.xml
  '';
}

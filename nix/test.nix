# `gallery-test`: the test run, on the software GL stack `with-software-gl` attaches.
{ pkgs, softwareGl }:
pkgs.writeShellApplication {
  name = "gallery-test";
  runtimeInputs = [
    (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml)
    pkgs.cargo-llvm-cov
    pkgs.cargo-nextest
    softwareGl
  ];
  text = ''
    # `--workspace`: the root manifest is both a package and the workspace root, so cargo on its own
    # runs `gallery`'s tests alone and never reaches `gallery-build` or `gallery-macros`.
    exec with-software-gl cargo llvm-cov --no-report nextest --workspace "$@"
  '';
}

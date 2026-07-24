{
  description = "gallery — an egui-shelled component catalog with Storybook-style scene discovery";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (
        system: f (import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        })
      );
      formatterFor = pkgs: import ./nix/formatter.nix pkgs;
      checkerFor = pkgs: import ./nix/checker.nix pkgs;
      validateFor = pkgs: import ./nix/validate.nix {
        inherit pkgs;
        formatter = formatterFor pkgs;
        checker = checkerFor pkgs;
      };
    in
    {
      # `nix fmt` (and `just format`) run this wrapper over the whole tree.
      formatter = forAllSystems formatterFor;

      # Forge-agnostic CI gate: `nix flake check` fails if anything is unformatted or fails the repo lint.
      checks = forAllSystems (pkgs: {
        formatting =
          pkgs.runCommandLocal "check-formatting" { nativeBuildInputs = [ (formatterFor pkgs) ]; }
            ''
              cp -r ${self} work && chmod -R u+w work && cd work
              export HOME="$TMPDIR"
              repofmt --fail-on-change
              touch "$out"
            '';
        checking =
          pkgs.runCommandLocal "check-lint" { nativeBuildInputs = [ (checkerFor pkgs) pkgs.gitMinimal ]; }
            ''
              cp -r ${self} work && chmod -R u+w work && cd work
              export HOME="$TMPDIR"
              git init -q && git add -A
              repochk
              touch "$out"
            '';
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.just
            (formatterFor pkgs)
            (checkerFor pkgs)
            # Rust toolchain from rust-toolchain.toml (rust-overlay) + test/deps tooling.
            (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            pkgs.cargo-nextest
            pkgs.cargo-llvm-cov
            (validateFor pkgs) # `validate`: the full gate
            pkgs.cargo-outdated
            pkgs.cargo-deny
            pkgs.cargo-generate
            pkgs.uv
            pkgs.ruff
            pkgs.ty
            pkgs.samply
            # egui/eframe build tooling.
            pkgs.pkg-config
          ];
          # egui/wgpu dlopen these at runtime (examples, or a consumer's binary).
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
            pkgs.libGL
            pkgs.libxkbcommon
            pkgs.wayland
            pkgs.vulkan-loader
            pkgs.libx11
            pkgs.libxcursor
            pkgs.libxi
            pkgs.libxrandr
          ];
        };
      });
    };
}

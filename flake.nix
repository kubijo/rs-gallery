{
  description = "gallery — an egui-shelled component catalog with Storybook-style scene discovery";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # `tools/` has real dependencies now, so nix builds its environment from the same `uv.lock`
    # uv resolves — one lockfile rather than a nix-side restatement of it.
    pyproject-nix = {
      url = "github:pyproject-nix/pyproject.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    uv2nix = {
      url = "github:pyproject-nix/uv2nix";
      inputs.pyproject-nix.follows = "pyproject-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    pyproject-build-systems = {
      url = "github:pyproject-nix/build-system-pkgs";
      inputs.pyproject-nix.follows = "pyproject-nix";
      inputs.uv2nix.follows = "uv2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, rust-overlay, pyproject-nix, uv2nix, pyproject-build-systems }:
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
      testFor = pkgs: import ./nix/test.nix { inherit pkgs; };

      # `tools/`, built from its own `uv.lock`: the interpreter is pinned here, the dependency set
      # comes from the file uv resolves, and the console scripts (`gallery-perf`, `gallery-release`)
      # land on PATH. `deps.all` takes the dev group with it, which is what runs the tests.
      pythonToolsFor =
        pkgs:
        let
          workspace = uv2nix.lib.workspace.loadWorkspace { workspaceRoot = ./tools; };
          pythonSet =
            (pkgs.callPackage pyproject-nix.build.packages { python = pkgs.python314; }).overrideScope
              (nixpkgs.lib.composeManyExtensions [
                pyproject-build-systems.overlays.default
                (workspace.mkPyprojectOverlay { sourcePreference = "wheel"; })
              ]);
        in
        pythonSet.mkVirtualEnv "gallery-tools-env" workspace.deps.all;

      validateFor = pkgs: import ./nix/validate.nix {
        inherit pkgs;
        formatter = formatterFor pkgs;
        checker = checkerFor pkgs;
        test = testFor pkgs;
        pythonTools = pythonToolsFor pkgs;
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
            (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
            pkgs.cargo-nextest
            pkgs.cargo-llvm-cov
            (validateFor pkgs)
            (testFor pkgs)
            pkgs.cargo-outdated
            pkgs.cargo-deny
            pkgs.cargo-generate
            # The tools' own environment: their interpreter, their dependencies, and the console
            # scripts the `just` recipes call. `uv` stays for maintaining the lockfile it is built
            # from.
            (pythonToolsFor pkgs)
            pkgs.uv
            pkgs.ruff
            pkgs.ty
            pkgs.samply
            pkgs.binutils
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

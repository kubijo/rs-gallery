{
  description = "gallery — an egui-shelled component catalog with Storybook-style scene discovery";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nix-tools.url = "github:kubijo/nix-tools/v0.3.0";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # `tools/` builds from the same `uv.lock` uv resolves, not a nix-side restatement of it.
    pyproject-nix = {
      url = "github:pyproject-nix/pyproject.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    uv2nix = {
      url = "github:pyproject-nix/uv2nix";
      inputs = {
        pyproject-nix.follows = "pyproject-nix";
        nixpkgs.follows = "nixpkgs";
      };
    };
    pyproject-build-systems = {
      url = "github:pyproject-nix/build-system-pkgs";
      inputs = {
        pyproject-nix.follows = "pyproject-nix";
        uv2nix.follows = "uv2nix";
        nixpkgs.follows = "nixpkgs";
      };
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      nix-tools,
      rust-overlay,
      pyproject-nix,
      uv2nix,
      pyproject-build-systems,
    }:
    let
      inherit (nixpkgs) lib;

      # x86_64-darwin is absent because nixpkgs 26.11 dropped it.
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      pythonToolsFor =
        pkgs:
        let
          workspace = uv2nix.lib.workspace.loadWorkspace { workspaceRoot = ./tools; };
          pythonSet =
            (pkgs.callPackage pyproject-nix.build.packages { python = pkgs.python314; }).overrideScope
              (
                lib.composeManyExtensions [
                  pyproject-build-systems.overlays.default
                  (workspace.mkPyprojectOverlay { sourcePreference = "wheel"; })
                ]
              );
        in
        # `deps.all` takes the dev group with it, which is what runs the tests.
        pythonSet.mkVirtualEnv "gallery-tools-env" workspace.deps.all;

      each = lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          pythonTools = pythonToolsFor pkgs;
          test = import ./nix/test.nix { inherit pkgs; };

          project = nix-tools.lib.configure {
            inherit system;
            src = self;
            # The svg formatter runs on it; nothing else here does.
            nodejs = pkgs.nodejs_24;

            format = {
              # `onUnmatched = "fatal"`, so whatever no formatter claims has to be named here.
              exclude = [
                # `png` stays off besides: tests/snapshots/ holds byte-compared references.
                "*.png"
                "*.ttf"
                # Cargo-generate removes `.liquid`;
                # keeping the source inert stops vendors parsing it.
                "template/Cargo.toml.liquid"
                # Kept as received: this repo's licence, and the font's own.
                "UNLICENSE"
                "fonts/noto/OFL.txt"
              ];
              # nixpkgs' rustfmt would drag in a second toolchain beside the pinned one.
              rust = {
                exe = lib.getExe' rustToolchain "rustfmt";
                configFile = ./rustfmt.toml;
              };
              # Kept over the library's: it would reflow to 120 columns and single quotes,
              # and drop the rule selection under `[tool.ruff.lint]`.
              python.configFile = ./tools/pyproject.toml;
              javascript = true;
              svg.configFile = ./svgo.config.js;
            };

            lint = {
              python.configFile = ./tools/pyproject.toml;
              javascript = true;
            };

            validate = {
              runtimeInputs = [
                rustToolchain
                pkgs.cargo-llvm-cov
                pkgs.cargo-nextest
                pkgs.ty
                test
                pythonTools
              ];
              steps = [
                "cargo clippy --workspace --all-targets -- -D warnings"
                # egui gates `Style::debug` on `debug_assertions`, so code touching it compiles
                # in dev and fails in release.
                "cargo clippy --workspace --release --all-targets -- -D warnings"
                # The forwarded `egui_extras` gates are off by default, so nothing else builds them.
                "cargo clippy --workspace --all-targets --all-features -- -D warnings"
                # tools/ is its own uv project and both resolve imports from its root —
                # something repochk's file-by-file lint never sees.
                "(cd tools && ty check --python ${pythonTools} . && pytest -q)"
                {
                  name = "tests under coverage";
                  # One step: the gate keeps going after a failure, and a report built
                  # on a failed test run is noise.
                  run = lib.concatStringsSep " && " [
                    # `--no-report` accumulates into the target dir by design: without the clean,
                    # the reports merge every earlier run and count code that is gone.
                    "cargo llvm-cov clean --workspace"
                    # Through the wrapper, which carries the GL stack the capture tests render on.
                    "gallery-test"
                    "cargo llvm-cov report"
                    # A report to read rather than build output, hence .tmp over the target dir.
                    "cargo llvm-cov report --html --output-dir .tmp/coverage"
                    "cargo llvm-cov report --lcov --output-path .tmp/coverage/lcov.info"
                    "cargo llvm-cov report --cobertura --output-path .tmp/coverage/cobertura.xml"
                  ];
                }
              ];
            };
          };
        in
        {
          inherit (project) formatter checks apps;

          devShell = pkgs.mkShell {
            packages = project.packages ++ [
              pkgs.just
              rustToolchain
              pkgs.cargo-nextest
              pkgs.cargo-llvm-cov
              test
              pkgs.cargo-outdated
              pkgs.cargo-deny
              pkgs.cargo-generate
              pythonTools
              # Kept beside the built environment, to maintain the lockfile it is built from.
              pkgs.uv
              # The pinned one, so what a shell or an editor reports is what the gate enforces.
              (nix-tools.lib.toolPkgsFor system).ruff
              pkgs.ty
              pkgs.samply
              pkgs.binutils
              pkgs.pkg-config
            ];
            # egui/wgpu dlopen these at runtime (examples, or a consumer's binary).
            LD_LIBRARY_PATH = lib.makeLibraryPath [
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
        }
      );

      pick = attr: lib.mapAttrs (_: perSystem: perSystem.${attr}) each;
    in
    {
      formatter = pick "formatter";
      checks = pick "checks";
      apps = pick "apps";
      devShells = lib.mapAttrs (_: perSystem: { default = perSystem.devShell; }) each;
    };
}

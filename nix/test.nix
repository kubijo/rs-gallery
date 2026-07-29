# `gallery-test`: the test run, with a GL stack attached. A CI runner has neither a GPU nor a system
# EGL; mesa supplies a software device through `EGL_MESA_device_software`, needing no display server.
#
# On this wrapper rather than the dev shell because `__EGL_VENDOR_LIBRARY_FILENAMES` *replaces* the
# driver list — shell-wide it would take the machine's own GPU away from `just run`.
{ pkgs }:
pkgs.writeShellApplication {
  name = "gallery-test";
  runtimeInputs = [
    (pkgs.rust-bin.fromRustupToolchainFile ../rust-toolchain.toml)
    pkgs.cargo-llvm-cov
    pkgs.cargo-nextest
  ];
  runtimeEnv = {
    # Where libglvnd finds mesa's EGL driver. Without it there is no device enumeration to query.
    __EGL_VENDOR_LIBRARY_FILENAMES = "${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json";

    # Mesa's own libraries are opened at runtime, so they have to be findable.
    LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.mesa pkgs.libGL ];

    # A runner has no GPU to fall back on.
    LIBGL_ALWAYS_SOFTWARE = "1";

    # Pin the rasteriser, not just the driver: a reference image
    # is only worth comparing against if every machine draws
    # it identically, and llvmpipe from this pinned mesa does.
    #
    # A GPU would render the same scene with its own antialiasing
    # and the snapshots would differ per developer.
    GALLERY_CAPTURE_RENDERER = "llvmpipe";
  };
  text = ''
    # `--workspace`: the root manifest is both a package and the workspace root, so cargo on its own
    # runs `gallery`'s tests alone and never reaches `gallery-build` or `gallery-macros`.
    exec cargo llvm-cov --no-report nextest --workspace "$@"
  '';
}

# `gallery-test`: the test run, with a GL stack attached.
#
# The glow capture tests need an EGL device, and a CI runner has neither a GPU nor a system EGL. Mesa
# supplies both — a software device through `EGL_MESA_device_software`, which is what makes a headless
# OpenGL context possible with no display server at all.
#
# The environment rides on this wrapper rather than the dev shell on purpose: `__EGL_VENDOR_LIBRARY_-
# FILENAMES` *replaces* the driver list, so setting it shell-wide would take the machine's own GPU away
# from `just run`. Here it reaches the tests and nothing else.
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
    exec cargo llvm-cov --no-report nextest "$@"
  '';
}

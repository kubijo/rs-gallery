# Changelog

Notable changes to `gallery`, newest first, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0,
so a minor release may carry a breaking change.

## [Unreleased]

## [0.1.0] - 2026-08-15

- **A `--hot` run says in the window what it is doing.** A chip in the scenes panel's corner follows the cycle —
  watching, changed, building with its elapsed, swapping, reloaded — and a failed build puts a red bar over the canvas
  that opens what cargo said, each message in the colour of its level. All of it was terminal-only before, and a failure
  was worse than hidden: cargo leaves the dylib alone, so the window went on drawing the last scenes that built,
  indistinguishable from an edit that changed nothing. The chip is painted into chrome rather than laid out over the
  canvas, so nothing it reports covers a scene or shifts a control as the words change width. A run without `--hot`
  draws none of it.
- **Gallery watches, builds and opens the scenes itself; `cargo watch` and `hot-lib-reloader` are gone.** A build
  gallery starts is one it can read as it happens, through cargo's documented `--message-format=json`; the watcher it
  used to spawn kept the cycle to itself and the terminal, and was an undeclared binary besides — missing, cargo exited
  101, the spawn reported success, and `--hot` silently ran cold. The opening is its own for a different reason: a
  library a scene has drawn from can never be closed. Its widget state is a `Box<dyn Any>` whose vtable lives in the
  library that boxed it, held by egui's `Memory` as long as its `Context` — so each rebuild is opened over the last and
  none unmapped, at a mapping per rebuild. `notify` and `libloading` were already in the tree under what they replace.
  Two deliberate differences: `.gitignore` goes unhonoured (a missed edit costs more than a spare build), and an edit
  landing mid-build queues rather than restarting it, so cargo is never killed part-written.
- **The shell's own components are scenes now** — `just shell-scenes` opens a gallery of gallery: the chip in every
  phase, the failure bar at each count, the knob widgets side by side, the tree, the actions log, the frame-cost meter,
  the icons, the carets and rails. States a run reaches one at a time, some only when something breaks. They live inside
  the crate behind a feature, because the chrome they pose is `pub(crate)` — the component itself, not a copy that
  drifts — and reach the shell through `Linked`, as a host that links its own scenes does; `mod shell_scenes` is the
  sibling of `mod scaffold_scenes`. It paid for itself at once: the build report had its 80%-of-window bound baked into
  its body, so it ignored the size its caller asked for.
- **A rebuild that compiled nothing is not shown as one.** Cargo marks every artifact it did not rebuild as fresh, and a
  build of nothing but those wrote no dylib — so the chip goes back to watching instead of waiting out a swap that never
  comes. It keeps a directory of generated files from reading as a rebuild every time one lands, and is what lets the
  filter afford to err towards rebuilding.
- **A scenes crate finds its own scenes.** `build.rs` reads `gallery.toml` itself now, so a bare `cargo build` compiles
  in the scenes a launcher run does. Before, the globs reached it only through `GALLERY_SCENE_GLOBS`, which only the
  launcher sets — so any other cargo invocation built a scene-less dylib that linked and passed. It is quiet by
  construction: the tell is an `unused extern crate` warning on the `extern crate self as …` line, and a CI job
  type-checking the scenes green-lit code it had never compiled unless it restated the globs in its runner. That
  restatement was the real cost — a second place to write the glob set down, and the one thing this arrangement exists
  to avoid.
- **`GALLERY_CONFIG` replaces `GALLERY_SCENE_GLOBS`**, and carries the config's path rather than the globs it declares.
  Both sides then read one file through one function ([`gallery_build::Config`](gallery-build/src/lib.rs), moved there
  from the launcher with `resolve_glob`), so they cannot arrive at two spellings of one glob — which cargo compares as
  strings, and rebuilds on. It is set only when the config is not the one `build.rs` finds beside `CARGO_MANIFEST_DIR`,
  because `rerun-if-env-changed` puts the variable in the build script's fingerprint: one that appears and disappears as
  you alternate a launcher run with a bare build rebuilds the dylib every time, whatever the globs come to.
  `GALLERY_SCENE_GLOBS` is still honoured where something sets it, so a consumer pinned to an older gallery keeps
  working.
- **The generated scene includes are written only when they differ.** The file is `include!`d, so rewriting it with
  identical bytes still recompiles the crate — a fresh mtime is all it takes.
- **`build.rs` watches the directories the scenes are in, not the tree they were found in.** Cargo takes a watched
  directory's newest descendant, so a directory the build writes into is one the build dirties itself: watch it and
  every build makes the next one stale. That is what the static prefix of each glob was — for scenes living beside their
  components it is a repo root, holding the crate's `target/` and its `.git`, and a `git status` between two builds was
  enough to rebuild the dylib. A scene's own directory is not that, `build_output` having already dropped the matches
  under a cache tag — except where `OUT_DIR` sits beneath it, which is the scaffold's own layout with `target/` beside
  the scenes, so that one goes unwatched too. The cost is notice that a scene was *added* to such a directory; every
  scene file is still watched one by one, so editing one still rebuilds, and a `CARGO_TARGET_DIR` outside the crate or
  scenes one directory down gets the notice back.
- **The launcher drops the variables cargo set for the shell itself** before shelling out — `CARGO_MANIFEST_DIR`, the
  `CARGO_PKG_*` set, `OUT_DIR` and the rest describing the running binary. Inherited, they reach every build script
  cargo then runs and land in its fingerprint: `ring` reads `CARGO_MANIFEST_DIR`, so it saw the variable present under a
  launcher run and absent under a bare `cargo build`, rebuilt on every alternation, and took `rustls`, `ureq`, `gallery`
  and the scenes dylib with it. This was the larger half of the rebuild-churn a consumer reported; the config path being
  stable is the other. `CARGO_HOME`, `CARGO_TARGET_DIR` and the other settings are the user's and stay.

## 2026-08-14

- **A scene's egui ids are scoped by the scene.** The canvas was handed the panel's `Ui` as it stood, so every id
  derived inside a scene came off the parent and the order the widgets were called in, and nothing said which scene was
  drawing. Two scenes reaching a widget at the same point in the same shape therefore derived the *same* id and shared
  whatever egui keeps under it — scroll offsets, folded stages, collapsing headers, any `Memory`-backed widget state.
  Knobs never broke the tie, since they draw in the Controls panel rather than on the canvas, so scenes differing only
  in their knobs still arrived at their first canvas widget together. `render_canvas` pushes `scene_key` now, which is
  the identity the selection and the per-scene target caches already run on: it comes off the module path and the
  scene's name, so a hot reload that renumbers everything else leaves it alone, and both the window and the headless
  renderer go through that one function, so a capture and a window cannot disagree about it. The knob store is untouched
  — knob values are keyed by label in gallery's own state, never by an egui id.
- **A capture no longer depends on what was shot before it.** Each shot already gets a harness of its own, so egui's
  memory cannot cross between them; a scene's own cache is another matter, living in the scenes dylib that outlives
  every harness. A consumer keying one render target per stage by the id it derived found two scenes sharing an entry,
  and the retained state inside it, so the same scene at the same knobs captured differently depending on whether its
  neighbour had been shot first in that process — deterministic, and wrong. Scoping the canvas is the fix rather than
  keying that one cache, because the cache is only what noticed: everything else derived from a scene's ids was already
  crossing between scenes in a window, where the context is shared for the session and nothing gets a fresh one.
- **The canvas scroll area is inside the scope**, so each scene keeps its own place in the canvas rather than inheriting
  wherever the last one was left. Switching scenes used to land you partway down a scene you had just opened; coming
  back to one now returns you to where you were, as its knobs already did.

## 2026-08-13

- **`SceneCtx::matrix_with`** — the matrix's columns with the staging left to the caller: the callback takes
  `(&mut SceneCtx, &mut Ui, usize)`, so a cell can reach a knob, `texture_stage`, or a staging method a host has added
  to `SceneCtx` itself. `matrix` holds `&mut self` and stages every cell for you, which leaves its callback the only
  borrow there is — so a host that renders its widgets elsewhere and hands gallery the texture, staging through methods
  of its own, could not use the one reflow layout gallery ships, and its variants could only stack. `matrix` is this
  with a `stage` per cell now, so the column arithmetic exists once and no downstream reimplements it; its signature,
  its layout, and the order it claims stages in are all unchanged. Passed over: publishing the column count alone hands
  out the number but not the layout, leaving every caller to rebuild the grid and the row breaks; and a `matrix` generic
  over a staging strategy puts a type parameter on the only reflow layout there is, when the closure already is the
  strategy. The scaffold's `matrix.scene.rs` gains a scene giving each breakpoint a copy length of its own, so one size
  can be pushed past what it holds while the rest stay put.

## 2026-08-11

- **Sheets pack denser, on `binpack2d` rather than `rectangle-pack`.** A capture whose panels' aspects spread wide — a
  narrow column two and a half times the height of the rest, beside a banner twice their width — put every panel in one
  row and left a third of the page empty. That is the shortest sheet there is, since nothing stands taller than the
  tallest panel, and the score had no reason to look further: weighting area by how far the sheet sits off a screen's
  proportions cancels, because `area × (TARGET / ratio)` is `TARGET × height²`, so a tall sheet's width and all the
  slack along it came free. Reaching a better arrangement took more than scoring, though — rectangle-pack splits free
  space guillotine-style and never puts it back together, so a hole wide enough for a panel stops being one. The score
  divides by the share its panels cover now, and MaxRects keeps whole free rectangles to seat them in, every placement
  rule it offers tried and scored. The capture above packs to 84% on a sheet 876 px narrower.
  `tests/snapshots/sheet.png` is repacked to match.
- **egui 0.36.** `egui`, `eframe`, `egui_extras` and `egui_kittest` move together to 0.36.1, and a host has to move with
  them: only one egui is ever linked, so gallery's and the host's must be the same one. The MSRV is 1.95 now. Nothing in
  the shell changed shape — the reference images match what 0.35 rendered, byte for byte.
- **`gallery::egui`** — egui is re-exported, and a scaffolded instance no longer declares it: scenes reach
  `egui::Color32` through the prelude. Two egui versions in one build is the failure the rule above describes, and a
  manifest that never names egui cannot cause it. Existing instances can drop the dependency.
- **`gallery::egui_extras`, behind forwarded features** — `extras-svg` and `extras-image` turn on the loaders
  `install_image_loaders` installs, so an instance registers them without declaring egui_extras itself. Declaring it was
  the same seam as declaring egui, one crate further out: an instance on egui_extras 0.35 pulls egui 0.35 back in.
  Forgetting the feature is worth knowing about — `install_image_loaders` is not itself gated, so the call compiles and
  silently installs nothing. `extras-svg` costs a second SVG stack, resvg pinning usvg 0.45 beside the 0.48 the sidebar
  icons tessellate through; gallery's own use of egui_extras is `syntax_highlighting`, which needs no feature at all.
- **Texture uploads follow epaint's new contract.** `TestRenderer::handle_delta` takes the delta by `&mut`,
  `TexturesDelta::set` groups several `ImageDelta`s under one texture, and `Drop` asserts the delta was consumed. The
  glow capture drains the uploads and discards the frees: it runs before the paint, where `egui_glow` frees only after
  one, so acting on them there would take textures out from under output that still names them.
- **`usvg` 0.48 and `ureq` 3.** A response body is read through `body_mut().read_to_string()` now. `glow` stays at 0.16
  — it mirrors the glow the femtovg demo pulls, so it moves when femtovg does rather than tracking the latest.

## 2026-08-07

- **A stage's padding is settable** — `Stage::Fit.padding(0)`, or on a spec that already carries other settings,
  `Stage::Fill.scrollable().padding(4)`. The margin a rectangle hides behind its corners is all that shows around a
  round component, where it reads as a bezel. The alternative — staging the face at `diameter + 2×padding` and masking
  the difference — fakes the geometry and leaves the size badge reporting something the component is not. Clamped at
  zero: a negative margin would put content outside the checkerboard and hand a scrolling stage a viewport bigger than
  its own box. `PADDING` is public now, as the default it always was.
- **`SceneCtx::texture_stage`** — puts a texture the scene owns on a stage, with the chrome
  [`offscreen_stage`](src/context.rs) gives one gallery owns, and no copy: nothing crosses into a framebuffer of
  gallery's. `StageTexture::new(id, allocated)` names the texture, `.showing(size)` says how much of it to draw and
  `.interactive()` asks for the pointer; it hands back an [`ImageInput`](src/offscreen.rs) carrying the `Response` and
  the events in the image's own pixels, so content that hit-tests itself can take this path rather than staying on
  `offscreen` for the input alone. Any `TextureId` works, including one from `egui::Context::load_texture`, so this is
  not glow-only — only making one out of a GL name is.
- **A stage told to show nothing says so** — `showing([w, 0])` draws a named hint in place of the image. The bug that
  lands there is measuring a layout node that reports no extent, and a one-pixel sliver under a `×0` caption reads as
  gallery having lost the texture rather than as the measurement having failed.
- **Gallery owns the V flip and the crop.** GL textures are bottom-left origin while a stage reads top-down, so an
  adopted texture is drawn flipped, and `showing` has to crop the end the flip put the content on. Derived at a call
  site from an upside-down render, that comes out backwards — keeping the slack and cutting the content. `showing` also
  removes the auto-height round trip: allocate loosely once, render, and show the height just measured, in one frame,
  where sizing a gallery-owned target means render, measure, repaint, resize, with a frame at the wrong size in between.
  Allocating once also keeps the texture from being reallocated, which is what would leak a `TextureId`.
- **`report`** — a recipe's `report = "capture.json"` writes what the run came to beside the images: per shot its name,
  path, size, byte count, whether it `settled` and how many frames it drew, plus the sheet's path when one was gathered.
  Its own serialisable types rather than the internals, so the file is a stated format. For an unattended loop, which
  needs to tell a settled capture from one the frame ceiling landed on without reading English. It says what did *not*
  happen too — `complete`, `requested`, `failed` and `warnings` sit alongside the shots, because a reader counting
  records alone cannot tell a recipe of three from one of ten that stopped at three. A run that fails partway still
  writes it, listing what landed and what stopped the rest; a sheet asked for and skipped leaves a warning rather than a
  path. The file is removed before the first shot rather than overwritten after the last, so a run that dies leaves no
  report instead of the previous one, which a loop would take for this run's.
- **A `frames` under 2 is refused** rather than quietly raised. One frame declares a scene's knobs and the next applies
  the recipe over them, so a single frame captures the scene's own defaults whatever the recipe says — the wrong picture
  rather than a rougher one. A recipe and `--frames` are both checked; the windowed profiling run, where any count means
  something, is not.
- **`settle`** — a recipe (or a single shot) can say `settle = true`, and each scene is then captured as soon as it
  stops asking egui to redraw it, with `frames` becoming the most to draw rather than the number drawn. It replaces
  tuning a frame count by hand, where too few catches a scene mid-animation and too many makes every settled scene in
  the set wait for the slowest. A scene that animates forever is captured at the ceiling and reported **still moving**,
  so an unattended run neither hangs nor silently diffs one arbitrary frame against another. Two consecutive quiet
  frames are required, since the signal is the egui context's rather than the scene's and a one-shot repaint from
  anything on the canvas would otherwise end the wait early.
- **The offscreen colour space is written down** — the target is `SRGB8_ALPHA8` and its sampler decodes, so bytes
  written straight in come back one decode darker: mid-grey `128` reads as `54`. A 2D library handing over sRGB-encoded
  bytes (femtovg, cairo, skia) therefore looks right in its own preview and crushed here, which cost a consumer real
  time to work out from nothing. Drawing under `FRAMEBUFFER_SRGB` cancels it, to within a least-significant bit — the
  encode and the decode round independently, and a software rasteriser and a GPU disagree by one. Both halves are on
  [`Offscreen`](src/offscreen.rs) and pinned by a round-trip test; no behaviour changed.
- Captures are pinned as reproducible: two runs of one recipe write the same bytes, over a scene that reads the frame
  clock, so a capture's frame times staying fixed rather than following the wall clock is now a test rather than a happy
  accident.

## 2026-08-06

- **Sidebar folding** — `Settings::collapsed(..)` says which folders a gallery opens with folded: `true` for all of
  them, or a list of top-level names (compared without case) for those and no others. A starting state rather than a
  fixed one — opening a folder keeps it open for the session, and a filter still reaches into one that started folded. A
  name matching no root folds nothing, as does one naming a title of a single segment, which the sidebar shows as a
  scene rather than a folder.
- **Fixed** — only the first shot of a `--capture` run drew, for any scene holding a GL object between frames. Each shot
  built its own EGL context and dropped it with its harness, while the scene's cache lives in a thread-local in the
  scenes dylib that outlives every harness — so from the second shot on a scene painted with textures and framebuffers
  belonging to a context that was gone, and the image came back empty. The context is opened once now and lent to every
  shot, including the sheet. Scenes need no change, and could not have worked around it: a fresh context reissues GL
  names from 1 and a freed handle's address is commonly reused, so neither a name nor a pointer tells a scene its
  context has been replaced.
- **Breaking: a scene takes its `Ui` as an argument** — `fn(&mut SceneCtx)` becomes `fn(&mut SceneCtx, &mut Ui)`,
  `ctx.ui` is gone, and the five methods that draw take the `Ui` to draw into: `stage`, `offscreen`, `offscreen_input`,
  `offscreen_stage`, `offscreen_input_stage`. `stage!` gains a second argument, `stage!(ctx, ui, ..)`. Migrating a scene
  is three mechanical edits: add `ui: &mut Ui` to the signature, drop the `ctx.` from every `ctx.ui.…`, and pass `ui` to
  the calls above. **The knob API is untouched** — no accessor or `set_*` ever drew anything, so
  `ctx.slider("count", ..)` and friends are unchanged, as is `gallery::action`.
- **Stages go wherever widgets go** — which is the point of the above. A stage sits in `egui::Grid`, `ui.columns(..)`, a
  `Frame`, a `CollapsingHeader` or a `ScrollArea`, and anything egui adds later works without gallery knowing about it.
  One limit worth stating: a stage does **not** wrap in `ui.horizontal_wrapped(..)`. egui breaks a row on a size the
  widget declares before it draws, and a stage's size isn't known until its content has drawn, so a row of them runs off
  the pane rather than wrapping. Use `matrix` below.
- **`SceneCtx::matrix`** — one stage per size, in as many columns as the pane holds, aligned in a grid; the column count
  is measured against the widest of them, so resizing the window reflows the set (CSS's `repeat(auto-fit, ..)`). A grid
  rather than a wrapping row because the point is comparison — columns line up, where a row of differing sizes leaves
  every cell at its own offset. Nothing is packed: the order given is the order shown, a size matrix reading smallest to
  largest. The scaffold gains `matrix.scene.rs`.
- **Fixed** — a stage put the badge row and its content into the parent layout as two separate items, so in any
  horizontal layout the collapse arrow and the size caption landed beside the component instead of above it. A stage is
  one block now, whatever it is placed in. Invisible before this release, since the canvas only ever stacked them.
- **`Ui` joins the prelude**, since every scene signature names it.
- **Knob writeback** — `SceneCtx::set_slider`, `set_toggle`, `set_text`, `set_color`, `set_select`, `set_select_index`
  and `set_pad2d` write a knob back from the scene, for rendered content that does its own hit-testing. Each takes the
  first knob of its kind carrying exactly that label and returns whether one matched — an unknown label writes nothing
  and creates nothing. Values are clamped as the panel would clamp them and snapped to a slider's `step`, where a
  recipe's out-of-range value is still an error: a write overshoots by nature, a recipe states its values on purpose. A
  write lands where a panel edit would, so anything that resets a knob — the scene dropping it, or changing its label or
  kind at that position — drops the write too.
- **Capture precedence** — a recipe's overrides are re-applied before every frame and once more after the last, so a
  scene writing its own knobs cannot outlast them: the image and the store both end on the recipe, and `--list-knobs`
  and `--init-capture` therefore report what was captured. Two consequences for recipes that predate this. A key is now
  resolved afresh each frame, so a regex that only turns ambiguous once another override reveals a second matching knob
  stops the run rather than slipping through on the frame it happened to be unique. A key whose knob applied and then
  disappeared still counts as honoured, and does not fail the run.
- **Offscreen input** — `SceneCtx::offscreen_input` shows an offscreen image as `offscreen` does and reports the pointer
  that landed on it as `Pointer::{Down, Move, Up, Wheel}`, in that image's own pixels: a press captures until its
  release, so a drag off the edge keeps reporting and never leaves the content held; coordinates come off the whole
  image, so scrolling part of it out of sight doesn't shift them; and the image takes the drag and the wheel, so neither
  reaches the canvas behind it. Whatever egui reports as a pointer arrives, so a touchscreen drives it too.
- **Actions** — `gallery::action(..)` reports a line into a panel of its own, timestamped and scoped to the scene, with
  a toggle beside Perf and a Clear button. A free function, so it reaches into a callback handed to a component: the
  component keeps its own event type and never names gallery. Heard only while a scene renders, so a headless capture
  and a spawned thread both drop it.
- **Prelude** — `action` and `Pointer` join it, and the scaffold's `knobs.scene.rs` gains a `writeback` scene exercising
  all seven setters and reporting each write as an action.
- **Staged offscreen** — `SceneCtx::offscreen_stage` and `offscreen_input_stage` put a rendered frame on a stage, so GL
  content gets the checkerboard, size caption and collapse toggle egui content has always had; a scene mixing prose,
  widgets and rendered frames now reads as one document. A folded stage runs no GL at all and returns `None`, and keeps
  its target for when it opens again. `stage` itself is unchanged — the image is drawn to its texture first, leaving
  only an image for the stage to hold.
- **Fixed** — every `offscreen` call in a scene shared one framebuffer, keyed by scene alone, so each image showed
  whatever the last call drew, stretched to the size its own call had asked for — and two sizes reallocated the target
  twice a frame. Each call site now keeps its own, told apart by the order the scene makes them, as knobs are. A scene
  that stops making a call keeps that target for its return rather than freeing a `TextureId` eframe cannot release.
- **Fixed** — a capture asked for less room than its scene lays out is drawn twice, and the second pass used to run on a
  rebuilt GL context. A scene that had cached anything against the first — a femtovg canvas, a compiled shader — drew
  into nothing, so the shot came back with the egui parts and none of the GL. The canvas is resized in place now, and
  the context lives as long as the shot.

## 2026-07-31

- **Trimmed captures** — a shot's size is the canvas its scene lays out in, and the PNG is now cropped to what the scene
  actually drew, rather than padded out to that size with background. A recipe's `trim = false`, or `--no-trim`, keeps
  the whole canvas. This narrows what "Headless render" below says about the size being where the scene lays out: that
  is still what governs the layout, but no longer what the file ends up measuring.

## 2026-07-30

- **Fixed** — the canvas scrollbar sat at the right edge of the content rather than of the pane, leaving dead space
  beside it whenever a scene was narrower than the pane and taller than it.

## 2026-07-20

First public pre-release — it all lands in the initial commit, so there's nothing to migrate from yet; this log records
changes from the first tagged release on. Highlights (the [README](README.md) has the detail):

- **Discovery** — `#[scene]` / `scene_meta!` self-register via `inventory`; `build.rs` globs `*.scene.rs` from config.
  Scenes reach the shell `Linked` (compiled in) or via `HotDylib` (rebuilt and hot-swapped on `--hot`). A `**` glob from
  a crate root walks into `target/`, so a match found there is skipped as build output, and an entry that goes missing
  mid-walk — cargo writes and removes temp files as it builds — is skipped rather than failing the build.
- **Controls** — `text`, `slider`, `toggle`, `color`, `select` / `radio` / `buttons`, `pad2d`, `group`;
  declarative-by-use, with values persisted per scene across reloads.
- **Scenes read as documents** — the canvas is plain, so headings and prose drawn onto `ctx.ui` look like headings and
  prose. Each thing being demonstrated goes in a `stage!`, which carries the checkerboard, a size caption and a collapse
  toggle: a bare closure fits the content, `fill` takes the rest of the canvas, and anything else is a size —
  `(300.0, 200.0)`, `(300, 200)` or `200` for a square all convert, so a call site writes the dimensions the way it
  already holds them. A scene with several stages and prose between them is the point; a scene that is a single `fill`
  is the whole-canvas shape every scene had before stages, said out loud.
- **Shell** — tree sidebar with fuzzy filter and keyboard nav, Preview/Source and Debug toggles, collapsible panels, and
  mesh-tessellated SVG icons.
- **Performance window** — frame cost and p95 in a separate viewport on its own repaint clock, so watching the numbers
  never drives the loop being measured. Reports the cost of building a frame, not the interval between frames, and holds
  still when the shell is idle rather than manufacturing traffic to look live.
- **Profiling** — `--frames <n>`, optionally with `--scene <key>`, renders a fixed count and exits, so two recordings
  are comparable rather than however long you happened to sit there. `just profile <scene>` records one under samply
  into `reports/`, resolving its addresses to symbols on the spot — samply defers that to view time, and by the next
  build the binaries the addresses point into are gone. `gallery-perf analyze` (the uv package in `tools/`) then splits
  self time per crate — separating gallery's own cost from the component's — over the samples that were drawing rather
  than parked on the event loop, reporting how many that excluded.
- **Headless render** — `--render <path.png> --scene <pattern>` writes a scene's canvas with no window, so an agent or
  CI can look at a component instead of asking someone for a screenshot. Drawn by the same code the shell draws with, so
  the image is the canvas alone and holds still as the chrome around it changes. The size is where the scene lays itself
  out rather than a crop of it: one that comes out bigger is captured whole, at the size it turned out to need.
- **Sheets** — a recipe's root-level `sheet = "sheet.png"` also gathers that run's captures onto one captioned image, so
  a change across a whole set is one thing to look at rather than a directory to click through. `rectangle-pack` places
  the panels; the sheet size is searched over candidate widths and scored on area *and* proportions, since a column and
  a strip cover the same page and both leave every panel unreadable once scaled to a screen. The shots still write their
  own PNGs. One capture writes no sheet: a sheet of one image is the image.
- **Capture recipes** — the defaults are rarely the state worth seeing, so `--capture <file.toml>` renders a list of
  shots, each naming its scene, size and knobs. A file rather than flags: labels contain spaces that shell wrappers
  drop, and a set of states is worth committing. Knobs go by label (or a regex over them), choices by option label —
  both survive a reordering. Values are applied between frames rather than seeded, since a knob doesn't exist until the
  scene asks for it. Anything that can't be honoured stops the run: a clean render of the wrong state is worse than
  none.
- **Recipe generator** — `--init-capture` prints a recipe with every knob at the value its scene declared, so the
  starting point already renders and the first edit is the state you were after.
- **Capture follows the renderer** — under `Renderer::Glow` it paints through an OpenGL context taken off an EGL device,
  with no window and no display server, so a scene drawing with `ctx.offscreen(...)` captures its content rather than
  the hint wgpu would leave in its place. egui_kittest ships no glow renderer, so this is one of gallery's own; the
  texture a scene registers goes through `egui_glow`'s painter, since the `Frame` a test harness builds carries no glow
  hook and nothing outside eframe can give it one.
- **Reference images** — captures are diffed against committed PNGs, so a change to stage sizing, the bundled fallback
  faces, a knob override or the offscreen GL path fails a test instead of going unnoticed. They come from an ordinary
  capture recipe (`tests/reference.toml`), which makes the fixture exercise what a caller writes rather than the structs
  it parses into. `UPDATE_SNAPSHOTS=1` takes an intended change and keeps the old image beside it. Comparable between
  machines only because the tests pin llvmpipe from the pinned mesa — a GPU antialiases the same scene its own way.
- **Command line** — parsed by clap, so `--help` lists the arguments and a bad one is an error, not a panic. Failures
  that come down to a choice name the candidates in a framed list, styled through `anstream` so a pipe or `NO_COLOR`
  gets the same text plain.
- **Host overrides** — `Settings` (e.g. the Controls-panel width) and `apply_default_style`, layered under the host's
  `setup` closure.
- **Fonts** — bundled Noto fallback faces (Sans, Symbols, Symbols 2, Math; SIL OFL 1.1, in `fonts/noto/`) fill the
  arrow/math/symbol glyphs egui's defaults render as tofu. Appended to each family's fallback chain, so the default look
  is unchanged; CJK/emoji stay out (add per-consumer via `setup`).
- **Renderer** — `Settings::new(Renderer)` picks the eframe backend (`Wgpu` or `Glow`), a required choice with no
  default. Under `Glow`, a scene renders non-egui content into an offscreen framebuffer with `ctx.offscreen(...)` — or
  the raw `ctx.gl_loader()` / `ctx.register_native_texture(...)` beneath it — at its own femtovg/glow version, which
  gallery never pins. `just demo-wgpu` and `just demo-femtovg` run the two backends; the femtovg demo exercises the
  offscreen path.
- **Scaffolding** — `cargo generate … template --name <dir> --no-workspace` lays down a standalone instance crate (its
  own `[workspace]` plus a `justfile` with `just run` / `just hot` / `just update` / `just render` / `just capture` /
  `just knobs` / `just capture-init`), carrying example and knob scenes plus an animated one that drives the render loop
  for the performance window to measure, and a `capture.toml` showing the recipe format against those scenes.
- **Update check** — `just update` (`cargo run -- --check-updates`) fetches the upstream CHANGELOG over HTTPS and prints
  what's changed since the `gallery` version you're building against.
- **Docs formatting** — code fences in Markdown are formatted as code rather than left as prose: `just format` sends
  Rust fences through the rustfmt this repo already pins, Bash fences through beautysh and TOML fences through taplo, so
  the README's examples carry the same style as the source they mirror. taplo formats the `.toml` files too — bar
  `Cargo.toml`, which cargo itself writes.

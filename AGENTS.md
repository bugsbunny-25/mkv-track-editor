# Agent instructions

Rust workspace: a Slint UI (`crates/mkv-ui`) shared by a desktop app
(`mkv-desktop`) and a wasm web app (`mkv-web`, served by `mkv-server`).
See README.md for layout and usage. These notes cover build cost, which is
easy to get wrong here.

## Why builds are heavy

- `slint-build` compiles `crates/mkv-ui/ui/**/*.slint` into one generated
  Rust file (~10 MB). rustc needs ~2 GB of RAM for `mkv-ui` alone; it is
  the slowest crate in every build.
- Slint inlines a component at every use site in some cases, which
  multiplies the generated code (it once reached 23 MB and 4.2 GB of rustc
  RAM). Known triggers: `opacity` on a component's root, and layout
  properties such as `padding` set where a layout component is used. Keep
  those on an inner wrapper element instead. After UI changes, check the
  size of `target/*/build/mkv-ui-*/out/app_window.rs` did not jump.

## Local container builds (Apple `container` CLI, no Docker on this Mac)

- Never run `container build` without `-c`/`-m`. The builder VM defaults to
  2 CPUs / 2 GB, less than rustc needs: it swaps nonstop (fans, heat,
  terabytes of disk I/O) until rustc is killed. Use:
  `container build -c 8 -m 8g -f docker/Dockerfile -t mkv-track-editor .`
- `-c`/`-m` only size a builder that the build starts itself, and a build
  without them recreates the builder at 2 GB. If `container builder status`
  shows another size, run `container builder stop; container builder delete`
  first.
- Build only the host's architecture (arm64). Never pass `--arch amd64` or
  `--platform linux/amd64` locally: the builder has no Rosetta, so the whole
  compile runs under QEMU emulation. CI builds amd64 natively on x86_64
  runners and arm64 on arm64 runners.
- Stop the builder when done (`container builder stop`); it holds its RAM
  while running. Don't delete images or containers you did not create
  unless asked.

## Dockerfile (`docker/Dockerfile`)

- Keep the wasm stage on `--platform=$BUILDPLATFORM`; the wasm bundle is
  architecture-independent and must never build under emulation.
- Don't `cargo install` tools (wasm-pack, wasm-bindgen-cli) in the image:
  that recompiles hundreds of crates on every cold cache. The wasm-bindgen
  CLI is the prebuilt musl release matching the version in `Cargo.lock`
  (checksum-verified); keep it in sync with the lockfile, not pinned.
- Keep `target/` in `.dockerignore`; the local one is tens of GB.
- Never share a cargo registry cache mount (`/usr/local/cargo/registry`)
  between stages that build in parallel: cargo's package lock is outside
  the mount, so both unpack the same crate at once and one fails with
  "failed to unpack package ... File exists". Give each stage (and each
  platform) its own `id=`.
- Pull request CI does not build the image; only `main` does (releases
  retag `main`'s image). Build it locally (see above) before merging Dockerfile changes.

## Cargo builds and `target/`

- Build only what you need: `cargo check -p <crate>`, `cargo build -p
  mkv-desktop`, `cargo test -p <crate>`. Avoid repeated full-workspace
  release builds.
- Each distinct Slint version/feature set/command (`-p mkv-desktop`,
  `test --workspace`, wasm, release) keeps its own copy of `mkv-ui` in
  `target/`, and Cargo never removes old ones; debug info for the generated
  code is hundreds of MB per copy. `target/` reaching 40–60 GB is expected
  over time: `cargo clean` fixes it (next debug build ~25 s). Only run it
  when the user agrees.
- Slint features are trimmed per crate on purpose (see the comments in each
  `Cargo.toml`); don't turn Slint's default features back on.
- The web build without Docker: `cd crates/mkv-web && wasm-pack build
  --release --target web`.

## Releases and Homebrew

- Releases are cut by running the Release workflow (`release.yml`) by hand
  with the version: `gh workflow run release.yml -f version=X.Y.Z`. It
  refuses unless `main`'s latest CI run, including both images, is green,
  and `X.Y.Z` is the next patch/minor/major after the highest `vX.Y.Z` tag.
  It bumps `Cargo.toml` on `main` itself (`[skip ci]`) when needed, so don't
  bump by hand or push `release/*` branches; `ci.yml` no longer runs for
  them. It builds nothing: it retags `main`'s `:sha-<commit>` image and
  repackages that run's desktop archives with `package-desktop.sh`, so keep
  `release.yml`'s platform list in step with `ci.yml`'s desktop matrix.
  Re-run a failed release's failed jobs rather than starting a new one.
- A release run creates a GitHub release here (publishing it creates the
  `vX.Y.Z` tag), then publishes the archives to the tap repo with a
  **cask** for macOS (`packaging/homebrew/render-cask.sh`, installs the app
  into /Applications) and a **Linux-only formula** (`render-formula.sh`).
- The macOS bundle must be signed as a whole (`codesign --sign -` in
  `packaging/package-desktop.sh`, after the Info.plist and icon are in
  place). The linker's signature covers only the binary, and macOS then
  refuses to open the app from Finder. It's ad hoc, not notarized: users
  click "Open Anyway" once per install. Don't strip the quarantine flag in
  the cask.
- The Linux formula patches the binary's interpreter and RPATH at install
  time with Homebrew's patchelf.rb. patchelf.rb 1.6.2 (on stale Homebrew
  installs, like the x86_64 runner image) crashes adding an RPATH to the
  lld-linked x86_64 binary (`Encoding::CompatibilityError`); 1.7.0 fixed
  it. The install check therefore runs `brew update` first — keep it.

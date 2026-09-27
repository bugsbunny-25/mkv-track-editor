# MKV Track Editor

Open MKV files (one tab each), inspect their full media info, and let
**profiles** decide which video/audio/subtitle tracks to keep, in what
order, with what names and Matroska flags — then save with a
**Radarr/Sonarr-compatible file name**. Nothing is ever re-encoded.

Ships as two products sharing one Slint UI and one core Rust crate:

- **Desktop app** — native Slint UI, native file dialogs, runs the tools locally.
- **Docker web app** — the same UI compiled to WebAssembly, served by a Rust
  HTTP server that browses a mounted volume and runs the tools in the container.

All container manipulation shells out to [MKVToolNix](https://mkvtoolnix.download/)
(`mkvmerge` for reorder/removal, `mkvpropedit` for a fast in-place path when
only names/languages/flags change). Analysis uses
[MediaInfo](https://mediaarea.net/MediaInfo) (Dolby Vision profile and
fallback, HDR10+, HLG, Atmos, DTS:X, channel layouts, subtitle event counts).

## Features

- **Tabs** — open many files; each tab shows a status dot (pending changes,
  saving %, saved, failed). "Apply to all" / "Save all" for batches.
- **Drag and drop** (desktop) — drop MKV files onto the window to open them.
  The window shows whether a drag will be accepted; other files (MP4, MOV,
  images, …) are skipped with a message, since only Matroska can be edited.
- **Tracks** — keep/drop (dropped tracks stay visible, dimmed), reorder within
  a type, rename, language, and flag toggles: **D**efault, **F**orced,
  **O**riginal language, **HI** (hearing impaired), **AD** (audio
  description), **C**ommentary.
- **Media Info** — MediaInfo's full report by section, with a filter that
  searches every section.
- **Naming** — title/year/TMDb ID/edition/source/group/season/episode,
  prefilled from the release name, with a live preview of the output name.
- **Settings** — edit profiles and naming formats with validation and a live
  preview against the open file; stored in `config.toml`.

## Profiles

A profile has video, audio and subtitle sections. Track names use the same
`{Token}` templates as file names. The defaults:

| | Name format | Example |
|---|---|---|
| Video | `{Resolution} {DynamicRange} {Codec}` | `2160p DV8 HDR10 HEVC`, `1080p SDR AVC` |
| Audio | `{Language} {Format} {Channels}{ Atmos}{ Commentary}{ Description}` | `English DD+ 5.1 Atmos`, `Tamil Dolby TrueHD 7.1 Atmos` |
| Subtitles | `{Language}{ Kind}` | `English`, `English SDH`, `English Forced` |

**Dynamic range** includes the Dolby Vision profile and its fallback layer:
`SDR`, `HDR10`, `HDR10+`, `HLG`, `DV5`, `DV7 HDR10`, `DV8 HDR10`,
`DV8 HDR10+`, `DV8 HLG`, `DV8 SDR`. **Audio formats**: `DD`, `DD+`,
`Dolby TrueHD`, `DTS`, `DTS-ES`, `DTS-HD HRA`, `DTS-HD MA`, `DTS:X`,
`DTS Express`, `AAC`, `FLAC`, `PCM`, `Opus`, `MP3`, `AC-4`, …; ` Atmos` is
appended for Dolby Atmos (TrueHD or DD+ JOC).

**Audio rules** pick languages by the file's original language (chosen per
tab, pre-guessed from the existing "original" flag or first audio track).
First matching rule wins; `{original}` is the picked language; `keep` is also
the output order:

```
en, hi, te => {original}              # English/Hindi/Telugu films: only that language
ta, ml, kn => te, {original}, hi      # Tamil/Malayalam/Kannada: Telugu (default), original, Hindi
*          => {original}, en          # everything else: original + English
```

The first kept main track is the default; tracks in the original language
get the Original flag. Commentary and audio-description tracks in kept
languages are kept, named and flagged (never default). If a rule would keep
no audio at all, every audio track is kept and a warning is shown.

**Subtitles** keep English only by default. Each track is classified as
Forced, Full, SDH, CC, Signs & Songs or Commentary — from its existing flags,
then its title, then (for forced subtitles) an event-count heuristic. SDH/CC
set the hearing-impaired flag, Forced sets forced. For foreign-language
films the English Forced track is also made default.

## File naming (Radarr/Sonarr compatible)

Defaults (both editable in Settings):

```
Movie:   {Movie CleanTitle} {(Release Year)} [tmdbid-{TmdbId}] - {Edition Tags }{[Custom Formats]}{[Quality Full]}{[MediaInfo 3D]}{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[Mediainfo VideoCodec]}{-Release Group}
Episode: {Series TitleYear} - S{season:00}E{episode:00} - {Episode CleanTitle} [{Custom Formats }{Quality Full}]{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[MediaInfo VideoCodec]}{-Release Group}
```

→ `Vikram (2022) [tmdbid-689109] - [Remux-2160p][DV HDR10][TrueHD Atmos 7.1][x265]-GROUP.mkv`
→ `The Office US - S03E10 - The Convict [WEBDL-720p][EAC3 5.1][x264]-NTb.mkv`

Grammar follows Radarr's: `{prefix Token suffix}` only emits the prefix/suffix
when the token has a value; `.`/`_`/`-` between token words replace spaces in
the value (`{Movie.CleanTitle}`); ALL CAPS / lowercase token names change the
value's case; `:00` zero-pads. Additionally, a literal `[...]`/`(...)`/`{...}`
group whose tokens are **all empty** is dropped — so `[tmdbid-{TmdbId}]`
disappears when no TMDb ID is entered. Colons are replaced per the colon
setting and other illegal characters Radarr-style.

`{MediaInfo ...}` tokens use Radarr's exact vocabulary (`DV HDR10`,
`HDR10Plus`, `EAC3 Atmos`, `DTS-HD MA`, `x265`/`h265`, …) and reflect the
tracks you **keep** (the first kept audio track is the "primary" one), so
names stay parseable by Radarr/Sonarr. Custom formats are `Name = regex` rules
matched against the source file name.

## Workspace layout

```
crates/
  mkv-types/     shared data model — no I/O, compiles to wasm too
  mkv-profiles/  labels, profile rules, release-name parser, naming templates — pure, compiles to wasm
  mkv-core/      mkvmerge/mkvpropedit/mediainfo execution, command construction, config file store — native
  mkv-volume/    path-traversal-safe volume browsing — server only
  mkv-ui/        shared .slint UI + Backend trait + view-model — native and wasm
  mkv-desktop/   native binary (rfd dialogs + mkv-core in-process)
  mkv-server/    axum binary (JSON/SSE API + serves the wasm bundle)
  mkv-web/       wasm binary (talks to mkv-server over HTTP/SSE)
docker/
  Dockerfile (Alpine, multi-arch), docker-compose.yml
packaging/
  package-desktop.sh           release archives (macOS .app bundle, Linux binary)
  homebrew/render-formula.sh   the Homebrew tap formula, rendered by CI
.github/workflows/ci.yml       tests, builds, images, releases
```

### UI design system

The UI uses a **Playful Geometric** design language: clean content on warm
paper, chunky 2px ink outlines, hard offset "sticker" shadows, pill
controls, a dot-grid canvas, and violet / pink / yellow / mint confetti
accents, with bouncy motion. Headings use Outfit and body text uses
Plus Jakarta Sans. Both fonts are bundled.

`crates/mkv-ui/ui/` is layered so styling lives in one place:

```
theme/swatches.slint   primitive colour palette (the only raw colour values)
theme/theme.slint      semantic tokens: surfaces, ink, hard shadows, text,
                       accent, confetti, tones, spacing, radius, type scale,
                       breakpoints, sizes, motion — light ("paper") and dark
                       ("night")
theme/viewport.slint   responsive state: the window width and the sm / md /
                       lg / xl breakpoint flags
theme/icons.slint      icon registry (outline SVGs in icons/, tinted at
                       runtime) and tileable patterns (patterns/)
fonts/                 Outfit and Plus Jakarta Sans (variable, SIL OFL)
components/            Button, Badge, Toggle, SegmentedControl, TextField,
                       TextArea, Select, Card, Banner, AccentMark, FormField,
                       NavItem, NavHighlight, EmptyState, DotGrid, Confetti,
                       typography — import via components/index.slint
views/                 screens (app bar, document, tracks + inspector, media
                       info, naming, settings, library browser)
bridge.slint           the Rust ↔ UI contract
```

**Responsive.** Slint has no media queries, so the UI follows Slint's
documented approach: the window writes its width to the `Viewport` global,
and layouts switch arrangement with plain `if`s on its min-width flags. The
breakpoints are the standard scale (`sm` 640, `md` 768, `lg` 1024, `xl`
1280, defined once in `Theme`). From `lg` up, the document fills the window
with side-by-side panels. Below `lg` it becomes one scrolling page with
stacked panels, and headers, toolbars and forms restack below `md` / `sm`.
Tab strips, fact badges and library roots scroll sideways instead of
widening the window, so nothing overflows horizontally down to the 320px
minimum width.

**Accessible (WCAG 2.1 AA).** Text colours meet 4.5:1 and control outlines
meet 3:1 in both schemes. Every button, tab, toggle, list row and option
has a hit area of at least 44 × 44 (`Theme.touch-target`), even when the
visible pill is smaller. Everything is keyboard-operable with a visible
focus ring: Tab to move, Enter/Space to activate, ←/→ in the page
switcher, and ↑/↓ plus Space in the track list. Icon-only buttons carry
accessible names.

Views only use `Theme` tokens and `Tone` for colour (`Tone.video`,
`Tone.warning`, …), never raw colours. The colour scheme follows the system
and can be toggled from the app bar. Text inputs and pickers are our own
components rather than Slint's std-widgets, because the stock widgets draw
frames that can't follow the design.

## Prerequisites

- **Rust** (stable) via [rustup](https://rustup.rs).
- **MKVToolNix** (`mkvmerge`, `mkvpropedit`) and **MediaInfo CLI**
  (`mediainfo`, 23.04 or newer for HDR10+ detection) on `PATH`. Both apps
  check for them at startup and fail fast with a clear message.
  - macOS: `brew install mkvtoolnix media-info`
  - Debian/Ubuntu: `apt install mkvtoolnix mediainfo`
- For the web/Docker build: `rustup target add wasm32-unknown-unknown` and
  `cargo install wasm-pack --locked`.
- **Docker** (with Compose) to build/run the container.
- Linux desktop builds: `apt install pkg-config libfontconfig1-dev libxkbcommon-dev`.

## Desktop app

### Install with Homebrew (macOS Apple silicon, Linux x86_64/arm64)

```bash
brew install bugsbunny-25/tap/mkv-track-editor
```

This also installs MKVToolNix and MediaInfo. Run `mkv-track-editor` from a
terminal; on macOS the formula also installs `MKV Track Editor.app` (see
`brew info mkv-track-editor` to add it to `/Applications`). On Linux the
app uses Homebrew's X11/Wayland/Mesa libraries, which come with
MKVToolNix anyway, and file dialogs go through the desktop's
xdg-desktop-portal.

### From source

```bash
cargo run -p mkv-desktop
```

"+ Open" takes several files at once. "Save As…" suggests the generated name
next to the source; "Save all" asks for one output folder. Settings live in
`~/Library/Application Support/mkv-track-editor/config.toml` (macOS) or
`~/.config/mkv-track-editor/config.toml` (Linux); override the directory with
`MKV_CONFIG_DIR`.

## Web app

### Without Docker

```bash
cd crates/mkv-web && wasm-pack build --release --target web && cd ../..
mkdir -p /tmp/mkv-static && cp crates/mkv-web/index.html /tmp/mkv-static/ && cp -r crates/mkv-web/pkg /tmp/mkv-static/

MKV_LIBRARY_ROOTS=movies=/path/to/library \
MKV_STATIC_DIR=/tmp/mkv-static \
  cargo run -p mkv-server
```

Open `http://localhost:8080`.

### Prebuilt image

CI publishes a multi-arch (`linux/amd64`, `linux/arm64`) Alpine image to
`ghcr.io/bugsbunny-25/mkv-track-editor`: `:main` and `:sha-<commit>` from
`main`, `:X.Y.Z`, `:X.Y` and `:latest` from `release/X.Y.Z`. The package is
private like this repository, so `docker login ghcr.io` first. To use it with
Compose, replace the `build:` block in `docker-compose.yml` with
`image: ghcr.io/bugsbunny-25/mkv-track-editor:latest`.

### With Docker Compose

```bash
cd docker
mkdir -p library output config   # put source .mkv files in ./library (mounted read-only)
docker compose up --build
```

Open `http://localhost:8080`. Edited files go to `docker/output/`; settings
to `docker/config/config.toml`. The container runs as an unprivileged user —
if it can't write to `output/` or `config/`, uncomment `user: "UID:GID"` in
`docker-compose.yml` with your own IDs. Build context is the repository root
(`docker build -f docker/Dockerfile .`); Compose handles that.

## Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `MKV_LIBRARY_ROOTS` | *(required, server)* | Comma-separated `name=path` pairs, e.g. `movies=/data/movies,shows=/data/shows`. |
| `MKV_OUTPUT_ROOT` | `<first root>/_output` | Where the server writes edited files. |
| `MKV_CONFIG_DIR` | server: `config`; desktop: OS config dir | Directory holding `config.toml` (profiles + naming). Created with defaults if missing. |
| `MKV_STATIC_DIR` | `static` | Directory with `index.html` and the wasm `pkg/`. |
| `PORT` | `8080` | HTTP port. |
| `MKV_MKVMERGE_BIN` / `MKV_MKVPROPEDIT_BIN` / `MKV_MEDIAINFO_BIN` | `mkvmerge` / `mkvpropedit` / `mediainfo` | Override tool paths. |
| `RUST_LOG` | *(unset)* | Log filter, e.g. `info`, `mkv_server=debug`. |

## HTTP API (server)

| Method | Path | |
|---|---|---|
| GET | `/api/volumes` | Configured library roots |
| GET | `/api/browse/{root}?path=` | Directory listing |
| POST | `/api/analyze` | `{root_id, path}` → mkvmerge probe + MediaInfo analysis + report sections |
| GET / PUT | `/api/config` | Profiles + naming (validated before saving) |
| POST | `/api/jobs` | Submit a remux/edit; output name is resolved inside the output root |
| GET | `/api/jobs/{id}/events` | SSE progress stream |
| POST | `/api/jobs/{id}/cancel` | Cancel (kills the tool process) |

Every client path is resolved by `mkv-volume`, which rejects absolute paths,
`..` traversal and symlink escapes; MediaInfo's absolute file path is replaced
with the bare file name before analysis results leave the server.

## Testing

```bash
# Unit tests — no external tools needed (process execution is mocked)
cargo test --workspace

# Real-binary integration tests (need mkvmerge, mkvpropedit, mediainfo and
# ffmpeg for fixtures; skipped automatically when missing)
cargo test -p mkv-core --test real_binaries
```

`mkv-profiles` covers the dynamic-range matrix (SDR … DV8 HLG), the audio
format/channel matrix, all three audio language rules, subtitle
classification/flags, ~20 real-world release names, and the Radarr/Sonarr
templates (including the missing-TMDb-ID case).

## CI/CD

`.github/workflows/ci.yml` runs on pull requests and on pushes to `main` and
`release/X.Y.Z` branches.

| | Pull request | `main` | `release/X.Y.Z` |
|---|---|---|---|
| `cargo test --workspace` (with real mkvmerge/mediainfo/ffmpeg) | ✓ | ✓ | ✓ |
| Web build (wasm UI + server) | ✓ | ✓ | ✓ |
| Desktop build: macOS arm64, Linux x86_64, Linux arm64 | ✓ | ✓ | ✓ |
| Desktop archives kept as workflow artifacts | | ✓ | ✓ |
| Alpine image for amd64 + arm64 pushed to GHCR | | `:main`, `:sha-…` | `:X.Y.Z`, `:X.Y`, `:latest` |
| GitHub release + formula in the Homebrew tap, then install-tested with `brew` on macOS and Linux | | | ✓ |

**Cutting a release:** bump `version` in the root `Cargo.toml` to `X.Y.Z`
on `main`, then push a `release/X.Y.Z` branch. The run fails early if the
branch name and `Cargo.toml` disagree. Pushing fixes to the same branch
republishes `X.Y.Z` (the image tags, release archives and formula checksums
are replaced).

Linux desktop binaries are built on Ubuntu 22.04, so they need glibc 2.35+,
the same minimum as Homebrew on Linux. Each architecture's image is built on
a native runner, and the two are then merged into one multi-arch tag.

**One-time setup:**

- The public tap repository
  [`bugsbunny-25/homebrew-tap`](https://github.com/bugsbunny-25/homebrew-tap)
  hosts the formula *and* the release archives, since Homebrew can't
  download from this private repository.
- Repository secret `HOMEBREW_TAP_TOKEN`: a fine-grained personal access
  token limited to `bugsbunny-25/homebrew-tap` with **Contents: Read and
  write**.
- Images are pushed with the workflow's own `GITHUB_TOKEN`; nothing else is
  needed.

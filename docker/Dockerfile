# syntax=docker/dockerfile:1
#
# Debian trixie throughout: the runtime needs MediaInfo >= 23.04 (first
# release that reads HDR10+ from Matroska); bookworm ships 23.01. The build
# stages match the runtime's distro so the server binary links against the
# same glibc.

#####################################################################
# Stage 1: build the Slint UI compiled to wasm (mkv-web), served as
# static assets by the server in the final stage. The whole workspace is
# copied since its crates are path dependencies of one another.
#####################################################################
FROM rust:1-slim-trixie AS wasm-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack --locked

WORKDIR /workspace
COPY . .

WORKDIR /workspace/crates/mkv-web
RUN wasm-pack build --release --target web

#####################################################################
# Stage 2: build the server binary. mkv-server has no UI/Slint
# dependency at all, so this stage stays lean (no wasm target).
#####################################################################
FROM rust:1-slim-trixie AS server-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .

RUN cargo build --release -p mkv-server

#####################################################################
# Stage 3: slim runtime image.
#  - mkvtoolnix: mkvmerge / mkvpropedit, which do every remux/edit —
#    nothing in this container re-encodes or writes Matroska itself.
#  - mediainfo: HDR / Dolby Vision / Atmos / DTS:X analysis and the
#    media-info viewer.
#####################################################################
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    mkvtoolnix mediainfo ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system mkv && useradd --system --gid mkv --home-dir /app --create-home mkv

WORKDIR /app

COPY --from=server-builder /workspace/target/release/mkv-server /app/mkv-server
COPY --from=wasm-builder /workspace/crates/mkv-web/index.html /app/static/index.html
COPY --from=wasm-builder /workspace/crates/mkv-web/pkg /app/static/pkg

RUN mkdir -p /data/library /data/output /config && chown -R mkv:mkv /app /data /config

USER mkv

ENV MKV_STATIC_DIR=/app/static \
    MKV_CONFIG_DIR=/config \
    PORT=8080 \
    RUST_LOG=info

# Profiles and naming templates live here (config.toml); mount it to keep
# settings across container rebuilds.
VOLUME ["/config"]

EXPOSE 8080

ENTRYPOINT ["/app/mkv-server"]

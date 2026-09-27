# syntax=docker/dockerfile:1
#
# Alpine throughout. The runtime needs MediaInfo >= 23.04 (first release
# that reads HDR10+ from Matroska); Alpine 3.24 ships 26.01. The server is
# built against musl in an Alpine stage so it runs on the Alpine runtime.
#
# Multi-arch: build with `--platform linux/amd64,linux/arm64`. The wasm
# bundle is architecture-independent, so its stage always runs on the build
# machine's own platform (no emulation) and is shared by every target.

ARG ALPINE_VERSION=3.24

#####################################################################
# Stage 1: build the Slint UI compiled to wasm (mkv-web), served as
# static assets by the server in the final stage. The whole workspace is
# copied since its crates are path dependencies of one another.
#####################################################################
FROM --platform=$BUILDPLATFORM rust:1-alpine${ALPINE_VERSION} AS wasm-builder

RUN apk add --no-cache musl-dev binaryen \
    && rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack --locked

# wasm-pack's own downloads of wasm-bindgen are glibc builds that can't run
# on musl, so install the CLI matching the locked wasm-bindgen version;
# wasm-pack picks it (and binaryen's wasm-opt) up from PATH.
WORKDIR /workspace
COPY Cargo.lock .
RUN version=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p') \
    && cargo install wasm-bindgen-cli --locked --version "$version"

COPY . .

WORKDIR /workspace/crates/mkv-web
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target,id=wasm-target \
    wasm-pack build --release --target web

#####################################################################
# Stage 2: build the server binary for the target platform. mkv-server
# has no UI/Slint dependency at all, so this stage stays lean.
#####################################################################
FROM rust:1-alpine${ALPINE_VERSION} AS server-builder
ARG TARGETPLATFORM

RUN apk add --no-cache musl-dev

WORKDIR /workspace
COPY . .

# The target dir is a cache mount, so the binary is copied out of it.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/target,id=server-target-$TARGETPLATFORM \
    cargo build --release --locked -p mkv-server \
    && cp target/release/mkv-server /mkv-server

#####################################################################
# Stage 3: slim runtime image.
#  - mkvtoolnix: mkvmerge / mkvpropedit, which do every remux/edit —
#    nothing in this container re-encodes or writes Matroska itself.
#  - mediainfo: HDR / Dolby Vision / Atmos / DTS:X analysis and the
#    media-info viewer.
#####################################################################
FROM alpine:${ALPINE_VERSION}

RUN apk add --no-cache mkvtoolnix mediainfo ca-certificates \
    && addgroup -S mkv && adduser -S -G mkv -h /app mkv

WORKDIR /app

COPY --from=server-builder /mkv-server /app/mkv-server
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

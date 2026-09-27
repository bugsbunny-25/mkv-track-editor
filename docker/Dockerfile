# syntax=docker/dockerfile:1
#
# Alpine throughout. The runtime needs MediaInfo >= 23.04 (first release
# that reads HDR10+ from Matroska); Alpine 3.24 ships 26.01. The server is
# built against musl in an Alpine stage so it runs on the Alpine runtime.
#
# Multi-arch: CI builds each platform natively on its own runner (amd64 on
# x86_64, arm64 on arm64) and merges them into one manifest. Locally, build
# only the machine's own platform (the default): any other platform compiles
# the server under emulation (QEMU), many times slower and CPU-bound. The
# wasm bundle is architecture-independent, so its stage always runs on the
# build machine's own platform (no emulation) and is shared by every target.

ARG ALPINE_VERSION=3.24

#####################################################################
# Stage 1: build the Slint UI compiled to wasm (mkv-web), served as
# static assets by the server in the final stage. The whole workspace is
# copied since its crates are path dependencies of one another.
#####################################################################
FROM --platform=$BUILDPLATFORM rust:1-alpine${ALPINE_VERSION} AS wasm-builder

RUN apk add --no-cache musl-dev binaryen curl \
    && rustup target add wasm32-unknown-unknown

# The prebuilt musl wasm-bindgen CLI matching the locked wasm-bindgen
# version, instead of compiling it (and wasm-pack) from source: those two
# `cargo install`s were ~500 crates, rebuilt whenever the builder's cache
# is gone. wasm-pack only ran cargo, wasm-bindgen and wasm-opt, so the
# build below calls them directly.
WORKDIR /workspace
COPY Cargo.lock .
RUN version=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p') \
    && name="wasm-bindgen-$version-$(uname -m)-unknown-linux-musl" \
    && url="https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$version/$name.tar.gz" \
    && cd /tmp \
    && curl -fsSLO "$url" \
    && echo "$(curl -fsSL "$url.sha256sum" | cut -d' ' -f1)  $name.tar.gz" | sha256sum -c - \
    && tar -xzf "$name.tar.gz" \
    && install "$name/wasm-bindgen" /usr/local/bin/ \
    && rm -rf /tmp/wasm-bindgen-*

COPY . .

# Same steps and output (crates/mkv-web/pkg) as `wasm-pack build --release
# --target web`. Each stage has its own registry cache: this stage and the
# server's run in parallel, and cargo's package lock lives outside the
# mount, so a shared one lets both unpack the same crate at once
# ("failed to unpack package ... File exists").
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-wasm \
    --mount=type=cache,target=/workspace/target,id=wasm-target \
    cargo build --release --locked --lib --target wasm32-unknown-unknown -p mkv-web \
    && wasm-bindgen --target web --out-dir crates/mkv-web/pkg \
        target/wasm32-unknown-unknown/release/mkv_web.wasm \
    && wasm-opt -O crates/mkv-web/pkg/mkv_web_bg.wasm -o crates/mkv-web/pkg/mkv_web_bg.wasm

#####################################################################
# Stage 2: build the server binary for the target platform. mkv-server
# has no UI/Slint dependency at all, so this stage stays lean.
#####################################################################
FROM rust:1-alpine${ALPINE_VERSION} AS server-builder
ARG TARGETPLATFORM

RUN apk add --no-cache musl-dev

WORKDIR /workspace
COPY . .

# The target dir is a cache mount, so the binary is copied out of it. See
# the wasm stage for why the registry cache isn't shared with it.
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry-server-$TARGETPLATFORM \
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

#!/usr/bin/env bash
# Cross-compiles the show editor (scareflow-showui) for x86_64 Linux in a Debian
# trixie container (glibc 2.41). The container runs natively on arm64 and links
# against the amd64 libasound from Debian multiarch.
# Output: target/linux-docker/x86_64-unknown-linux-gnu/release/scareflow-showui
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"

docker run --rm \
    -v "$root:/src" \
    -v scareflow-cargo-registry:/usr/local/cargo/registry \
    -w /src \
    rust:1-trixie bash -euo pipefail -c '
        dpkg --add-architecture amd64
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends \
            gcc-x86-64-linux-gnu libc6-dev-amd64-cross libasound2-dev:amd64 pkg-config >/dev/null
        rustup target add x86_64-unknown-linux-gnu
        export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
        export PKG_CONFIG_ALLOW_CROSS=1
        export PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig
        export CARGO_TARGET_DIR=target/linux-docker
        cargo build --release --locked -p scareflow-showui --target x86_64-unknown-linux-gnu
    '

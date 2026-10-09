FROM node:24-bookworm-slim AS node
FROM rust:1-bookworm AS builder

COPY --from=node /usr/local/ /usr/local/

ENV DEBIAN_FRONTEND=noninteractive
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    clang \
    curl \
    dbus-x11 \
    ffmpeg \
    libayatana-appindicator3-dev \
    libavcodec-dev \
    libavfilter-dev \
    libavformat-dev \
    libavutil-dev \
    libavdevice-dev \
    librsvg2-dev \
    libswresample-dev \
    libswscale-dev \
    llvm-dev \
    patchelf \
    p7zip-full \
    pkg-config \
    libwebkit2gtk-4.1-dev \
    xauth \
    xvfb \
    zip \
  && rm -rf /var/lib/apt/lists/*

COPY package.json package-lock.json ./
RUN npm ci

COPY . .
RUN npm run tauri -- build --no-bundle

FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    ca-certificates \
    dbus-x11 \
    ffmpeg \
    git \
    libayatana-appindicator3-1 \
    librsvg2-2 \
    libwebkit2gtk-4.1-0 \
    p7zip-full \
    xauth \
    xvfb \
    zip \
  && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/src-tauri/target/release/fbro /usr/local/bin/fbro
COPY --chmod=755 docker/e2e/fake-nano /usr/local/bin/nano

ENV FBRO_APP_BINARY=/usr/local/bin/fbro
ENV RUST_LOG=info

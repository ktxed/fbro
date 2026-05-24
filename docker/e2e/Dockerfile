FROM rust:1-bookworm

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
    nodejs \
    npm \
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

ENV FBRO_APP_BINARY=/app/src-tauri/target/release/fbro
ENV RUST_LOG=info

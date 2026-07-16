FROM mcr.microsoft.com/devcontainers/rust:latest

# Install additional system dependencies for the GUI
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get install -y --no-install-recommends \
        libxkbcommon-dev \
        libwayland-dev \
        libx11-dev \
        libxcb-composite0-dev \
        libxcb-xfixes0-dev \
        libxcb-xkb-dev \
        libxcb-shape0-dev \
        libgtk-3-dev \
        libdbus-1-dev \
        libvulkan-dev \
        cmake \
        pkg-config \
    && apt-get clean -y \
    && rm -rf /var/lib/apt/lists/*

# Install useful cargo tools
RUN cargo install cargo-deny cargo-audit --locked || true

# Set default build features
ENV CARGO_BUILD_FEATURES=local

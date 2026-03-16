FROM ubuntu:24.04

# Prevent apt from prompting for timezone/keyboard input
ENV DEBIAN_FRONTEND=noninteractive

# 1. Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
    libwayland-dev wayland-protocols gobject-introspection \
    libgirepository1.0-dev valac git meson ninja-build curl

WORKDIR /build

# 2. Build and install gtk4-layer-shell
RUN git clone https://github.com/wmww/gtk4-layer-shell.git && \
    cd gtk4-layer-shell && \
    meson setup build && \
    ninja -C build && \
    ninja -C build install && \
    ldconfig

# 3. Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# 4. Build cursor-clip
RUN cargo install --git https://github.com/Sirulex/cursor-clip

# 5. Move the compiled binary and libraries to an output folder
RUN mkdir /output && \
    cp /root/.cargo/bin/cursor-clip /output/ && \
    cp -a /usr/local/lib/x86_64-linux-gnu/libgtk4-layer-shell.so* /output/

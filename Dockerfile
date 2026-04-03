FROM alpine:latest

WORKDIR /app

# Download dependencies
RUN apk add --no-cache \
    build-base \
    musl-dev \
    pkgconf \
    curl \
    gtk4.0-dev \
    libadwaita-dev \
    gtk4-layer-shell-dev \
    wayland-dev \
    dbus-dev \
    glib-dev
    
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

ENV PATH="/root/.cargo/bin:${PATH}"
ENV CC=gcc CXX=g++

# Force dynamic linking (static GTK libs not available on Alpine)
ENV RUSTFLAGS="-C target-feature=-crt-static"

COPY . .

# Build
RUN cargo build --release

# Execute
RUN cp target/release/cursor-clip /app/run && chmod +x /app/run

ENTRYPOINT ["/app/run"]

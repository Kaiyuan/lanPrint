# syntax=docker/dockerfile:1
FROM rust:1.95-bookworm AS builder
WORKDIR /src

# Cache dependency compilation
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/Cargo.toml
COPY crates/desktop/Cargo.toml crates/desktop/Cargo.toml
COPY crates/bridge/Cargo.toml crates/bridge/Cargo.toml
RUN mkdir -p crates/core/src crates/desktop/src crates/bridge/src \
    && echo 'fn main() {}' > crates/desktop/src/main.rs \
    && echo 'fn main() {}' > crates/bridge/src/main.rs \
    && echo '' > crates/core/src/lib.rs \
    && cargo build --release -p lanprint-bridge || true

COPY crates ./crates
COPY locales ./locales
RUN cargo build --release -p lanprint-bridge --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin lanprint \
    && mkdir -p /data \
    && chown -R lanprint:lanprint /data

COPY --from=builder /src/target/release/lanprint-bridge /usr/local/bin/lanprint-bridge

ENV LANPRINT_BRIDGE_ADDR=0.0.0.0:52334 \
    LANPRINT_BRIDGE_DATA=/data/hosts.json \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 52334
USER lanprint
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/lanprint-bridge", "--version"]
ENTRYPOINT ["/usr/local/bin/lanprint-bridge"]

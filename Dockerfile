# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.74-bookworm AS chef
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY . .
RUN cargo build --release --bin sinker

# We do not need the Rust toolchain to run the binary!
FROM debian:bookworm-slim

RUN apt update \
    && apt install --yes ca-certificates libssl3 --no-install-recommends \
    && rm -rf /var/lib/{apt,dpkg,cache,log} \
    && groupadd --gid 1500 sinker \
    && useradd --uid 1500 --gid sinker --shell /bin/bash --create-home sinker

USER sinker

WORKDIR app
COPY --from=builder /app/target/release/sinker /usr/local/bin
ENTRYPOINT ["/usr/local/bin/sinker"]

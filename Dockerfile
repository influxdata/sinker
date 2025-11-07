# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.85-bookworm AS chef
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
FROM gcr.io/distroless/base-nossl-debian12

WORKDIR app
COPY --from=builder /lib/x86_64-linux-gnu/libgcc_s.so.1 /lib/x86_64-linux-gnu/
COPY --from=builder /app/target/release/sinker /usr/local/bin/sinker
ENTRYPOINT ["/usr/local/bin/sinker"]

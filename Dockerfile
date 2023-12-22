# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.73.0@sha256:09ec7a922dc592d980f3fcfa97b873e1a678ad2fb252671569a65187f1cd4a75 AS chef
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
FROM debian:bookworm-slim@sha256:45287d89d96414e57c7705aa30cb8f9836ef30ae8897440dd8f06c4cff801eec AS runtime
WORKDIR app
COPY --from=builder /app/target/release/sinker /usr/local/bin
ENTRYPOINT ["/usr/local/bin/sinker"]

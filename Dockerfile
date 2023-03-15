# Leveraging the pre-built Docker images with 
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.68.0@sha256:c4590f21203c02486565b1d68c248b1301bd0f1444d04ddeee16b143dfa48b9b AS chef
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
FROM debian:bullseye-slim@sha256:77f46c1cf862290e750e913defffb2828c889d291a93bdd10a7a0597720948fc AS runtime
WORKDIR app
COPY --from=builder /app/target/release/sinker /usr/local/bin
ENTRYPOINT ["/usr/local/bin/sinker"]
# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.74-bookworm@sha256:f2be0d7e17e30166653ccc67498e82759d8124ed8770b48f06395caa8e95c97f AS chef
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
FROM debian:bookworm-slim@sha256:f80c45482c8d147da87613cb6878a7238b8642bcc24fc11bad78c7bec726f340

# Update the system and install necessary packages
RUN apt update \
    && apt install --yes ca-certificates libssl3 --no-install-recommends \
    && rm -rf /var/lib/{apt,dpkg,cache,log}

# Remove setuid/setgid bits from executables as a hardening measure so non-root processes can't escalate.
RUN find / \( -path /dev -o -path /proc -o -path /sys \) -prune -o -type f \( -perm -4000 -o -perm -2000 \) -exec chmod a-s {} \;

# Create a dedicated user and group for the application
RUN groupadd --gid 1500 sinker \
    && useradd --uid 1500 --gid sinker --shell /bin/bash --create-home sinker


USER sinker

WORKDIR app
COPY --from=builder /app/target/release/sinker /usr/local/bin
ENTRYPOINT ["/usr/local/bin/sinker"]

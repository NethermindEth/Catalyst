# NOTE: This Dockerfile requires the repository root as the build context.
# Build with: docker build -f urc/Dockerfile .

# Build stage: compile urc-cli only
FROM rust:1.90-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    libclang-dev \
    ca-certificates \
    pkg-config \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY rust-toolchain.toml .
RUN rustup show

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build -p urc --bin urc-cli --release \
    && cp /app/target/release/urc-cli /usr/local/bin/urc-cli

# Minimal runtime: distroless + binary + CA certs only
FROM gcr.io/distroless/cc-debian13

COPY --from=builder /usr/local/bin/urc-cli /usr/local/bin/urc-cli
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/

USER 65532:65532
ENTRYPOINT ["/usr/local/bin/urc-cli"]

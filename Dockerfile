FROM rust:1.90 AS builder

# Update CA certificates in builder stage
RUN apt-get update && apt-get install -y \
    libclang-dev \
    ca-certificates \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory inside the container
WORKDIR /app/catalyst_whitelist

# Copy only the toolchain file first
COPY rust-toolchain.toml .

# Install the toolchain components
RUN rustup show

# Now copy the rest of the files
COPY . .

# Build catalyst_whitelist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/catalyst_whitelist/target \
    cargo build -p whitelist --release \
    && mv /app/catalyst_whitelist/target/release/catalyst_whitelist /root

# Use small size system for final image
FROM gcr.io/distroless/cc

# Copy artifacts
COPY --from=builder /root/catalyst_whitelist /usr/local/bin/catalyst_whitelist
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /bin/sleep /bin/sleep

ENTRYPOINT ["catalyst_whitelist"]
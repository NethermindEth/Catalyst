FROM rust:1.93 AS builder

# Update CA certificates in builder stage
RUN apt-get update && apt-get install -y \
    libclang-dev \
    ca-certificates \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory inside the container
WORKDIR /app/catalyst_node

# Force blst (and the blst vendored by c-kzg) to compile without ADX/BMI2 asm.
# Without this, binaries built on modern CI runners SIGILL on older Intel CPUs
# (e.g. pre-Broadwell Macs, and Intel-Mac Docker Desktop VMs that don't expose
# those features to the guest).
ARG BLST_PORTABLE=1
ENV BLST_PORTABLE=${BLST_PORTABLE}

# Copy only the toolchain file first
COPY rust-toolchain.toml .

# Install the toolchain components
RUN rustup show

# Now copy the rest of the files
COPY . .

# Build catalyst_whitelist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/catalyst_node/target \
    cargo build -p node --release \
    && mv /app/catalyst_node/target/release/catalyst_node /root

# Use small size system for final image
FROM gcr.io/distroless/cc-debian13

# Copy artifacts
COPY --from=builder /root/catalyst_node /usr/local/bin/catalyst_node
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /bin/sleep /bin/sleep

# Copy required shared libraries, dependencies for event indexer
COPY --from=builder /usr/lib/*/libzstd.so.1 /usr/lib/

ENTRYPOINT ["catalyst_node"]
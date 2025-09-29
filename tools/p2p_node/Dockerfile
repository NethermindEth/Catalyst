FROM rust:1.90 AS builder

# Set the working directory inside the container
WORKDIR /usr/src/build

# Copy only the toolchain file first
COPY rust-toolchain.toml .

# Install the toolchain components
RUN rustup show

# Copy the entire project into the container
COPY . .

# Build the project in release mode
RUN cargo build -p p2p-node --release

# Use ubuntu as the base image
FROM ubuntu:latest

# Copy the build artifact from the builder stage
COPY --from=builder /usr/src/build/target/release/p2p-node /usr/local/bin/p2p-node

# Expose the port that the server will run on
EXPOSE 9000

# Run the binary
CMD ["p2p-node"]

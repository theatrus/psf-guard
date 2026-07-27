# Build stage
# Version should match rust-toolchain.toml
FROM rust:1.97.1 AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    build-essential \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Compile the dependency tree against a stub crate.
#
# "Copy manifests first for better caching" only pays off if something builds
# between the manifest copy and the source copy. Nothing did, so every source
# edit invalidated the single `cargo build --release` layer and recompiled
# every dependency from scratch: 614s on amd64 and 555s on arm64 for the
# v0.6.3 merge, and a fresh multi-hundred-MB layer written to the build cache
# each time.
#
# The stubs keep this layer keyed on Cargo.toml and Cargo.lock alone. build.rs
# is stubbed too — the real one shells out to npm, and lib.rs is empty here so
# nothing yet references static/dist.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && \
    echo 'fn main() {}' > src/main.rs && \
    : > src/lib.rs && \
    echo 'fn main() {}' > src/bin/psf-guard-cli.rs && \
    echo 'fn main() {}' > build.rs && \
    cargo build --release --locked && \
    rm -rf src build.rs

# Copy source code. Only these layers move on an ordinary commit; the compiled
# dependencies above are reused.
COPY build.rs ./
COPY src ./src
COPY static ./static

# Drop the stub crate's fingerprints before building for real.
#
# COPY preserves the host's mtimes, and cargo decides freshness by mtime. The
# stub above was written inside the container, so it is *newer* than the real
# sources that land on top of it: cargo concludes psf-guard is already built,
# skips build.rs, never runs the frontend build, and the compile dies on
# include_dir!("static/dist") with "is not a directory". Clearing only this
# crate's fingerprints forces it and its build script to run again; every
# compiled dependency in target/ is left alone, which is the point of the
# layer above.
RUN rm -rf target/release/.fingerprint/psf-guard-* && \
    cargo build --release --locked

# Runtime stage - use trixie to match the build stage
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 psfguard

# Copy binary from builder
COPY --from=builder /app/target/release/psf-guard /usr/local/bin/psf-guard

# Create directories for mounting
RUN mkdir -p /data /images && \
    chown -R psfguard:psfguard /data /images

USER psfguard

# Expose the web server port
EXPOSE 3000

# Default volumes for database and images
VOLUME ["/data", "/images"]

# Default command to run the server
# Users can override with their own database and image paths
ENTRYPOINT ["/usr/local/bin/psf-guard"]
CMD ["server", "/data/database.sqlite", "/images"]

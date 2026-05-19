# Build stage
# Version should match rust-toolchain.toml
FROM rust:1.89.0 AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libopencv-dev \
    clang \
    llvm \
    libclang-dev \
    cmake \
    build-essential \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy manifests first for better caching
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src
COPY static ./static
COPY build.rs ./

# Build the application with OpenCV support
ENV OPENCV_LINK_LIBS=opencv_core,opencv_imgproc,opencv_imgcodecs,opencv_ximgproc
ENV OPENCV_LINK_PATHS=/usr/lib/x86_64-linux-gnu
ENV OPENCV_INCLUDE_PATHS=/usr/include/opencv4

RUN cargo build --release --features opencv

# Runtime stage - use trixie to match the build stage
FROM debian:trixie-slim

# Install runtime dependencies including OpenCV libraries
RUN apt-get update && apt-get install -y \
    libopencv-core410 \
    libopencv-imgproc410 \
    libopencv-imgcodecs410 \
    libopencv-contrib410 \
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
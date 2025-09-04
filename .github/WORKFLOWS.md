# PSF Guard CI/CD Workflows

This repository contains workflows for building both CLI and desktop versions of PSF Guard.

## Workflow Overview

### CI Workflows (Run on PRs and pushes)

1. **`ci.yml`** - Comprehensive testing and building
   - **test job**: Tests the core Rust CLI functionality, builds and uploads CLI binaries, runs formatting, clippy, and tests
   - **test-tauri job**: Separately tests complete Tauri build process on all platforms in parallel
   - **Artifacts**: 
     - CLI binaries: `psf-guard-{platform}-x64`
     - Tauri bundles: `psf-guard-tauri-{platform}-x64` (includes installers and CLI binary)

2. **Other CI workflows**:
   - `dependency-review.yml` - Security review of dependencies
   - `security-audit.yml` - Security auditing with cargo-audit
   - `docker.yml` - Docker containerization

### Release Workflows (Run on version tags)

1. **`release.yml`** - Complete release with both CLI and Desktop apps
   - Creates both CLI binaries and Tauri desktop applications
   - Produces standalone executables and native installers
   - **Artifacts**: 
     - **CLI binaries**: `psf-guard-{platform}-x64` executables
     - **Linux**: `.deb` packages, `.AppImage` bundles
     - **Windows**: `.msi` installers, NSIS installers
     - **macOS**: `.dmg` installers, `.app` bundles (zipped)

## Build Process

### Frontend Build
All workflows now build the React frontend as part of the process:
```bash
cd static
npm ci
npm run build
```

### Tauri Build Process
The release workflow builds both CLI and Tauri versions:
1. Install system dependencies (WebKit, OpenCV, etc.)
2. Install Node.js and build frontend
3. Install Rust toolchain
4. Install Tauri CLI: `cargo install tauri-cli --version "^2.0"`
5. Build CLI binary: `cargo build --release --locked`
6. Build Tauri app: `cargo tauri build --verbose`
7. Package and upload both CLI binaries and native bundles

## System Dependencies

### Ubuntu/Debian
```bash
sudo apt-get install -y \
  libopencv-dev clang libclang-dev \
  libwebkit2gtk-4.1-dev \
  libappindicator3-dev \
  librsvg2-dev \
  patchelf
```

### macOS
```bash
brew install opencv
```

### Windows
- Uses vcpkg for OpenCV: `opencv4[contrib,nonfree]:x64-windows-static-md`
- WebKit dependencies are handled by Tauri automatically

## Smart Binary Architecture

PSF Guard uses a smart binary approach:
- Single binary that detects run mode automatically
- GUI mode: When `tauri` feature is enabled and no CLI arguments passed
- CLI mode: When arguments are provided OR `tauri` feature is disabled

This means:
- `cargo run` → GUI mode (if tauri feature enabled)
- `cargo run -- server --help` → CLI mode
- `cargo run --features tauri` → GUI mode
- `cargo run --features tauri -- --help` → CLI mode

## Release Artifacts

All releases now include both CLI binaries and desktop applications:

### CLI Binaries
- `psf-guard-linux-x64` - Standalone Linux binary
- `psf-guard-windows-x64.exe` - Standalone Windows binary
- `psf-guard-macos-x64` - Standalone macOS binary

### Desktop Applications
- **Linux**: 
  - `*.deb` - Debian package installer
  - `*.AppImage` - Portable application bundle
- **Windows**:
  - `*.msi` - Windows Installer package
  - `*-installer.exe` - NSIS installer
- **macOS**:
  - `*.dmg` - macOS disk image installer
  - `*-macos.zip` - Zipped .app bundle

## Running Workflows

### Trigger CI
- Push to `main` or `master` branch
- Create pull request targeting `main` or `master`

### Trigger Release
- Create and push a version tag: `git tag v1.0.0 && git push origin v1.0.0`
- A single comprehensive release will be created with both CLI binaries and desktop applications
- Release notes are generated automatically

## Development Tips

1. **Test locally before pushing**: Use `cargo tauri build` to test Tauri builds locally
2. **Frontend changes**: Ensure `npm run build` works in the `static/` directory
3. **Version bumps**: Update version in `Cargo.toml` and `tauri.conf.json`
4. **Platform testing**: The CI workflows test on all three major platforms
5. **Cache optimization**: Workflows use extensive caching for faster builds

## Troubleshooting

### Common Issues

1. **Tauri build fails**: 
   - Check system dependencies are installed
   - Ensure frontend builds successfully
   - Verify Tauri CLI is the correct version

2. **OpenCV linking issues**:
   - Check platform-specific OpenCV installation
   - Verify environment variables (especially on Windows)

3. **Bundle generation fails**:
   - Ensure all required system dependencies are installed
   - Check Tauri configuration in `tauri.conf.json`
   - Verify app icons exist and are properly configured

4. **Frontend build fails**:
   - Check Node.js version (should be 24)
   - Verify `package-lock.json` is committed
   - Ensure all npm dependencies are available
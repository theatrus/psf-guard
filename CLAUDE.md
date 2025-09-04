# CLAUDE.md - Development Notes

## Project Overview

PSF Guard is a Rust CLI utility for analyzing N.I.N.A. Target Scheduler databases and managing astronomical image files. Features N.I.N.A. star detection algorithm, PSF fitting, and React web interface.

## Quick Start

```bash
# Development
cargo fmt && cargo clippy && cargo test

# CLI mode - Run server (supports multiple directories)
cargo run -- server db.sqlite images1/ images2/

# GUI mode - Launch desktop app (with tauri feature)
cargo run --features tauri

# Tauri desktop development
cargo tauri dev

# Build for production
cargo build --release                    # CLI only
cargo build --release --features tauri   # GUI capable
```

## Architecture

### Core Components
- **CLI**: Command-pattern with clap-derive (`src/cli_main.rs`)
- **Tauri Desktop**: Desktop app with server integration (`src/tauri_main.rs`)
- **Database**: SQLite via rusqlite
- **Star Detection**: N.I.N.A. algorithm + HocusFocus detector
- **Web Server**: Axum + embedded React frontend
- **Cache System**: Directory tree + file cache with 5-minute TTL

### Smart Binary Mode Selection
- **Single binary** `psf-guard` with intelligent mode detection
- **GUI mode**: When tauri feature is enabled and no arguments passed → Desktop app launches
- **CLI mode**: When arguments are provided OR tauri feature is disabled → Command-line interface
- `src/main.rs`: Smart dispatcher that checks for arguments to determine mode
- `src/cli_main.rs`: Traditional command-line interface implementation
- `src/tauri_main.rs`: Tauri desktop application implementation

### Cache System (Current)
- **File Cache**: Database-based existence checking, auto-refreshed every 5 minutes
- **Directory Tree**: In-memory filename→path mapping, auto-refreshed every 5 minutes
- **Singleton Refresh**: Non-blocking with real-time progress tracking via SSE
- **Manual Refresh**: Button (file cache) + Shift+click (both caches)
- **Multi-Directory**: Scans multiple directories with first-hit preference

## Database Schema

```sql
project (1:many) → target (1:many) → acquiredimage

acquiredimage:
- gradingStatus: 0=Pending, 1=Accepted, 2=Rejected
- metadata: JSON with FileName
```

**Column naming**: Use exact case - `Id`, `projectId`, `acquireddate`, `filtername`

## Web Server

### API Endpoints
```
GET  /api/projects
GET  /api/images?project_id=X&target_id=Y
PUT  /api/images/{id}/grade
GET  /api/images/{id}/preview?size=screen|large|original
POST /api/cache/refresh (file cache)
POST /api/cache/refresh-directory (directory cache)
GET  /api/cache/status (SSE progress stream)
```

### Frontend Architecture
- React 18 + TypeScript + Vite
- TanStack Query for server state
- Hash router with URL state management
- Embedded in binary for single-file deployment

### Navigation Fix (2025-09-01)
Fixed overview→grid navigation by building URLs directly:
- `navigate('/grid?project=5')` instead of state coordination
- Eliminates race conditions and timing issues
- Works for projects, targets, and "all projects"

## Key Features

### Web UI
- Smart image loading (preview → full resolution)
- Batch operations with multi-selection
- Undo/redo system (Ctrl+Z/Y)  
- Side-by-side comparison with zoom sync
- Real-time cache refresh with progress tracking

### Cache Progress UI (2025-09-01)
- Smart path truncation showing distinctive parts
- Pulsating progress indicator with integrated timer
- Fixed dimensions to prevent layout shifts
- Hover tooltips for full paths

## Development Workflow

```bash
# Essential commands
cargo fmt && cargo clippy && cargo test

# Run with logging
RUST_LOG=debug cargo run -- server db.sqlite images/

# Tauri desktop development
cargo tauri dev

# Frontend development  
cd static && npm run dev

# OpenCV setup (macOS)
brew install opencv
export DYLD_FALLBACK_LIBRARY_PATH="$(xcode-select --print-path)/Toolchains/XcodeDefault.xctoolchain/usr/lib/"
```

### Tauri Desktop Configuration
- **Settings Panel**: Configure database and image directories via native file dialogs
- **System Directory Structure**: Uses platform-appropriate directories for all data:
  - **Configuration**: 
    - macOS: `~/Library/Application Support/psf-guard/config.json`
    - Windows: `%APPDATA%\psf-guard\config.json`
    - Linux: `~/.config/psf-guard/config.json`
  - **Cache**: 
    - macOS: `~/Library/Caches/psf-guard/`
    - Windows: `%LOCALAPPDATA%\psf-guard\cache\`
    - Linux: `~/.cache/psf-guard/`
  - **Temp Database** (when no N.I.N.A. database found):
    - macOS: `~/Library/Application Support/psf-guard/temp.db`
    - Windows: `%APPDATA%\psf-guard\temp.db`
    - Linux: `~/.local/share/psf-guard/temp.db`
- **Smart Settings Modal**: Only appears on first launch or when configuration is invalid/missing
- **Configuration Updates**: Settings saved immediately, with user-friendly restart prompt to apply changes  
- **Automatic Loading**: Configuration loaded and validated on application startup
- **Directory Management**: All directories are automatically created as needed
- **Database Validation**: Checks that configured database file actually exists before considering config valid

### Development Notes
- **Important**: Remove `static/dist/` contents if Tauri detection fails - cached production assets may be served instead of dev server
- File picker commands are async to prevent UI freezing
- Application restart applies configuration changes cleanly without data loss

### Recent Fixes
- **Navigation**: Direct URL building eliminates timing issues
- **Cache Progress**: Real-time directory scanning with smart path display
- **Multi-Directory**: Priority-based file lookup with comprehensive caching

## Key Implementation Details

### Star Detection
- N.I.N.A. algorithm port with MTF stretching
- Optional OpenCV integration (`--features opencv`)
- PSF fitting: Gaussian/Moffat models

### Performance
- O(1) file lookups via directory tree cache
- Non-blocking server startup with background refresh
- Comprehensive cache key strategy prevents collisions
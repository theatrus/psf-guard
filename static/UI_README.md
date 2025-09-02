# PSF Guard Web UI

This is the React-based web interface for PSF Guard, providing an efficient image grading workflow with keyboard navigation.

## Development

1. **Start the backend server** (from project root):
   ```bash
   cargo run -- server schedulerdb.sqlite filter/ --port 3000
   ```

2. **Start the React dev server** (from this directory):
   ```bash
   npm install  # First time only
   npm run dev
   ```

3. Open http://localhost:5173 in your browser

## Building for Production

```bash
npm run build
```

The built files will be in the `dist/` directory. The PSF Guard server will automatically serve these files when running in production mode.

## Keyboard Shortcuts

- **Navigation**: J/K or arrow keys to move between images
- **Grading**: A (accept), R (reject), U (unmark/pending)
- **View**: Enter (detail view), S (toggle stars), Z (toggle zoom)
- **Help**: ? (show shortcuts)

## Features

- Project and target selection
- Grid view with lazy loading for thousands of images
- Keyboard-first navigation for efficient grading
- Real-time image preview with star detection overlay
- Detailed metadata view
- Visual status indicators

## Architecture

- **React** with TypeScript
- **Vite** for fast development and building
- **React Query** for API state management
- **react-hotkeys-hook** for keyboard shortcuts
- **react-intersection-observer** for lazy loading
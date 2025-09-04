import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { AppRouter } from './router'
import { initializeApiClient } from './api/client'
import './index.css'

// Initialize API client (handles Tauri detection and server URL caching)
initializeApiClient().then(() => {
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <AppRouter />
    </StrictMode>,
  )
}).catch(error => {
  console.error('Failed to initialize API client:', error);
  // Fallback: render anyway with default settings
  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <AppRouter />
    </StrictMode>,
  )
})

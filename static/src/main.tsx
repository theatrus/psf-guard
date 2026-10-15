import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { AppRouter } from './router'
import './index.css'

// Desktop startup and recovery render before any HTTP client is initialized.
// AuthGate's first request initializes the client once the owned server is ready.
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <AppRouter />
  </StrictMode>,
)

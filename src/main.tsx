import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import OsdOverlay from './components/OsdOverlay';
import './index.css';

const params = new URLSearchParams(window.location.search);
const isOsd = !!(window as any).__OPENREVO_OSD_WINDOW__ || params.get('window') === 'osd' || window.location.hash === '#/osd';

async function bootstrap() {
  // Browser dev mode: install a mock of the Tauri v2 IPC surface before React
  // renders, so `pnpm dev` runs the real UI without the Rust backend.
  // The dynamic import sits behind `import.meta.env.DEV`, so neither the mock
  // nor this branch exists in production bundles.
  if (import.meta.env.DEV && !(window as any).__TAURI_INTERNALS__) {
    try {
      const mock = await import('./dev/mockTauri');
      mock.installMockTauri();
    } catch (err) {
      console.error('[OpenRevo mock] Failed to install browser Tauri mock:', err);
    }
  }

  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      {isOsd ? (
        <OsdOverlay />
      ) : (
        <App />
      )}
    </React.StrictMode>,
  );
}

void bootstrap();

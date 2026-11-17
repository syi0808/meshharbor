import React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './studio/App';
import './styles.css';
import './Workbench.css';
import './ui/shadcn.css';
import './StudioShell.css';
import './StudioDesign.css';

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

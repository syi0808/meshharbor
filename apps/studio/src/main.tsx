import React from 'react';
import { createRoot } from 'react-dom/client';
import { IconContext } from '@phosphor-icons/react';
import { App } from './studio/App';
import './styles.css';
import './Workbench.css';
import './ui/shadcn.css';
import './StudioShell.css';
import './StudioDesign.css';

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <IconContext.Provider value={{ size: 16, weight: 'regular' }}>
      <App />
    </IconContext.Provider>
  </React.StrictMode>,
);

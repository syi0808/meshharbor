import React from 'react';
import { createRoot } from 'react-dom/client';
import { IconContext } from '@phosphor-icons/react';
import { App } from './studio/App';
import './index.css';

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <IconContext.Provider value={{ size: 16, weight: 'regular' }}>
      <App />
    </IconContext.Provider>
  </React.StrictMode>,
);

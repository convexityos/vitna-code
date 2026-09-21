import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@fontsource-variable/space-grotesk';
import '@fontsource-variable/manrope';
import '@fontsource-variable/jetbrains-mono';
import './styles/tokens.css';
import './styles/base.css';
import './styles/bar.css';
import './styles/stage.css';
import './styles/composer.css';
import './styles/run.css';
import './styles/approval.css';
import './styles/changes.css';
import './styles/receipt.css';
import './styles/overlay.css';
import { App } from './app/App';

const root = document.getElementById('root');
if (!root) throw new Error('index.html is missing #root');

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

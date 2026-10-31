import React from 'react';
import ReactDOM from 'react-dom/client';
import App from '@/App';
import { Agentation } from 'agentation';

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <App />
    {import.meta.env.DEV && <Agentation />}
  </React.StrictMode>
);

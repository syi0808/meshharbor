import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { createDevSecurityMiddleware } from './dev-security.mjs';

export default defineConfig({
  plugins: [react(), {
    name: 'meshharbor-local-dev-security',
    configureServer(server) {
      server.middlewares.use(createDevSecurityMiddleware());
    },
  }],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: true,
    // The development proxy does not expand API access: the guard plugin
    // above validates the browser Origin and dashboard marker before proxying.
    proxy: {
      '/api/v1': {
        target: 'http://127.0.0.1:7432',
        changeOrigin: true,
        configure(proxy) {
          proxy.on('proxyReq', (proxyRequest) => {
            proxyRequest.setHeader('Origin', 'http://127.0.0.1:7432');
          });
        },
      },
    },
  },
  build: {
    manifest: true,
    outDir: 'dist',
    emptyOutDir: true,
  },
});

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";
import path from "path";

export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  plugins: [
    react(),
    VitePWA({
      registerType: "autoUpdate",
      devOptions: {
        enabled: true,
      },
    }),
  ],
  server: {
    proxy: {
      "/api": {
        target: "http://0.0.0.0:8084",
        changeOrigin: true,
        secure: false,
        ws: false,
        configure: (proxy, _options) => {
          proxy.on("error", (err, _req, _res) => {
            console.error("Proxy error", err);
          });
          proxy.on("proxyReq", (_proxyReq, req, _res) => {
            console.log("Sending request:", req.method, req.url);
          });
          proxy.on("proxyRes", (proxyRes, req, _res) => {
            console.log(
              "Received response:",
              proxyRes.statusCode,
              req.url,
            );
          });
        },
      },
    },
  },
});

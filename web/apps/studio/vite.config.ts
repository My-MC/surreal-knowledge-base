import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  resolve: {
    // Same T10 landmine as vault: @skb/api-client re-exports useChatStream,
    // which imports "react" without declaring it. Force Vite to resolve
    // react/react-dom from this app's copy.
    dedupe: ["react", "react-dom"],
  },
  server: {
    proxy: {
      "/api": {
        target: `http://127.0.0.1:${process.env.SKB_SERVER_PORT ?? "8080"}`,
        changeOrigin: true,
      },
    },
  },
});

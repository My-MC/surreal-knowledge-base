import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
// @skb/ui's exports map only exposes "." — deep CSS subpaths are blocked, so
// the design tokens are imported relatively from the workspace package.
// MarkdownView styling ships with the package and must be imported by apps.
import "../../../packages/ui/src/tokens.css";
import "../../../packages/ui/src/markdown/markdown.css";
import { queryClient, router } from "./router";
import "./blog.css";

const rootElement = document.getElementById("root");
if (!rootElement) {
  throw new Error("#root not found in index.html");
}

createRoot(rootElement).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);

import { QueryClient } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { AppLayout } from "./App";
import { DocumentEditor } from "./components/DocumentEditor";
import { IndexView } from "./components/IndexView";

export const queryClient = new QueryClient();

const rootRoute = createRootRoute({ component: AppLayout });

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: IndexView,
});

const docRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/doc/$id",
  component: DocumentEditor,
});

const routeTree = rootRoute.addChildren([indexRoute, docRoute]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

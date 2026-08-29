import { QueryClient } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter, redirect } from "@tanstack/react-router";
import { AppLayout } from "./App";
import { useAuthStore } from "./auth";
import { AuthForm } from "./components/AuthForm";
import { NewPost } from "./components/NewPost";
import { PostDetail } from "./components/PostDetail";
import { PostList } from "./components/PostList";

export const queryClient = new QueryClient();

const rootRoute = createRootRoute({ component: AppLayout });

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: PostList,
});

const postRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/post/$id",
  component: PostDetail,
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: () => <AuthForm mode="login" />,
});

const registerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/register",
  component: () => <AuthForm mode="register" />,
});

const newRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/new",
  component: NewPost,
  // Author guard: the role is the client-side echo of the session JWT's
  // claim; without it (logged out, or a reader) there is nothing to do here.
  beforeLoad: () => {
    if (useAuthStore.getState().role !== "author") {
      throw redirect({ to: "/login" });
    }
  },
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  postRoute,
  loginRoute,
  registerRoute,
  newRoute,
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

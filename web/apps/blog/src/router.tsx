import { QueryClient } from "@tanstack/react-query";
import { createRootRoute, createRoute, createRouter } from "@tanstack/react-router";
import { AppLayout } from "./App";
import { AuthStub } from "./components/AuthStub";
import { NewPostStub } from "./components/NewPostStub";
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

// Skeletons only — todo 19 implements the auth UI and the posting flow.
const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  component: () => <AuthStub mode="login" />,
});

const registerRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/register",
  component: () => <AuthStub mode="register" />,
});

const newRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/new",
  component: NewPostStub,
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

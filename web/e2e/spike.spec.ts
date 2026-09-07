import { test } from "@playwright/test";

// Bun compatibility spike: proves the Playwright runner works under the Bun
// runtime (node is absent on this machine). Todos 15/17/19 build on web/e2e/.
test("spike", () => {});

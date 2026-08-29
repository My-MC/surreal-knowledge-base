import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { repoRoot } from "./helpers.mts";

/**
 * Blog auth + posting + publish e2e (plan todo 19). Fully self-contained:
 * the flow test seeds its own published post through the API (register →
 * login → upload with metadata app=blog → publish) with unique <ts> titles,
 * then drives register → logout → login → /new → 投稿完了 → 公開する → list →
 * detail through the real UI. The 関連記事 assertion relies on entity
 * extraction: the new post's content carries a [[SeedPost<ts>]] wikilink and
 * the seeded post's `# SeedPost<ts>` heading mints the same-named entity, so
 * the backlinks walk deterministically connects them (T15/T18 pattern).
 *
 * Files are .mts on purpose — see vault.spec.mts for the bun/Playwright
 * loader rationale.
 */

const BLOG_URL = "http://localhost:5175/";
const evidenceDir = path.join(repoRoot, "target", "evidence", "19");

interface UploadResponse {
  document_id: string | null;
  entities: string[];
  status: string;
}

test("blog: register, login, post, publish, and related posts", async ({ page, request }) => {
  mkdirSync(evidenceDir, { recursive: true });
  const ts = Date.now();
  const seedTitle = `SeedPost${ts}`;
  const seedEmail = `seeder${ts}@example.com`;
  const bloggerEmail = `blogger${ts}@example.com`;
  const password = "blog-e2e-passw0rd";

  // -- Step 1: seed one published post via the API (self-contained) ----------
  const registered = await request.post(`${BLOG_URL}api/auth/register`, {
    data: { email: seedEmail, password, role: "author" },
  });
  expect(registered.status()).toBe(201);
  const loggedIn = await request.post(`${BLOG_URL}api/auth/login`, {
    data: { email: seedEmail, password },
  });
  expect(loggedIn.ok()).toBeTruthy();
  const seeded = await request.post(`${BLOG_URL}api/documents`, {
    data: {
      title: seedTitle,
      content: `# ${seedTitle}\n\n${seedTitle} はこの e2e でシードされた公開済み投稿です。`,
      metadata: { app: "blog" },
    },
  });
  expect(seeded.status()).toBe(201);
  const seedDoc = (await seeded.json()) as UploadResponse;
  const seedDocId = seedDoc.document_id;
  expect(seedDocId).toMatch(/^document:/);
  // The heading must have minted the entity the wikilink will target.
  expect(seedDoc.entities).toContain(seedTitle);
  const publishedSeed = await request.post(
    `${BLOG_URL}api/blog/posts/${encodeURIComponent(seedDocId)}/publish`,
  );
  expect(publishedSeed.ok()).toBeTruthy();

  // -- Step 2: register an author through the UI (auto-login) ----------------
  await page.goto(`${BLOG_URL}register`);
  await page.getByTestId("auth-email").fill(bloggerEmail);
  await page.getByTestId("auth-password").fill(password);
  await page.getByTestId("auth-role").selectOption("author");
  await page.getByTestId("auth-submit").click();
  await page.waitForURL((url) => url.pathname === "/");
  await expect(page.getByTestId("header-email")).toHaveText(bloggerEmail);

  // -- Step 3: log out, then log back in through /login ----------------------
  await page.getByTestId("logout").click();
  await expect(page.getByRole("link", { name: "ログイン" })).toBeVisible();
  await expect(page.getByTestId("header-email")).toHaveCount(0);
  await page.goto(`${BLOG_URL}login`);
  await page.getByTestId("auth-email").fill(bloggerEmail);
  await page.getByTestId("auth-password").fill(password);
  await page.getByTestId("auth-submit").click();
  await page.waitForURL((url) => url.pathname === "/");
  await expect(page.getByTestId("header-email")).toHaveText(bloggerEmail);

  // -- Step 4: /new → post with a wikilink to the seeded post ----------------
  const postTitle = `BlogFlow${ts}`;
  await page.goto(`${BLOG_URL}new`);
  await page.getByTestId("new-title").fill(postTitle);
  await page.getByTestId("new-content").fill(`# ${postTitle}\n\n関連: [[${seedTitle}]]`);
  await page.getByTestId("new-submit").click();
  await expect(page.getByTestId("new-success")).toBeVisible();

  // -- Step 5: 公開する → redirected to / → the post is listed ---------------
  await page.getByTestId("new-publish").click();
  await page.waitForURL((url) => url.pathname === "/");
  const listLink = page.getByTestId("post-list").getByRole("link", { name: postTitle });
  await expect(listLink).toBeVisible();
  await page.screenshot({ path: path.join(evidenceDir, "blog-flow.png"), fullPage: true });

  // -- Step 6: open it → 関連記事 shows the seeded post -----------------------
  await listLink.click();
  await page.waitForURL((url) => url.pathname.startsWith("/post/"));
  const related = page.getByTestId("related");
  await expect(related.getByRole("link", { name: seedTitle })).toBeVisible();
});

test("blog: logged-out /new redirects to /login", async ({ page }) => {
  mkdirSync(evidenceDir, { recursive: true });

  await page.goto(`${BLOG_URL}new`);
  await page.waitForURL((url) => url.pathname === "/login");
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByTestId("auth-email")).toBeVisible();
  await page.screenshot({ path: path.join(evidenceDir, "failure.png"), fullPage: true });
});

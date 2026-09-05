import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { evidenceDir } from "./helpers.mts";

/**
 * Graph assertion seam (documented choice): packages/ui is off-limits, so
 * GraphView cannot grow a testid. The seam is the wire + the DOM — the spec
 * awaits the real POST /api/graph/query response (filtered by the request
 * body's `from`, which StrictMode double-mounts make necessary) and asserts
 * nodes.length > 0, then asserts the sigma <canvas> renders inside the
 * fullscreen overlay. The overlay renders an app-level empty state
 * (data-testid="graph-empty") instead of GraphView when a document has no
 * depth>0 entity nodes — the depth-0 document node is always present for a
 * `document:` origin, so GraphView's own empty state is unreachable there.
 *
 * Files are .mts on purpose: under the bun runtime Playwright skips its TS
 * loader and requires CJS-mode .ts specs through a JS parser, which chokes on
 * type annotations; .mts takes bun's native ESM/TS import path.
 */

type Backlink = { id: string; title: string };
type GraphNode = { id: string; name: string; kind: string; depth: number };
type SearchHitDto = { document_id: string; title?: string | null };

const docPath = (id: string) => `/doc/${id}`;

async function currentDocId(page: Page): Promise<string> {
  const pathname = decodeURIComponent(new URL(page.url()).pathname);
  expect(pathname.startsWith("/doc/")).toBe(true);
  return pathname.slice("/doc/".length);
}

test("vault: backlinks, graph view, cmd+k search, and selection qa", async ({ page, request }) => {
  mkdirSync(evidenceDir, { recursive: true });

  // -- Step 1: fresh vault -> 新規作成 -> /doc/{id} with editor -------------
  await page.goto("/");
  // Scoped to main: the tree header carries a second 新規作成 button.
  const createButton = page.locator(".vault-main").getByRole("button", { name: "新規作成" });
  await expect(createButton).toBeVisible();
  await createButton.click();
  await page.waitForURL(/\/doc\//);
  await expect(page.locator(".cm-content")).toBeVisible();
  const id1 = await currentDocId(page);

  // Marker for the no-reload assertion after the autosave rotation.
  await page.evaluate(() => {
    (window as { e2eAlive?: boolean }).e2eAlive = true;
  });

  // -- Step 2: type [[Bar]] content -> debounce PUT -> route rotates -------
  await page.locator(".cm-content").click();
  await page.keyboard.press("Control+a");
  await page.keyboard.type("# Untitled\n\n関連: [[Bar]]");
  await page.waitForFunction(
    (prevPath) => decodeURIComponent(window.location.pathname) !== prevPath,
    docPath(id1),
    { timeout: 15_000 },
  );
  const id1new = await currentDocId(page);
  expect(id1new).not.toBe(id1);
  // Rotation is a replace-navigation, not a page reload.
  expect(await page.evaluate(() => (window as { e2eAlive?: boolean }).e2eAlive)).toBe(true);

  // -- Step 3: seed doc 2 "Bar" via API (title + Bar-heavy content) --------
  const seeded = await request.post("/api/documents", {
    data: {
      title: "Bar",
      content:
        "# Bar\n\nBar についての文書です。Bar は重要なトピックで、Bar への参照が複数あります。",
    },
  });
  expect(seeded.ok()).toBeTruthy();
  const doc2 = (await seeded.json()) as { document_id: string };
  const id2 = doc2.document_id;
  expect(id2).toMatch(/^document:/);

  // -- Step 4: backlinks of doc 1 list doc 2; click navigates --------------
  // "/" redirects to the latest document (doc 2); wait it out so its pane's
  // requests cannot race the response captures below (the predicates filter
  // by doc-1 identity anyway).
  await page.goto("/");
  await page.waitForURL(/\/doc\//);
  const backlinksResponse = page.waitForResponse((response) =>
    decodeURIComponent(response.url()).includes(`/api/documents/${id1new}/backlinks`),
  );
  const graphResponse = page.waitForResponse(
    (response) =>
      response.url().includes("/api/graph/query") &&
      response.request().postDataJSON()?.from === id1new,
  );
  // Scoped to the sidebar: doc 2's own backlinks pane also renders an
  // "untitled" button (doc 1 mentions Bar), which would trip strict mode.
  const treeItem = (title: string) =>
    page.locator(".vault-sidebar").getByRole("button", { name: title });
  await treeItem("untitled").click();
  await page.waitForURL((url) => decodeURIComponent(url.pathname) === docPath(id1new));

  const backlinks = (await (await backlinksResponse).json()) as { documents: Backlink[] };
  expect(backlinks.documents.map((doc) => doc.title)).toContain("Bar");
  await page.getByTestId("backlink-item").filter({ hasText: "Bar" }).click();
  await page.waitForURL((url) => decodeURIComponent(url.pathname) === docPath(id2));

  // -- Step 5: fullscreen graph on doc 1: canvas + non-empty nodes ---------
  await treeItem("untitled").click();
  await page.waitForURL((url) => decodeURIComponent(url.pathname) === docPath(id1new));
  const graph = (await (await graphResponse).json()) as { nodes: GraphNode[] };
  expect(graph.nodes.length).toBeGreaterThan(0);
  expect(graph.nodes.some((node) => node.name === "Bar")).toBe(true);

  // Happy-path evidence: backlinks + related entities pane (the graph canvas
  // itself renders edge-clipped — sigma autoRescale maps toGraphology's unit
  // circle onto the viewport boundary; fixing that lives in packages/ui).
  await expect(page.getByTestId("backlinks-list")).toContainText("Bar");
  await expect(page.getByTestId("related-entities")).toContainText("Bar");
  await page.screenshot({ path: path.join(evidenceDir, "vault-e2e.png"), fullPage: true });

  await page.getByRole("button", { name: "全画面グラフ" }).click();
  const overlay = page.getByTestId("graph-overlay");
  await expect(overlay.locator("canvas").first()).toBeVisible();
  await overlay.getByRole("button", { name: "閉じる" }).click();
  await expect(overlay).toBeHidden();

  // -- Step 6: Cmd+K -> "Bar" -> navigate to doc 2 --------------------------
  await page.keyboard.press(process.platform === "darwin" ? "Meta+k" : "Control+k");
  const paletteInput = page.locator(".skb-palette-input");
  await expect(paletteInput).toBeVisible();
  const searchResponse = page.waitForResponse(
    (response) => response.url().includes("/api/search") && response.request().method() === "POST",
  );
  await paletteInput.fill("Bar");
  const hits = ((await (await searchResponse).json()) as { hits: SearchHitDto[] }).hits;
  expect(hits.length).toBeGreaterThan(0);
  // Selection is driven by the response payload, not the ranking: move the
  // palette cursor onto doc 2's hit whatever rank hybrid search gave it.
  const hitIndex = hits.findIndex((hit) => hit.document_id === id2);
  expect(hitIndex).toBeGreaterThanOrEqual(0);
  for (let index = 0; index < hitIndex; index += 1) {
    await page.keyboard.press("ArrowDown");
  }
  await page.keyboard.press("Enter");
  await page.waitForURL((url) => decodeURIComponent(url.pathname) === docPath(id2));
  await expect(page.locator(".cm-content")).toBeVisible();

  // -- Step 7: selection QA -> streaming overlay ----------------------------
  // A user-style triple-click keeps the selection alive: CodeMirror (focused)
  // re-asserts its own collapsed selection and wipes a programmatically set
  // DOM range within one update cycle.
  await page
    .locator(".cm-line")
    .filter({ hasText: "Bar についての文書です" })
    .click({ clickCount: 3 });
  const qaButton = page.getByTestId("qa-floating-button");
  await expect(qaButton).toBeVisible();
  await qaButton.click();
  const qaOverlay = page.getByTestId("qa-overlay");
  // mock_llm's fixed stream text, rendered through MarkdownView.
  await expect(qaOverlay).toContainText("mock answer");
  await expect(qaOverlay.getByTestId("qa-citations")).toContainText("Bar");
  await page.getByTestId("qa-close").click();
  await expect(qaOverlay).toBeHidden();

  // -- Failure evidence: entity-less document shows the graph empty state ---
  const seededPlain = await request.post("/api/documents", {
    data: {
      title: "プレーン",
      content: "この文書には見出しもリンクもタグもありません。",
    },
  });
  expect(seededPlain.ok()).toBeTruthy();
  const doc3 = (await seededPlain.json()) as { document_id: string };
  await page.goto(docPath(doc3.document_id));
  await expect(page.locator(".cm-content")).toBeVisible();
  await page.getByRole("button", { name: "全画面グラフ" }).click();
  await expect(page.getByTestId("graph-empty")).toBeVisible();
  await page.screenshot({ path: path.join(evidenceDir, "failure.png"), fullPage: true });
});

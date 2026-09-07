import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { repoRoot } from "./helpers.mts";

/**
 * Studio chat e2e (plan todo 17). Self-contained seeding: every test mints a
 * unique term and POSTs a document containing it before sending it as the
 * chat message — cross-spec DB state is never relied on.
 *
 * Progressive-token assertion: the assistant bubble starts as the
 * 回答を生成中… hint with no .md-root, and tokens can only arrive via SSE
 * after the POST round trip, so the length sampled right after 送信 is the
 * "before" poll. mock_llm's three fragments land within one 100ms
 * MarkdownView throttle window on localhost, so intermediate fragment
 * boundaries are not deterministically paintable; growth between the two
 * polls plus the final fragment is the robust progressive signal.
 *
 * Files are .mts on purpose — see vault.spec.mts for the bun/Playwright
 * loader rationale.
 */

const STUDIO_URL = "http://localhost:5174/";
const evidenceDir = path.join(repoRoot, "target", "evidence", "17");

type DocumentResponse = { document_id: string };
type DocumentSummary = { id: string };

/** Rendered markdown length of the LAST assistant bubble (0 while the hint shows). */
async function lastAssistantMarkdownLength(page: Page): Promise<number> {
  return page.evaluate(() => {
    const bubbles = document.querySelectorAll("[data-testid='chat-bubble-assistant']");
    const root = bubbles[bubbles.length - 1]?.querySelector(".md-root");
    return root?.textContent?.length ?? 0;
  });
}

test("studio: streaming tokens, citation panel, and document link", async ({ page, request }) => {
  mkdirSync(evidenceDir, { recursive: true });

  // -- Step 1: seed a document whose content carries the chat term ----------
  // A `# <term>` heading mints a section entity, so the hit's
  // matched_entities (origin entities of the graph enrichment) is populated.
  const term = `StudioSeed${Date.now()}`;
  const seeded = await request.post(`${STUDIO_URL}api/documents`, {
    data: {
      title: `${term} の解説`,
      content: `# ${term}\n\n${term} とは、この e2e で使う固有語です。${term} explained in detail for the studio citation panel.`,
    },
  });
  expect(seeded.ok()).toBeTruthy();
  const doc = (await seeded.json()) as DocumentResponse;
  expect(doc.document_id).toMatch(/^document:/);

  // -- Step 2: open studio and send the seeded term --------------------------
  await page.goto(STUDIO_URL);
  const input = page.getByTestId("chat-input");
  await expect(input).toBeVisible();
  await input.fill(`${term} とは`);
  await page.getByTestId("chat-send").click();

  // -- Step 3: tokens render progressively -----------------------------------
  const beforeTokens = await lastAssistantMarkdownLength(page);
  await expect
    .poll(() => lastAssistantMarkdownLength(page), { timeout: 20_000 })
    .toBeGreaterThan(beforeTokens);
  // Stream completion: mock_llm's final fragment rendered through MarkdownView.
  await expect(page.getByTestId("chat-bubble-assistant").last()).toContainText(
    "end-to-end testing",
  );

  // -- Step 4: citation panel shows the seeded hit ---------------------------
  const panel = page.getByTestId("citation-panel");
  const item = panel.getByTestId("citation-item").filter({ hasText: term });
  await expect(item).toBeVisible();
  // なぜ引用されたか: the RRF score plus the heading entity as origin. The
  // space in the message keeps the term a standalone query term, so the
  // keyword leg's highlight filter matches the chunk body too.
  await expect(item.getByTestId("citation-entities")).toContainText(term);
  await expect(item).toContainText("スコア");
  await expect(item).toContainText(term.toLowerCase());

  await page.screenshot({ path: path.join(evidenceDir, "studio-citations.png"), fullPage: true });

  // -- Step 5: the citation link navigates to the vault doc route ------------
  // /doc/{id} belongs to the vault app; the URL is the contract under test.
  await item.getByRole("link").click();
  await page.waitForURL((url) => decodeURIComponent(url.pathname) === `/doc/${doc.document_id}`);
});

test("studio: citation panel empty state for a citation-less response", async ({
  page,
  request,
}) => {
  mkdirSync(evidenceDir, { recursive: true });

  // -- Step 1: make 0 hits reachable -----------------------------------------
  // Hybrid search's vector leg has no similarity threshold (and the mock
  // embedder maps every batch's first text to the same unit vector), so a
  // non-matching term still returns hits while ANY chunk exists. The
  // deterministic citation-less response is an empty chunk table: wipe every
  // document via the API, then send a message — search returns 0 hits and
  // the citation event carries an empty list.
  const listed = await request.get(`${STUDIO_URL}api/documents?limit=10000`);
  expect(listed.ok()).toBeTruthy();
  const documents = (await listed.json()) as DocumentSummary[];
  for (const document of documents) {
    const deleted = await request.delete(
      `${STUDIO_URL}api/documents/${encodeURIComponent(document.id)}`,
    );
    expect(deleted.ok()).toBeTruthy();
  }

  // -- Step 2: send a message that matches nothing ---------------------------
  await page.goto(STUDIO_URL);
  const input = page.getByTestId("chat-input");
  await expect(input).toBeVisible();
  await input.fill("存在しないトピックxyzzyについて教えて");
  await page.getByTestId("chat-send").click();

  // -- Step 3: the response completes normally, the panel stays empty --------
  await expect(page.getByTestId("chat-bubble-assistant").last()).toContainText(
    "end-to-end testing",
    { timeout: 20_000 },
  );
  await expect(page.getByTestId("citation-empty")).toBeVisible();
  await expect(page.getByTestId("citation-item")).toHaveCount(0);

  await page.screenshot({ path: path.join(evidenceDir, "failure.png"), fullPage: true });
});

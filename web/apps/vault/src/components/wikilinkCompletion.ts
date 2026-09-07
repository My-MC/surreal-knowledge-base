import type { CompletionContext, CompletionResult } from "@codemirror/autocomplete";

/**
 * Titles whose start matches the prefix, case-insensitively. Pure function so
 * the filtering rule is unit-tested without an editor.
 */
export function filterCompletions(prefix: string, titles: string[]): string[] {
  const needle = prefix.toLowerCase();
  return titles.filter((title) => title.toLowerCase().startsWith(needle));
}

/** Text right before the cursor: the `[[` opener plus the typed prefix. */
const WIKILINK_BEFORE_CURSOR = /\[\[([^\][]*)$/;

/** While the user keeps typing non-bracket characters the result stays valid. */
const WIKILINK_VALID_FOR = /^[^\][]*$/;

/**
 * CodeMirror completion source: typing `[[` pops up document titles. The
 * replacement starts after the brackets and the accepted title re-closes the
 * link, so `[[Al` + Enter yields `[[Alpha]]`.
 */
export function wikilinkCompletionSource(titles: () => string[]) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const before = ctx.matchBefore(WIKILINK_BEFORE_CURSOR);
    if (before === null) return null;
    const prefix = before.text.slice(2);
    const matches = filterCompletions(prefix, titles());
    if (matches.length === 0) return null;
    return {
      from: before.from + 2,
      options: matches.map((title) => ({
        label: title,
        apply: `${title}]]`,
        type: "text",
      })),
      validFor: WIKILINK_VALID_FOR,
    };
  };
}

import { describe, expect, test } from "bun:test";
import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { filterCompletions, wikilinkCompletionSource } from "./wikilinkCompletion";

describe("filterCompletions", () => {
  test("returns every title when the prefix is empty", () => {
    expect(filterCompletions("", ["Foo", "Bar"])).toEqual(["Foo", "Bar"]);
  });

  test("keeps only titles with a forward prefix match", () => {
    expect(filterCompletions("Ba", ["Bar", "Baz", "Foo"])).toEqual(["Bar", "Baz"]);
  });

  test("matches case-insensitively in both directions", () => {
    expect(filterCompletions("ba", ["Bar"])).toEqual(["Bar"]);
    expect(filterCompletions("BA", ["bar"])).toEqual(["bar"]);
  });

  test("returns nothing when no title matches", () => {
    expect(filterCompletions("zz", ["Foo", "Bar"])).toEqual([]);
  });
});

describe("wikilinkCompletionSource", () => {
  const source = wikilinkCompletionSource(() => ["Alpha", "Beta"]);

  const complete = (doc: string) => {
    const state = EditorState.create({ doc });
    return source(new CompletionContext(state, doc.length, false));
  };

  test("offers matching titles with the replacement starting past [[", () => {
    const result = complete("see [[Al");
    expect(result).not.toBeNull();
    expect(result?.from).toBe("see [[".length);
    expect(result?.options.map((option) => option.label)).toEqual(["Alpha"]);
    expect(result?.options[0]?.apply).toBe("Alpha]]");
  });

  test("offers every title right after typing [[", () => {
    const result = complete("[[");
    expect(result?.options.map((option) => option.label)).toEqual(["Alpha", "Beta"]);
  });

  test("returns null outside a wikilink", () => {
    expect(complete("plain text")).toBeNull();
  });

  test("returns null when no title matches the prefix", () => {
    expect(complete("[[Zz")).toBeNull();
  });
});

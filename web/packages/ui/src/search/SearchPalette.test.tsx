import { afterEach, describe, expect, jest, test } from "bun:test";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { SearchHit } from "./createSearch";
import { SearchPalette } from "./SearchPalette";

function hit(overrides?: Partial<SearchHit>): SearchHit {
  return {
    document_id: "document:doc-1",
    chunk_idx: 0,
    content: "first chunk content",
    score: 0.75,
    title: "Doc 1",
    ...overrides,
  };
}

function renderPalette(props?: Partial<Parameters<typeof SearchPalette>[0]>) {
  const onSelect = jest.fn();
  const onClose = jest.fn();
  const search = jest.fn(async (query: string) => [hit({ content: `result for ${query}` })]);
  const view = render(
    <SearchPalette
      open={props?.open ?? true}
      onSelect={props?.onSelect ?? onSelect}
      onClose={props?.onClose ?? onClose}
      search={props?.search ?? search}
    />,
  );
  return { view, onSelect, onClose, search };
}

// The search promise resolves in microtasks after the debounce timer fires;
// async act drains those microtasks so setHits lands inside act.
async function advance(ms: number): Promise<void> {
  await act(async () => {
    jest.advanceTimersByTime(ms);
  });
}

describe("SearchPalette", () => {
  afterEach(() => cleanup());

  test("Cmd+K opens and Ctrl+K toggles closed (onClose fires on self-close)", () => {
    const { onClose } = renderPalette({ open: false });
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent.keyDown(document, { key: "k", metaKey: true });
    expect(screen.getByRole("dialog")).toBeDefined();
    fireEvent.keyDown(document, { key: "k", ctrlKey: true });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test("Esc closes and calls onClose", () => {
    const { onClose } = renderPalette();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  test("open prop changes re-sync the palette", () => {
    const { view } = renderPalette({ open: false });
    expect(screen.queryByRole("dialog")).toBeNull();
    view.rerender(
      <SearchPalette open onSelect={jest.fn()} onClose={jest.fn()} search={jest.fn()} />,
    );
    expect(screen.getByRole("dialog")).toBeDefined();
  });

  test("typing debounces: search runs once, 250ms after the last keystroke", async () => {
    jest.useFakeTimers();
    try {
      const { search } = renderPalette();
      const input = screen.getByRole("textbox");
      fireEvent.change(input, { target: { value: "s" } });
      fireEvent.change(input, { target: { value: "su" } });
      fireEvent.change(input, { target: { value: "sur" } });
      expect(search).not.toHaveBeenCalled();
      await advance(249);
      expect(search).not.toHaveBeenCalled();
      await advance(1);
      expect(search).toHaveBeenCalledTimes(1);
      expect(search).toHaveBeenCalledWith("sur");
    } finally {
      jest.useRealTimers();
    }
  });

  test("renders hits with title, content head, and score after the debounce", async () => {
    jest.useFakeTimers();
    try {
      const search = jest.fn(async () => [hit({ score: 0.75, content: "first chunk content" })]);
      renderPalette({ search });
      const input = screen.getByRole("textbox");
      fireEvent.change(input, { target: { value: "doc" } });
      await advance(250);
      expect(screen.getByText("Doc 1")).toBeDefined();
      expect(screen.getByText("first chunk content")).toBeDefined();
      expect(screen.getByText("0.750")).toBeDefined();
    } finally {
      jest.useRealTimers();
    }
  });

  test("shows the empty-state message when the search returns no hits", async () => {
    jest.useFakeTimers();
    try {
      const search = jest.fn(async () => []);
      renderPalette({ search });
      const input = screen.getByRole("textbox");
      fireEvent.change(input, { target: { value: "nothing" } });
      await advance(250);
      expect(screen.getByText("結果なし")).toBeDefined();
    } finally {
      jest.useRealTimers();
    }
  });

  test("ArrowDown/ArrowUp move the selection and Enter selects the hit", async () => {
    jest.useFakeTimers();
    try {
      const hits = [
        hit(),
        hit({ document_id: "document:doc-2", title: "Doc 2", chunk_idx: 1 }),
        hit({ document_id: "document:doc-3", title: "Doc 3", chunk_idx: 2 }),
      ];
      const search = jest.fn(async () => hits);
      const { onSelect } = renderPalette({ search });
      const input = screen.getByRole("textbox");
      fireEvent.change(input, { target: { value: "doc" } });
      await advance(250);

      expect(screen.getAllByRole("button")[0]?.className).toContain("is-selected");

      fireEvent.keyDown(input, { key: "ArrowDown" });
      const buttons = screen.getAllByRole("button");
      expect(buttons[1]?.className).toContain("is-selected");
      expect(buttons[0]?.className).not.toContain("is-selected");

      fireEvent.keyDown(input, { key: "ArrowDown" });
      expect(screen.getAllByRole("button")[2]?.className).toContain("is-selected");

      // ArrowDown at the last hit stays there.
      fireEvent.keyDown(input, { key: "ArrowDown" });
      expect(screen.getAllByRole("button")[2]?.className).toContain("is-selected");

      fireEvent.keyDown(input, { key: "ArrowUp" });
      expect(screen.getAllByRole("button")[1]?.className).toContain("is-selected");

      fireEvent.keyDown(input, { key: "Enter" });
      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith(hits[1]);
      expect(screen.queryByRole("dialog")).toBeNull(); // selection closes the palette
    } finally {
      jest.useRealTimers();
    }
  });

  test("ArrowUp at the first hit stays there and Enter without hits does nothing", async () => {
    jest.useFakeTimers();
    try {
      const { onSelect, search } = renderPalette();
      const input = screen.getByRole("textbox");
      fireEvent.keyDown(input, { key: "ArrowUp" });
      fireEvent.keyDown(input, { key: "Enter" });
      expect(onSelect).not.toHaveBeenCalled();

      fireEvent.change(input, { target: { value: "doc" } });
      await advance(250);
      expect(search).toHaveBeenCalledTimes(1);
      expect(screen.getAllByRole("button")[0]?.className).toContain("is-selected");
      fireEvent.keyDown(input, { key: "ArrowUp" });
      expect(screen.getAllByRole("button")[0]?.className).toContain("is-selected");
      expect(onSelect).not.toHaveBeenCalled();
    } finally {
      jest.useRealTimers();
    }
  });

  test("clicking a hit selects it and closes the palette", async () => {
    jest.useFakeTimers();
    try {
      const { onSelect, onClose } = renderPalette();
      const input = screen.getByRole("textbox");
      fireEvent.change(input, { target: { value: "doc" } });
      await advance(250);
      fireEvent.click(screen.getAllByRole("button")[0] as HTMLElement);
      expect(onSelect).toHaveBeenCalledTimes(1);
      expect(onSelect).toHaveBeenCalledWith(hit({ content: "result for doc" }));
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(screen.queryByRole("dialog")).toBeNull();
    } finally {
      jest.useRealTimers();
    }
  });
});

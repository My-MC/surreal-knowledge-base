import { autocompletion } from "@codemirror/autocomplete";
import { markdown } from "@codemirror/lang-markdown";
import { MarkdownView } from "@skb/ui";
import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import CodeMirror from "@uiw/react-codemirror";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { documentsQuery } from "../api";
import styles from "./DocumentEditor.module.css";
import { QaOverlay } from "./QaOverlay";
import { SaveStatusIndicator } from "./SaveStatusIndicator";
import { useAutosave } from "./useAutosave";
import { wikilinkCompletionSource } from "./wikilinkCompletion";

type EditorSurfaceProps = {
  doc: {
    id: string;
    content: string;
    title: string;
  };
};

/**
 * Center pane for one loaded document: Editor/Preview tabs, wikilink
 * completion, and the autosave status. Stays mounted across autosave
 * rotations — the content equality check in the sync effect keeps the
 * CodeMirror cursor untouched when the PUT response re-points the route.
 */
export function EditorSurface({ doc }: EditorSurfaceProps) {
  const [tab, setTab] = useState<"editor" | "preview">("editor");
  const [content, setContent] = useState(doc.content);
  const contentRef = useRef(doc.content);
  const navigate = useNavigate();
  const { status, schedule, retry } = useAutosave(doc.id);

  const editorShellRef = useRef<HTMLDivElement | null>(null);
  const qaTriggerRef = useRef<HTMLButtonElement | null>(null);
  const [selection, setSelection] = useState("");
  const [qaQuestion, setQaQuestion] = useState<string | null>(null);
  const qaOpenRef = useRef(false);
  qaOpenRef.current = qaQuestion !== null;

  // Track editor-scoped text selection for the QA floating button. While the
  // QA overlay is open the captured selection is frozen (the overlay's own
  // focus changes would otherwise collapse it).
  useEffect(() => {
    const onSelectionChange = () => {
      if (qaOpenRef.current) return;
      const domSelection = window.getSelection();
      const text = domSelection?.toString() ?? "";
      const anchor = domSelection?.anchorNode;
      const container = editorShellRef.current;
      const inEditor =
        anchor !== null && anchor !== undefined && (container?.contains(anchor) ?? false);
      setSelection(inEditor && text.trim() !== "" ? text : "");
    };
    document.addEventListener("selectionchange", onSelectionChange);
    return () => document.removeEventListener("selectionchange", onSelectionChange);
  }, []);

  useEffect(() => {
    if (doc.content === contentRef.current) return;
    contentRef.current = doc.content;
    setContent(doc.content);
  }, [doc]);

  const { data: docs } = useQuery(documentsQuery());
  const titlesRef = useRef<string[]>([]);
  useEffect(() => {
    titlesRef.current = (docs ?? []).map((entry) => entry.title);
  }, [docs]);

  const extensions = useMemo(
    () => [
      markdown(),
      autocompletion({ override: [wikilinkCompletionSource(() => titlesRef.current)] }),
    ],
    [],
  );

  const handleChange = useCallback(
    (value: string) => {
      contentRef.current = value;
      setContent(value);
      schedule({ content: value, title: doc.title });
    },
    [schedule, doc.title],
  );

  const titleToId = useMemo(
    () => new Map((docs ?? []).map((entry) => [entry.title, entry.id])),
    [docs],
  );

  const onPreviewClick = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      const target = event.target instanceof HTMLElement ? event.target : null;
      const anchor = target?.closest("a[data-wikilink]") ?? null;
      if (anchor === null) return;
      event.preventDefault();
      const title = anchor.getAttribute("data-wikilink");
      if (title === null) return;
      const id = titleToId.get(title);
      // A title missing from the cached list resolves to no document; the
      // anchor stays inert instead of navigating to a dead route.
      if (id === undefined) return;
      void navigate({ to: "/doc/$id", params: { id } });
    },
    [titleToId, navigate],
  );

  return (
    <div className={styles.editor}>
      <div className={styles.toolbar}>
        <div className={styles.tabs} role="tablist" aria-label="表示切替">
          <button
            type="button"
            role="tab"
            aria-selected={tab === "editor"}
            className={tab === "editor" ? `${styles.tab} ${styles.tabActive}` : styles.tab}
            onClick={() => setTab("editor")}
          >
            エディタ
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={tab === "preview"}
            className={tab === "preview" ? `${styles.tab} ${styles.tabActive}` : styles.tab}
            onClick={() => setTab("preview")}
          >
            プレビュー
          </button>
        </div>
        <SaveStatusIndicator status={status} onRetry={retry} />
      </div>
      {tab === "editor" ? (
        <div className={styles.editorShell} ref={editorShellRef}>
          <CodeMirror
            value={content}
            extensions={extensions}
            onChange={handleChange}
            placeholder="Markdownで文書を入力… [[ で文書タイトルを補完"
            basicSetup={{ autocompletion: false }}
            className={styles.codemirror}
          />
          {selection !== "" && qaQuestion === null && (
            <button
              type="button"
              ref={qaTriggerRef}
              className={styles.qaFloating}
              data-testid="qa-floating-button"
              // preventDefault keeps the DOM selection alive through the
              // click — a plain mousedown would collapse it and unmount
              // this button before onClick fires.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => {
                setQaQuestion(
                  `文書「${doc.title}」の次の選択範囲について説明してください:\n\n${selection}`,
                );
              }}
            >
              選択範囲について質問
            </button>
          )}
        </div>
      ) : (
        // biome-ignore lint/a11y/noStaticElementInteractions: click delegation on rendered markdown; the interactive targets are native wikilink anchors with Space handling in packages/ui
        // biome-ignore lint/a11y/useKeyWithClickEvents: same delegation container; wikilinks re-dispatch keyboard activation as real clicks, so a keydown handler here would double-fire
        <div className={styles.preview} onClick={onPreviewClick}>
          <MarkdownView content={content} />
        </div>
      )}
      {qaQuestion !== null && (
        <QaOverlay
          question={qaQuestion}
          onClose={() => setQaQuestion(null)}
          triggerRef={qaTriggerRef}
        />
      )}
    </div>
  );
}

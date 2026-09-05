import {
  type ChangeEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createSearch, type SearchFn, type SearchHit } from "./createSearch";

const DEBOUNCE_MS = 250;
const CONTENT_HEAD_LEN = 80;
const DEFAULT_SEARCH: SearchFn = createSearch();

export interface SearchPaletteProps {
  /** Called with the chosen hit (Enter or click); the palette closes after. */
  onSelect: (hit: SearchHit) => void;
  /** Syncs the open state from the parent (initial value + prop changes). */
  open: boolean;
  /** Called whenever the palette closes itself (Esc / Cmd+K / selection). */
  onClose: () => void;
  /** Search fn; defaults to the api-client hybrid search (top_k 8). */
  search?: SearchFn;
}

/**
 * Cmd+K / Ctrl+K command palette over `POST /api/search` (hybrid, top_k 8).
 *
 * Open-state model (documented choice): the palette owns its open state,
 * initialized from and re-synced on `open` prop changes; Cmd+K toggles it
 * internally, and every self-initiated close (Esc, Cmd+K, selection) invokes
 * `onClose` so the parent can keep its own state in sync. Apps load
 * tokens.css and `@skb/ui/src/search/search.css` themselves — CSS is
 * intentionally not imported from the TSX (bun test safety, T9/T11
 * convention).
 */
export function SearchPalette({
  onSelect,
  open,
  onClose,
  search = DEFAULT_SEARCH,
}: SearchPaletteProps) {
  const [isOpen, setIsOpen] = useState(open);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [selected, setSelected] = useState(0);

  const isOpenRef = useRef(isOpen);
  isOpenRef.current = isOpen;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const searchRef = useRef(search);
  searchRef.current = search;
  const seqRef = useRef(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const invalidateSearch = useCallback((): void => {
    seqRef.current += 1;
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  // Re-sync when the parent drives the state.
  useLayoutEffect(() => {
    if (!open) invalidateSearch();
    setIsOpen(open);
  }, [open, invalidateSearch]);

  // Fresh query state per open; on close, invalidate any in-flight search and
  // drop a pending debounce so stale results can never land after close.
  useEffect(() => {
    if (isOpen) {
      setQuery("");
      setHits([]);
      setSelected(0);
    }
  }, [isOpen]);

  // Focus the input on open (autoFocus is banned by biome a11y).
  useEffect(() => {
    if (isOpen) inputRef.current?.focus();
  }, [isOpen]);

  // Global shortcuts: Cmd/Ctrl+K toggles, Esc closes. Bound once; the latest
  // callbacks are reached through refs.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (event.repeat) return;
        const next = !isOpenRef.current;
        if (!next) invalidateSearch();
        setIsOpen(next);
        if (!next) onCloseRef.current();
        return;
      }
      if (event.key === "Escape" && isOpenRef.current) {
        invalidateSearch();
        setIsOpen(false);
        onCloseRef.current();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [invalidateSearch]);

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) clearTimeout(timerRef.current);
    };
  }, []);

  const runSearch = async (value: string): Promise<void> => {
    const seq = ++seqRef.current;
    try {
      const results = await searchRef.current(value);
      if (seq !== seqRef.current) return; // a newer search superseded this one
      setHits(results);
      setSelected(0);
    } catch {
      if (seq !== seqRef.current) return;
      setHits([]);
    }
  };

  const onInputChange = (event: ChangeEvent<HTMLInputElement>) => {
    const value = event.target.value;
    // Invalidate any in-flight search before the state transition: its
    // results belong to the previous input and must never reach setHits.
    invalidateSearch();
    setQuery(value);
    if (value === "") {
      setHits([]);
      return;
    }
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      void runSearch(value);
    }, DEBOUNCE_MS);
  };

  const selectHit = (hit: SearchHit): void => {
    invalidateSearch();
    onSelectRef.current(hit);
    setIsOpen(false);
    onCloseRef.current();
  };

  const onInputKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" && hits.length > 0) {
      event.preventDefault();
      setSelected((index) => Math.min(index + 1, hits.length - 1));
    } else if (event.key === "ArrowUp" && hits.length > 0) {
      event.preventDefault();
      setSelected((index) => Math.max(index - 1, 0));
    } else if (event.key === "Enter") {
      const hit = hits[selected];
      if (hit !== undefined) selectHit(hit);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="skb-palette" role="dialog" aria-label="検索パレット">
      <input
        ref={inputRef}
        className="skb-palette-input"
        type="text"
        value={query}
        placeholder="検索…"
        onChange={onInputChange}
        onKeyDown={onInputKeyDown}
      />
      {hits.length === 0 ? (
        query === "" ? null : (
          <div className="skb-palette-empty">結果なし</div>
        )
      ) : (
        <ul className="skb-palette-list">
          {hits.map((hit, index) => (
            <li key={`${hit.document_id}:${hit.chunk_idx}`} className="skb-palette-item">
              <button
                type="button"
                className={index === selected ? "skb-palette-hit is-selected" : "skb-palette-hit"}
                onClick={() => selectHit(hit)}
              >
                <span className="skb-palette-hit-title">{hit.title ?? hit.document_id}</span>
                <span className="skb-palette-hit-content">
                  {hit.content.slice(0, CONTENT_HEAD_LEN)}
                </span>
                <span className="skb-palette-hit-score">{hit.score.toFixed(3)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

import type { components } from "@skb/api-client";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { api, documentQuery, toApiError } from "../api";

export const AUTOSAVE_DEBOUNCE_MS = 500;

export type SaveInput = {
  content: string;
  title: string;
};

export type SaveStatus =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved"; at: Date }
  | { kind: "error"; error: Error };

type DocumentDetail = components["schemas"]["DocumentDetailResponse"];

/**
 * A save bound to the editing session that produced it: the target document
 * and the generation counter are captured at dispatch time, so a request can
 * never carry one document's input into another document's PUT, and results
 * arriving after an external navigation are recognized as stale.
 */
type SaveRequest = {
  docId: string;
  generation: number;
  input: SaveInput;
};

type SaveDraft = Omit<SaveRequest, "docId">;

/**
 * Debounced autosave for the routed document.
 *
 * Every edit reschedules a PUT 500ms out (trailing debounce). The PUT target
 * id is captured when the debounce fires, so a debounce scheduled before our
 * own save rotated the route still hits the current document. PUTs are
 * serialized through an explicit one-in-flight queue: an edit landing while a
 * PUT is running replaces the queued input, and the latest queued input is
 * dispatched when the running PUT settles (its target re-captured then, so a
 * rotation mid-queue is followed correctly). On a rotation response (new
 * document_id — the server deleted the old document) the route is
 * replace-navigated and the query cache for the new id is seeded with the
 * saved content so the editor never unmounts (cursor would reset). An
 * external navigation away from the document bumps the generation and drops
 * the pending debounce: stale saves then no-op in onSuccess/onError/onSettled
 * and a leftover retry can never fire another document's input.
 */
export function useAutosave(docId: string) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [status, setStatus] = useState<SaveStatus>({ kind: "idle" });

  const docIdRef = useRef(docId);
  const generationRef = useRef(0);
  const pendingRef = useRef<SaveDraft | null>(null);
  const queuedRef = useRef<SaveDraft | null>(null);
  const inFlightRef = useRef(false);
  const lastRequestRef = useRef<SaveRequest | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rotatedRef = useRef(false);

  useLayoutEffect(() => {
    if (docIdRef.current === docId) return;
    docIdRef.current = docId;
    if (rotatedRef.current) {
      rotatedRef.current = false;
      return;
    }
    generationRef.current += 1;
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    pendingRef.current = null;
    setStatus({ kind: "idle" });
  }, [docId]);

  useLayoutEffect(() => {
    return () => {
      generationRef.current += 1;
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
    };
  }, []);

  const isCurrent = useCallback(
    (request: SaveRequest) =>
      request.docId === docIdRef.current && request.generation === generationRef.current,
    [],
  );

  const { mutate: mutateSave } = useMutation({
    mutationFn: async (request: SaveRequest) => {
      const { data, error, response } = await api.PUT("/api/documents/{id}", {
        params: { path: { id: request.docId } },
        body: { content: request.input.content, title: request.input.title },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return { document_id: data.document_id, request };
    },
    onSuccess: async (result, request) => {
      if (!isCurrent(request)) return;
      if (result.document_id !== request.docId) {
        rotatedRef.current = true;
        const oldDoc = queryClient.getQueryData<DocumentDetail>(
          documentQuery(request.docId).queryKey,
        );
        const seeded: DocumentDetail = {
          id: result.document_id,
          content: request.input.content,
          title: oldDoc?.title ?? request.input.title,
          created_at: oldDoc?.created_at ?? "",
          sha256: oldDoc?.sha256 ?? "",
          source: oldDoc?.source ?? "",
          source_type: oldDoc?.source_type ?? "",
          chunks: oldDoc?.chunks ?? null,
        };
        queryClient.setQueryData(documentQuery(result.document_id).queryKey, seeded);
        await queryClient.invalidateQueries({ queryKey: ["documents"] });
        if (!isCurrent(request)) return;
        setStatus({ kind: "saved", at: new Date() });
        await navigate({
          to: "/doc/$id",
          params: { id: result.document_id },
          replace: true,
        });
        return;
      }
      setStatus({ kind: "saved", at: new Date() });
    },
    onError: (error, request) => {
      if (!isCurrent(request)) return;
      setStatus({ kind: "error", error });
    },
    onSettled: (_result, _error, _request) => {
      const queued = queuedRef.current;
      queuedRef.current = null;
      // Chain the latest queued edit into a follow-up PUT, but only while the
      // editing session that queued it is still live. The target id is
      // re-captured here so both an intervening rotation and an edit queued
      // by a newly navigated document are sent to the correct document.
      if (queued !== null && queued.generation === generationRef.current) {
        lastRequestRef.current = {
          docId: docIdRef.current,
          generation: queued.generation,
          input: queued.input,
        };
        setStatus({ kind: "saving" });
        mutateSave(lastRequestRef.current);
        return;
      }
      inFlightRef.current = false;
    },
  });

  const dispatch = useCallback(
    (draft: SaveDraft) => {
      if (draft.generation !== generationRef.current) return;
      if (inFlightRef.current) {
        queuedRef.current = draft;
        return;
      }
      const request: SaveRequest = {
        docId: docIdRef.current,
        generation: draft.generation,
        input: draft.input,
      };
      lastRequestRef.current = request;
      inFlightRef.current = true;
      mutateSave(request);
    },
    [mutateSave],
  );

  const schedule = useCallback(
    (input: SaveInput) => {
      pendingRef.current = { generation: generationRef.current, input };
      setStatus({ kind: "saving" });
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        const pending = pendingRef.current;
        pendingRef.current = null;
        if (pending === null) return;
        dispatch(pending);
      }, AUTOSAVE_DEBOUNCE_MS);
    },
    [dispatch],
  );

  const retry = useCallback(() => {
    const last = lastRequestRef.current;
    if (last === null) return;
    // A retry is only meaningful for the live session: after navigating to
    // another document the failed request's input must never be PUT there.
    if (!isCurrent(last)) return;
    setStatus({ kind: "saving" });
    inFlightRef.current = true;
    mutateSave(last);
  }, [isCurrent, mutateSave]);

  return { status, schedule, retry };
}

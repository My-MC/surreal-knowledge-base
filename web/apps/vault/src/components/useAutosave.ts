import type { components } from "@skb/api-client";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useCallback, useEffect, useRef, useState } from "react";
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
 * Debounced autosave for the routed document.
 *
 * Every edit reschedules a PUT 500ms out (trailing debounce). The PUT target
 * id is read from a ref at fire time, so a debounce scheduled before our own
 * save rotated the route still hits the current document. On a rotation
 * response (new document_id — the server deleted the old document) the route
 * is replace-navigated and the query cache for the new id is seeded with the
 * saved content so the editor never unmounts (cursor would reset). An
 * external navigation away from the document drops the pending debounce so
 * one document's edits are never PUT into another.
 */
export function useAutosave(docId: string) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [status, setStatus] = useState<SaveStatus>({ kind: "idle" });

  const docIdRef = useRef(docId);
  const pendingRef = useRef<SaveInput | null>(null);
  const lastInputRef = useRef<SaveInput | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const rotatedRef = useRef(false);

  useEffect(() => {
    docIdRef.current = docId;
    if (rotatedRef.current) {
      rotatedRef.current = false;
      return;
    }
    if (timerRef.current !== null) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    pendingRef.current = null;
  }, [docId]);

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
    };
  }, []);

  const { mutate: mutateSave } = useMutation({
    mutationFn: async (input: SaveInput) => {
      const targetId = docIdRef.current;
      const { data, error, response } = await api.PUT("/api/documents/{id}", {
        params: { path: { id: targetId } },
        body: { content: input.content, title: input.title },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return { document_id: data.document_id, targetId };
    },
    onSuccess: async (result, input) => {
      if (result.document_id !== result.targetId) {
        rotatedRef.current = true;
        const oldDoc = queryClient.getQueryData<DocumentDetail>(
          documentQuery(result.targetId).queryKey,
        );
        const seeded: DocumentDetail = {
          id: result.document_id,
          content: input.content,
          title: oldDoc?.title ?? input.title,
          created_at: oldDoc?.created_at ?? "",
          sha256: oldDoc?.sha256 ?? "",
          source: oldDoc?.source ?? "",
          source_type: oldDoc?.source_type ?? "",
          chunks: oldDoc?.chunks ?? null,
        };
        queryClient.setQueryData(documentQuery(result.document_id).queryKey, seeded);
        await queryClient.invalidateQueries({ queryKey: ["documents"] });
        await navigate({
          to: "/doc/$id",
          params: { id: result.document_id },
          replace: true,
        });
      }
      setStatus({ kind: "saved", at: new Date() });
    },
    onError: (error) => {
      setStatus({ kind: "error", error });
    },
  });

  const schedule = useCallback(
    (input: SaveInput) => {
      pendingRef.current = input;
      setStatus({ kind: "saving" });
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        const pending = pendingRef.current;
        pendingRef.current = null;
        if (pending === null) return;
        lastInputRef.current = pending;
        mutateSave(pending);
      }, AUTOSAVE_DEBOUNCE_MS);
    },
    [mutateSave],
  );

  const retry = useCallback(() => {
    const last = lastInputRef.current;
    if (last === null) return;
    setStatus({ kind: "saving" });
    mutateSave(last);
  }, [mutateSave]);

  return { status, schedule, retry };
}

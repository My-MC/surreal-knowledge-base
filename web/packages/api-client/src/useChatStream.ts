import { useCallback, useEffect, useRef, useState } from "react";

import { consumeSseStream, type SearchHit } from "./sse";

export type ChatStreamStatus = "idle" | "streaming" | "done" | "error";

export interface ChatStreamError {
  code: string;
  message: string;
}

export interface ChatStreamController {
  start: (message: string) => Promise<void>;
  stop: () => void;
  status: ChatStreamStatus;
  tokens: string;
  citations: SearchHit[];
  error: ChatStreamError | null;
}

export function useChatStream(baseUrl: string): ChatStreamController {
  const [status, setStatus] = useState<ChatStreamStatus>("idle");
  const [tokens, setTokens] = useState("");
  const [citations, setCitations] = useState<SearchHit[]>([]);
  const [error, setError] = useState<ChatStreamError | null>(null);
  const controllerRef = useRef<AbortController | null>(null);

  const stop = useCallback(() => {
    controllerRef.current?.abort();
  }, []);

  const start = useCallback(
    async (message: string) => {
      controllerRef.current?.abort();
      const controller = new AbortController();
      controllerRef.current = controller;
      setTokens("");
      setCitations([]);
      setError(null);
      setStatus("streaming");
      try {
        const response = await fetch(`${baseUrl}/api/chat/stream`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ message }),
          signal: controller.signal,
        });
        if (!response.ok || response.body === null) {
          setError(await errorFromResponse(response));
          setStatus("error");
          return;
        }
        await consumeSseStream(
          response,
          {
            onCitation: (hits) => setCitations(hits),
            onToken: (text) => setTokens((prev) => prev + text),
            onDone: () => setStatus("done"),
            onError: (code, errorMessage) => {
              setError({ code, message: errorMessage });
              setStatus("error");
            },
          },
          controller.signal,
        );
      } catch (e) {
        if (!controller.signal.aborted) {
          setError({ code: "E_NETWORK", message: e instanceof Error ? e.message : String(e) });
          setStatus("error");
        }
      } finally {
        if (controllerRef.current === controller) {
          controllerRef.current = null;
          // Settle streaming -> idle on abort or premature EOF; done/error keep theirs.
          setStatus((prev) => (prev === "streaming" ? "idle" : prev));
        }
      }
    },
    [baseUrl],
  );

  useEffect(() => {
    return () => {
      controllerRef.current?.abort();
    };
  }, []);

  return { start, stop, status, tokens, citations, error };
}

async function errorFromResponse(response: Response): Promise<ChatStreamError> {
  const fallback = {
    code: `E_HTTP_${response.status}`,
    message: `chat stream request failed with HTTP ${response.status}`,
  };
  try {
    const body: unknown = await response.json();
    if (isErrorBody(body)) return { code: body.code, message: body.message };
  } catch {
    // non-JSON body — fall through to the HTTP-status fallback
  }
  return fallback;
}

function isErrorBody(body: unknown): body is { code: string; message: string } {
  return (
    typeof body === "object" &&
    body !== null &&
    "code" in body &&
    typeof body.code === "string" &&
    "message" in body &&
    typeof body.message === "string"
  );
}

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { api, toApiError } from "../api";

/**
 * POST /api/documents {content: "# Untitled"} then navigate to the new
 * document. Shared by the tree header button and the "/" empty state.
 */
export function useCreateDocument() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const mutation = useMutation({
    mutationFn: async () => {
      const { data, error, response } = await api.POST("/api/documents", {
        body: { content: "# Untitled" },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      return data;
    },
    onSuccess: async (result) => {
      await queryClient.invalidateQueries({ queryKey: ["documents"] });
      if (result.document_id) {
        await navigate({ to: "/doc/$id", params: { id: result.document_id } });
        return;
      }
      // status "skipped": identical content already ingested and the response
      // carries no id — "/" redirects to the latest document instead.
      await navigate({ to: "/" });
    },
  });

  return { createDocument: () => mutation.mutate(), isPending: mutation.isPending };
}

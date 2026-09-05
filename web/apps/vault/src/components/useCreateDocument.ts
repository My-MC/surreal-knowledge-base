import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { ApiError, api, toApiError } from "../api";

/**
 * POST /api/documents then navigate to the new document. Shared by the tree
 * header button and the "/" empty state.
 *
 * The initial content carries a creation timestamp because the server dedups
 * uploads by sha256: a fixed "# Untitled" would make the second and later
 * creations answer `skipped` with no document_id. A skipped (null-id)
 * response is never a successful creation, so it throws instead of
 * navigating.
 */
export function useCreateDocument() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const mutation = useMutation({
    mutationFn: async () => {
      const { data, error, response } = await api.POST("/api/documents", {
        body: { content: `# Untitled\n\n${new Date().toISOString()}` },
      });
      if (error !== undefined || data === undefined) {
        throw toApiError(error, response.status);
      }
      if (data.document_id === null || data.document_id === undefined) {
        throw new ApiError(
          "E_CREATE_SKIPPED",
          "ドキュメントは作成されませんでした（同一内容が既に存在します）",
        );
      }
      return data.document_id;
    },
    onSuccess: async (documentId) => {
      await queryClient.invalidateQueries({ queryKey: ["documents"] });
      await navigate({ to: "/doc/$id", params: { id: documentId } });
    },
  });

  return { createDocument: () => mutation.mutate(), isPending: mutation.isPending };
}

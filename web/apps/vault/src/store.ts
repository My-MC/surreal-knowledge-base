import { create } from "zustand";

/** UI-only state; server data lives in TanStack Query, not here. */
type VaultUiState = {
  selectedDocId: string | null;
  selectDoc: (id: string | null) => void;
};

export const useVaultStore = create<VaultUiState>()((set) => ({
  selectedDocId: null,
  selectDoc: (selectedDocId) => set({ selectedDocId }),
}));

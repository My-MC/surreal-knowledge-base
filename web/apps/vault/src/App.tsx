import { SearchPalette } from "@skb/ui";
import { Outlet, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { searchHits } from "./api";
import { DocPane } from "./components/DocPane";
import { DocumentTree } from "./components/DocumentTree";
import "./vault.css";

/**
 * App shell: left document tree / center route outlet / right backlinks pane.
 * The Cmd+K palette lives at the shell level so it works on every route;
 * hit ids are already `document:`-prefixed record ids, matching /doc/$id.
 */
export function AppLayout() {
  const navigate = useNavigate();
  const [paletteOpen, setPaletteOpen] = useState(false);
  return (
    <div className="vault-layout">
      <aside className="vault-sidebar">
        <DocumentTree />
      </aside>
      <main className="vault-main">
        <Outlet />
      </main>
      <aside className="vault-aside">
        <DocPane />
      </aside>
      <SearchPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onSelect={(hit) => void navigate({ to: "/doc/$id", params: { id: hit.document_id } })}
        search={searchHits}
      />
    </div>
  );
}

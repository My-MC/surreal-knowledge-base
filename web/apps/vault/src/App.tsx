import { Outlet } from "@tanstack/react-router";
import { DocumentTree } from "./components/DocumentTree";
import "./vault.css";

/** App shell: left document tree / center route outlet / right backlinks pane. */
export function AppLayout() {
  return (
    <div className="vault-layout">
      <aside className="vault-sidebar">
        <DocumentTree />
      </aside>
      <main className="vault-main">
        <Outlet />
      </main>
      <aside className="vault-aside">
        <h2 className="vault-aside-title">バックリンク</h2>
        <p className="vault-aside-hint">todo 15 で実装されます</p>
      </aside>
    </div>
  );
}

import type { ReactNode } from "react";

/** Old reddit's two columns: the listing on the left, a 300px sidebar on the right. */
export function PageBody({ side, children }: { side?: ReactNode; children: ReactNode }) {
  return (
    <div className="page-body">
      <main className="content">{children}</main>
      {side && <aside className="side">{side}</aside>}
    </div>
  );
}

export function SideBox({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section className="spacer sidecontentbox">
      {title && <h2 className="sidebox-title">{title}</h2>}
      <div className="sidebox-content">{children}</div>
    </section>
  );
}

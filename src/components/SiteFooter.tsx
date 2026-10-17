import { getDataSource } from "@/lib/datasource";

export function SiteFooter() {
  return (
    <footer className="footer">
      <p>
        <strong>allthing</strong> is a read-only Farcaster client · data from {getDataSource().name} ·{" "}
        <a href="https://github.com/ael-dev3/allthing">source</a>
      </p>
      <p className="gray">
        keyboard: <kbd>j</kbd>/<kbd>k</kbd> move · <kbd>o</kbd> open · <kbd>x</kbd> collapse
      </p>
    </footer>
  );
}

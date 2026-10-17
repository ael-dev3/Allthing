import { PageBody } from "./PageBody";
import { SiteHeader } from "./SiteHeader";

/** Static shell shown while a page's data streams in. */
export function PageFallback({ pageName }: { pageName?: string }) {
  return (
    <>
      <SiteHeader pageName={pageName} />
      <PageBody>
        <p className="loading" role="status">
          loading…
        </p>
      </PageBody>
    </>
  );
}

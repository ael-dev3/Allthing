"use client";

import Link from "next/link";
import { useEffect } from "react";
import { PageBody } from "@/components/PageBody";
import { SiteHeader } from "@/components/SiteHeader";

export default function ErrorPage({ error, retry }: { error: unknown; retry: () => void }) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <>
      <SiteHeader pageName="error" />
      <PageBody>
        <div className="error-page">
          <h1>something went wrong</h1>
          <p>we couldn&apos;t load this page from the Farcaster API. it may be busy; try again in a moment.</p>
          <p>
            <button type="button" onClick={() => retry()}>
              try again
            </button>{" "}
            or go to the <Link href="/">front page</Link>
          </p>
        </div>
      </PageBody>
    </>
  );
}

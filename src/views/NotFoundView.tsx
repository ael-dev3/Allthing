import Link from "next/link";
import { PageBody } from "@/components/PageBody";
import { SiteHeader } from "@/components/SiteHeader";
import { homeRoute, hrefFor } from "@/lib/links";

export function NotFoundView() {
  return (
    <>
      <SiteHeader pageName="page not found" />
      <PageBody>
        <div className="error-page">
          <h1>page not found</h1>
          <p>the cast, channel or user you requested doesn&apos;t exist (or the API doesn&apos;t know about it).</p>
          <p>
            <Link href={hrefFor(homeRoute)}>back to the front page</Link>
          </p>
        </div>
      </PageBody>
    </>
  );
}

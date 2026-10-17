import type { Metadata } from "next";
import { Suspense } from "react";
import { ClientPostPage } from "@/client/pages";
import { PageFallback } from "@/components/PageFallback";

export const metadata: Metadata = { title: "cast" };

/** /p/?hash=<hash> in the static build; /p/<hash> on a server. */
export default function PostPage() {
  return (
    <Suspense fallback={<PageFallback />}>
      <ClientPostPage />
    </Suspense>
  );
}

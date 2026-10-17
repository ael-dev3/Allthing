import type { Metadata } from "next";
import { Suspense } from "react";
import { ClientUserPage } from "@/client/pages";
import { PageFallback } from "@/components/PageFallback";

export const metadata: Metadata = { title: "user" };

/** /u/?name=<username> in the static build; /u/<username> on a server. */
export default function UserPage() {
  return (
    <Suspense fallback={<PageFallback />}>
      <ClientUserPage />
    </Suspense>
  );
}

import type { Metadata } from "next";
import { Suspense } from "react";
import { ClientChannelPage } from "@/client/pages";
import { PageFallback } from "@/components/PageFallback";

export const metadata: Metadata = { title: "channel" };

/** /c/?id=<channel> in the static build; /c/<channel> on a server. */
export default function ChannelPage() {
  return (
    <Suspense fallback={<PageFallback />}>
      <ClientChannelPage />
    </Suspense>
  );
}

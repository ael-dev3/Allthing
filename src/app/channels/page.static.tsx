import type { Metadata } from "next";
import { Suspense } from "react";
import { ClientChannelsPage } from "@/client/pages";
import { PageFallback } from "@/components/PageFallback";

export const metadata: Metadata = { title: "channels" };

export default function ChannelsPage() {
  return (
    <Suspense fallback={<PageFallback pageName="channels" />}>
      <ClientChannelsPage />
    </Suspense>
  );
}

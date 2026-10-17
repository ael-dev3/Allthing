import type { Metadata } from "next";
import { Suspense } from "react";
import { ClientFrontPage } from "@/client/pages";
import { PageFallback } from "@/components/PageFallback";

export const metadata: Metadata = {
  title: "allthing: the front page of farcaster",
};

export default function FrontPage() {
  return (
    <Suspense fallback={<PageFallback />}>
      <ClientFrontPage />
    </Suspense>
  );
}

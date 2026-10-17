import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import { KeyboardNav } from "@/components/KeyboardNav";
import { SiteFooter } from "@/components/SiteFooter";
import "./globals.css";

export const metadata: Metadata = {
  title: { default: "allthing", template: "%s" },
  description: "A read-only, old-reddit-style Farcaster client.",
};

export const viewport: Viewport = {
  colorScheme: "light",
  themeColor: "#cee3f8",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>
        {children}
        <SiteFooter />
        <KeyboardNav />
      </body>
    </html>
  );
}

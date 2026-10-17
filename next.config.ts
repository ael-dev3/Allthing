import type { NextConfig } from "next";

/*
 * One codebase, two builds:
 *
 * - default: server-rendered (Vercel or any Node host) with Cache Components.
 * - ALLTHING_STATIC_EXPORT=1: a static export for GitHub Pages. Only the
 *   *.static.tsx route files are used; they render the same views but fetch
 *   from the (CORS-enabled) Hypersnap API in the browser. Set
 *   ALLTHING_BASE_PATH=/<repo> when serving from a project subpath.
 */
const staticExport = process.env.ALLTHING_STATIC_EXPORT === "1";

const nextConfig: NextConfig = staticExport
  ? {
      output: "export",
      basePath: process.env.ALLTHING_BASE_PATH || undefined,
      trailingSlash: true,
      pageExtensions: ["static.tsx"],
      env: { NEXT_PUBLIC_ALLTHING_STATIC: "1" },
    }
  : {
      cacheComponents: true,
      partialPrefetching: true,
    };

export default nextConfig;

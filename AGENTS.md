<!-- BEGIN:nextjs-agent-rules -->

## This is NOT the Next.js you know

This version has breaking changes — APIs, conventions, and file structure may all differ from your training data. Read the relevant guide in `node_modules/next/dist/docs/` (resolved from this file's directory; in monorepos the `next` package may not be visible from the repo root) before writing any code. Heed deprecation notices.

This block is written and re-added by `next dev` — verify at `node_modules/next/dist/server/lib/generate-agent-files.js`. Removing it from a diff only re-creates the uncommitted change; committing it with your work keeps the tree clean.

<!-- END:nextjs-agent-rules -->

## Allthing project notes

- Read-only Farcaster client. Never add login, signing or write paths without being asked.
- Pages (`src/app`) read data only through the cached helpers in `src/lib/data.ts`, which call the `DataSource` interface (`src/lib/datasource/types.ts`). Provider quirks belong in `src/lib/datasource/`, not in components.
- Cache Components is on: anything that reads `params`, `searchParams` or the clock goes inside `<Suspense>`; use `requestNow()` instead of `Date.now()` in server components.
- Checks: `npm run check` (lint, typecheck, unit tests). End-to-end against live data: `npm run build && npm run test:e2e`.


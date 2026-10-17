/*
 * Props Next.js passes to server pages. Spelled out instead of the generated
 * PageProps helper so the same files type-check in the static-export build,
 * where these routes don't exist.
 */

export type SearchParams = Promise<Record<string, string | string[] | undefined>>;

export interface RouteProps<Params extends Record<string, string>> {
  params: Promise<Params>;
  searchParams: SearchParams;
}

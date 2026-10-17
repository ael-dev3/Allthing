import Link from "next/link";
import { queryString, type PageSlice } from "@/lib/listing";

/** "view more: ‹ prev | next ›" under a listing. */
export function NextPrev({
  path,
  params,
  slice,
}: {
  path: string;
  params: Record<string, string | undefined>;
  slice: PageSlice<unknown>;
}) {
  if (!slice.hasPrev && !slice.hasNext) return null;
  const href = (page: number) => path + queryString({ ...params, page: page > 1 ? page : undefined });
  return (
    <div className="nav-buttons">
      <span className="nextprev">
        view more:{" "}
        {slice.hasPrev && (
          <span className="prev-button">
            <Link href={href(slice.page - 1)} rel="prev">
              ‹ prev
            </Link>
          </span>
        )}
        {slice.hasPrev && slice.hasNext && <span className="separator" />}
        {slice.hasNext && (
          <span className="next-button">
            <Link href={href(slice.page + 1)} rel="next">
              next ›
            </Link>
          </span>
        )}
      </span>
    </div>
  );
}

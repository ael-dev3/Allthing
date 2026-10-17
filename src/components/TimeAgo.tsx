import { formatDateTime, timeAgo } from "@/lib/time";

export function TimeAgo({ iso, now }: { iso: string; now: number }) {
  return (
    <time className="live-timestamp" dateTime={iso} title={formatDateTime(iso)}>
      {timeAgo(iso, now)}
    </time>
  );
}

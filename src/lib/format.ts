/** Old reddit style compact numbers: 999, 1.2k, 12.3k, 1.2m. */
export function compactNumber(n: number): string {
  const abs = Math.abs(n);
  if (abs < 10_000) return String(n);
  if (abs < 1_000_000) return `${(n / 1000).toFixed(abs < 100_000 ? 1 : 0).replace(/\.0$/, "")}k`;
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, "")}m`;
}

/** 1221547 -> "1,221,547" */
export function fullNumber(n: number): string {
  return n.toLocaleString("en-US");
}

export function plural(n: number, singular: string, pluralForm = `${singular}s`): string {
  return `${fullNumber(n)} ${n === 1 ? singular : pluralForm}`;
}

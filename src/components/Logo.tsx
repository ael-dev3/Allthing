/** A little threaded-conversation glyph: a post and two nested replies. */
export function Logo() {
  return (
    <svg className="logo-mark" width="32" height="32" viewBox="0 0 32 32" aria-hidden="true">
      <circle cx="16" cy="16" r="15" fill="#fff" stroke="#5f99cf" strokeWidth="2" />
      <rect x="7" y="8" width="16" height="4" rx="2" fill="#ff4500" />
      <rect x="11" y="14.5" width="14" height="4" rx="2" fill="#5f99cf" />
      <rect x="15" y="21" width="10" height="4" rx="2" fill="#5f99cf" opacity="0.6" />
    </svg>
  );
}

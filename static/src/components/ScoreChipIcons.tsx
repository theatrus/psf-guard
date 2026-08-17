/** Icons that identify the two score bases without words: a crescent for
 * the night-session chip, stacked layers for the all-sessions chip. */

export function MoonIcon() {
  return (
    <svg
      className="score-chip-icon"
      viewBox="0 0 24 24"
      fill="currentColor"
      aria-hidden="true"
    >
      <path d="M20.6 14.5A8.6 8.6 0 0 1 9.5 3.4 9 9 0 1 0 20.6 14.5z" />
    </svg>
  );
}

export function LayersIcon() {
  return (
    <svg
      className="score-chip-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.4"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M12 3 3 8l9 5 9-5-9-5z" fill="currentColor" stroke="none" />
      <path d="m4.5 12.5 7.5 4.2 7.5-4.2" />
      <path d="m4.5 17 7.5 4.2 7.5-4.2" />
    </svg>
  );
}

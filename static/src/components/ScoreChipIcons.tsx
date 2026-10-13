/** Icons that identify the card chips without words: a crescent for the
 * night-session chip, stacked layers for the all-sessions chip, and the
 * Library's test tube for the calibration chip. */

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

/** A test tube, as on the Library's calibration pill, drawn so it shows
 * the same on every system rather than as an emoji. */
export function CalibrationIcon() {
  return (
    <svg
      className="score-chip-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <g transform="rotate(35 12 12)">
        <path d="M8 3h8" />
        <path d="M9.5 3v13.5a2.5 2.5 0 0 0 5 0V3" />
        <path d="M9.5 11.5h5v5a2.5 2.5 0 0 1-5 0z" fill="currentColor" stroke="none" />
      </g>
    </svg>
  );
}

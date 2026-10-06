/** A backend estimate with continued activity while one folder is being read. */
export function SearchProgressBar({ percent, running }: { percent?: number; running: boolean }) {
  return (
    <span
      className="search-progress"
      role="progressbar"
      aria-label="Estimated search progress"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      aria-valuetext={percent === undefined ? "Searching…" : `${percent}%${running ? " estimated" : ""}`}
      data-testid="search-progress-bar"
    >
      <span className="search-progress__fill" style={{ width: `${percent ?? 0}%` }} />
      {running && <span className="search-progress__activity" />}
    </span>
  );
}

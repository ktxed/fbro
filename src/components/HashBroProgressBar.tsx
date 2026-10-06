export function HashBroProgressBar({ label, percent }: { label: string; percent: number }) {
  const value = Math.max(0, Math.min(100, Math.round(percent)));
  return (
    <div className="modal__progress-section">
      <span className="modal__progress-label">{label} — {value}%</span>
      <div className="modal__progress-bar" role="progressbar" aria-label={label} aria-valuenow={value} aria-valuemin={0} aria-valuemax={100}>
        <div className="modal__progress-fill" style={{ width: `${value}%` }} />
      </div>
    </div>
  );
}


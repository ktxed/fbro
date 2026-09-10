import throbberUrl from "../assets/win98-flag-throbber-64nn.gif";

export function ViewerLoading() {
  return <div className="viewer__loading-overlay" role="status" aria-label="Loading image">
    <img src={throbberUrl} alt="" aria-hidden="true" />
    <span>Loading…</span>
  </div>;
}

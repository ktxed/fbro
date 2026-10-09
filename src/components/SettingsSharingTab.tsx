import { useCallback, useEffect, useRef, useState } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { WsClient } from "../ws/client";
import type { SharedRoot } from "../ws/types";
import { useInputHistory } from "../hooks/useInputHistory";
import { CopyButton } from "./CopyButton";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { PairingConfirmation } from "./PairingConfirmation";

interface SettingsSharingTabProps {
  client: WsClient;
}

/** A discovered peer is "stale" once we haven't seen an mDNS resolve for
 *  it in this long. The BE only ages entries out at 5 minutes; the FE
 *  visually dims much sooner so the user can tell the data is old. */
const STALE_AFTER_MS = 30_000;

function formatLastEndpointAge(seconds?: number) {
  if (!seconds) return "unknown";
  const delta = Math.max(0, Math.round(Date.now() / 1000) - seconds);
  if (delta < 60) return `${delta}s ago`;
  const minutes = Math.round(delta / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

function CopyMeta({ value, kind }: { value: string; kind: "endpoint" | "fingerprint" }) {
  return (
    <span className="sharing-copy-meta">
      <code className="sharing-copy-meta__value" title={value}>{value}</code>
      <CopyButton className="sharing-copy-meta__button" text={value} what={kind} iconSize={12} />
    </span>
  );
}

function mergeShareTargets(selected: SharedRoot[], suggestions: SharedRoot[]) {
  const byId = new Map<string, SharedRoot>();
  for (const target of selected) byId.set(target.id, target);
  for (const target of suggestions) byId.set(target.id, target);
  return Array.from(byId.values()).sort((a, b) => {
    const originOrder = originRank(a.origin) - originRank(b.origin);
    if (originOrder !== 0) return originOrder;
    const nameOrder = a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
    if (nameOrder !== 0) return nameOrder;
    return a.path.localeCompare(b.path);
  });
}

function originRank(origin: SharedRoot["origin"]) {
  if (origin === "source") return 0;
  if (origin === "home") return 1;
  if (origin === "bookmark") return 2;
  return 3;
}

function originLabel(origin: SharedRoot["origin"]) {
  if (origin === "source") return "source";
  if (origin === "home") return "home";
  if (origin === "folder") return "folder";
  return "bookmark";
}

function ShareTargetGroup({
  label,
  targets,
  sharedSet,
  onToggle,
}: {
  label: string;
  targets: SharedRoot[];
  sharedSet: Set<string>;
  onToggle: (targetId: string, control: HTMLInputElement) => void;
}) {
  return (
    <div className="sharing-source-group">
      <div className="sharing-source-group__label">{label}</div>
      <ul className="sharing-source-list">
        {targets.map((target) => (
          <li key={target.id} data-share-target-id={target.id} data-share-path={target.path}>
            <label className="sharing-source">
              <input
                type="checkbox"
                checked={sharedSet.has(target.id)}
                aria-label={`Share ${target.name}: ${target.path}`}
                onChange={(event) => onToggle(target.id, event.currentTarget)}
              />
              <span className="sharing-source__main">
                <strong>
                  {target.name}
                  <span className="sharing-source__origin">{originLabel(target.origin)}</span>
                </strong>
                <span>{target.path}</span>
              </span>
            </label>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * Sharing settings: enable LAN remote access, pick which local sources to
 * expose, manage paired peers, and see / connect to discovered peers.
 */
export function SettingsSharingTab({ client }: SettingsSharingTabProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const share = state.shareConfig;
  const discovered = state.discoveredPeers;
  const pairing = state.pendingPairing;
  const pairingPeerId = pairing
    ? discovered.find((peer) => peer.pubkeyFp === pairing.peerFingerprint)?.id
    : undefined;
  const shareTargets = mergeShareTargets(share.sharedRoots, share.shareSuggestions);
  const sharedSet = new Set(share.sharedRoots.map((root) => root.id));
  const sourceTargets = shareTargets.filter((target) => target.origin === "source");
  const suggestedTargets = shareTargets.filter((target) => target.origin === "home" || target.origin === "bookmark");
  const folderTargets = shareTargets.filter((target) => target.origin === "folder");
  const folderInput = state.sharedFolderInput;
  const setFolderPath = useCallback((path: string) => dispatch({ type: "SET_SHARED_FOLDER_INPUT", path }), [dispatch]);
  const folderHistory = useInputHistory("sharing.folder", setFolderPath);
  const submittingFolder = folderInput.requestId !== null;
  const submittedRequestRef = useRef<string | null>(folderInput.requestId);
  submittedRequestRef.current = folderInput.requestId;
  const [folderSuggestion, setFolderSuggestion] = useState("");
  useEffect(() => {
    if (!folderInput.path && !submittingFolder) setFolderSuggestion("");
  }, [folderInput.path, submittingFolder]);
  const activePanel = state.panels[state.focus];
  const activeSource = activePanel && state.sources[activePanel.sourceId];
  const currentFolder = activePanel?.tabKind === "listing" && activeSource?.kind === "filesystem"
    && activeSource.mounted && !activeSource.shadow ? activePanel.path : undefined;
  const folderSuggestions = [
    ...(currentFolder ? [{ id: "current", name: "Current folder", path: currentFolder }] : []),
    ...share.shareSuggestions,
  ];
  const focusFolderInput = () => {
    folderHistory.inputRef.current?.focus();
    folderHistory.inputRef.current?.scrollIntoView({ block: "nearest" });
  };

  const addFolder = () => {
    if (!state.connected || submittedRequestRef.current || !folderInput.path.trim()) return;
    const requestId = globalThis.crypto.randomUUID();
    submittedRequestRef.current = requestId;
    folderHistory.addToHistory(folderInput.path);
    folderHistory.close();
    dispatch({ type: "SUBMIT_SHARED_FOLDER", requestId });
    client.sendIntent("AddSharedFolder", undefined, {
      requestId, path: folderInput.path, panelId: state.focus,
    });
  };

  // Map peer fingerprint → arbitrary mounted source_id (used both for
  // the "live" stale-override and to feed DisconnectRemotePeer when
  // the user clicks Disconnect — the BE infers the peer fp from any
  // remote: source id and tears down siblings for that peer in one go).
  //
  // Match strategy: prefer `src.peer.fingerprint` (the structured
  // field) but fall back to parsing the source id (`remote:<fp>:<rest>`).
  // The latter is the canonical encoding and survives a temporarily
  // missing `peer` block (BE-side `paired_peers` race, partial patch),
  // which would otherwise leave the Disconnect button hidden behind a
  // misleading Connect.
  const mountedByFp = new Map<string, string>();
  // Fingerprints of peers we have at least one *healthy* remote mount
  // from. Live SFTP is stronger evidence of reachability than fresh
  // mDNS so we override the "stale" dim when we can see one.
  const liveByFp = new Set<string>();
  for (const src of Object.values(state.sources)) {
    if (src.kind !== "remote") continue;
    const fp =
      src.peer?.fingerprint ??
      (src.id.startsWith("remote:")
        ? src.id.split(":", 2)[1]
        : undefined);
    if (!fp) continue;
    if (!mountedByFp.has(fp)) mountedByFp.set(fp, src.id);
    // Reducer drops the entry when "connected" — `undefined` means
    // healthy. Anything else (reconnecting/disconnected/dead) means we
    // shouldn't claim liveness on the peer's behalf.
    if (state.remoteSourceStatus[src.id] === undefined) liveByFp.add(fp);
  }

  const toggleEnabled = () => {
    client.sendIntent("EnableSharing", undefined, { enabled: !share.enabled });
  };

  const toggleIpv6 = () => {
    client.sendIntent("SetSharingIpv6", undefined, { allowIpv6: !share.allowIpv6 });
  };

  const toggleShareTarget = (targetId: string, control: HTMLInputElement) => {
    const next = new Set(sharedSet);
    if (next.has(targetId)) next.delete(targetId);
    else next.add(targetId);
    // The folder row disappears when the backend confirms removal. Hand its
    // focused control to the input before removal; outside focus stays put.
    if (folderTargets.some((target) => target.id === targetId)
      && sharedSet.has(targetId) && document.activeElement === control) {
      focusFolderInput();
    }
    client.sendIntent("SetSharedTargets", undefined, {
      targetIds: Array.from(next),
    });
  };

  // Two-step revoke: first click arms the button (`pendingRevoke` is set
  // to the peer's fingerprint), second click within the timeout actually
  // fires `RevokePairing`. Tauri's WKWebView silently drops
  // `window.confirm()` so we can't use the native dialog.
  const [pendingRevoke, setPendingRevoke] = useState<string | null>(null);
  useEffect(() => {
    if (!pendingRevoke) return;
    const id = setTimeout(() => setPendingRevoke(null), 4000);
    return () => clearTimeout(id);
  }, [pendingRevoke]);

  const revoke = (fingerprint: string) => {
    if (pendingRevoke === fingerprint) {
      client.sendIntent("RevokePairing", undefined, { fingerprint });
      setPendingRevoke(null);
    } else {
      setPendingRevoke(fingerprint);
    }
  };

  const pair = (peerId: string) => {
    if (pairing) return;
    client.sendIntent("StartPairing", undefined, { peerId });
  };

  const [directPairAddress, setDirectPairAddress] = useState("");
  const pairHistory = useInputHistory("sharing.pair", setDirectPairAddress);

  const pairDirect = () => {
    if (pairing) return;
    const address = directPairAddress.trim();
    if (!address) return;
    pairHistory.addToHistory(directPairAddress);
    client.sendIntent("StartPairing", undefined, { address });
  };

  const connect = (peerId: string) => {
    client.sendIntent("ConnectRemotePeer", undefined, { peerId });
  };

  const retryLastPeer = (fingerprint: string) => {
    client.sendIntent("RetryLastPeer", undefined, { fingerprint });
  };

  // Active jobs = anything that would lose progress if we yank the driver
  // out from under it. The disconnect intent itself doesn't check; the
  // job will fail on its next chunk read, which is fine but we'd rather
  // warn the user first.
  const activeJobCount = Object.values(state.jobs).filter(
    (j) =>
      j.state === "Queued" ||
      j.state === "Running" ||
      j.state === "AwaitingDecision",
  ).length;

  // Two-step confirm for Disconnect when there are active jobs. Same
  // pattern as Revoke — Tauri's WKWebView silently drops window.confirm().
  const [pendingDisconnect, setPendingDisconnect] = useState<string | null>(null);
  useEffect(() => {
    if (!pendingDisconnect) return;
    const id = setTimeout(() => setPendingDisconnect(null), 4000);
    return () => clearTimeout(id);
  }, [pendingDisconnect]);

  const disconnect = (sourceId: string) => {
    if (activeJobCount > 0 && pendingDisconnect !== sourceId) {
      setPendingDisconnect(sourceId);
      return;
    }
    setPendingDisconnect(null);
    client.sendIntent("DisconnectRemotePeer", undefined, { sourceId });
  };

  // Local refresh-busy state — the BE doesn't echo back a "refresh
  // started" event, so we just light the badge for 3s after the click.
  const [refreshing, setRefreshing] = useState(false);
  // Tick every 5s so the dim-stale check re-evaluates as time passes
  // even if no patches arrive in the meantime.
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 5000);
    return () => clearInterval(id);
  }, []);

  const refresh = () => {
    client.sendIntent("RefreshDiscoveredPeers");
    setRefreshing(true);
    setTimeout(() => setRefreshing(false), 3000);
  };

  const livePeerCount = share.pairedPeers.filter((peer) =>
    liveByFp.has(peer.fingerprint),
  ).length;
  const staleDiscoveredCount = discovered.filter((peer) => {
    const liveMount = !!(peer.pubkeyFp && liveByFp.has(peer.pubkeyFp));
    return !liveMount && Math.max(0, now - peer.lastSeen) > STALE_AFTER_MS;
  }).length;

  return (
    <div className="settings-tab sharing-tab">
      <section className={`sharing-hero${share.enabled ? " sharing-hero--on" : ""}`}>
        <div className="sharing-hero__main">
          <span className="sharing-kicker">Remote bro</span>
          <h3 className="sharing-hero__title">
            {share.enabled ? "Listening for trusted peers" : "Remote access is off"}
          </h3>
          <p className="sharing-hero__copy">
            {share.enabled
              ? "This machine is advertised on the LAN and accepts paired read-only SFTP sessions."
              : "Enable remote access to pair another fbro and expose selected local sources."}
          </p>
        </div>
        <label className="sharing-switch">
          <input
            type="checkbox"
            checked={share.enabled}
            onChange={toggleEnabled}
          />
          <span>{share.enabled ? "On" : "Off"}</span>
        </label>
        {share.fingerprint && (
          <div className="sharing-fingerprint">
            <span>Fingerprint</span>
            <code>{share.fingerprint}</code>
          </div>
        )}
      </section>

      {share.enabled && share.sharedRoots.length === 0 && (
        <section className="sharing-warning" role="status">
          Remote access is on, but no folders are shared. Paired peers can connect but will not see any files.
        </section>
      )}

      <section className="sharing-stats" aria-label="Sharing status">
        <div className="sharing-stat">
          <span className="sharing-stat__value">{sharedSet.size}</span>
          <span className="sharing-stat__label">shared folders</span>
        </div>
        <div className="sharing-stat">
          <span className="sharing-stat__value">{share.pairedPeers.length}</span>
          <span className="sharing-stat__label">paired peers</span>
        </div>
        <div className="sharing-stat">
          <span className="sharing-stat__value">{livePeerCount}</span>
          <span className="sharing-stat__label">mounted now</span>
        </div>
        <div className="sharing-stat">
          <span className="sharing-stat__value">{discovered.length}</span>
          <span className="sharing-stat__label">
            discovered{staleDiscoveredCount > 0 ? `, ${staleDiscoveredCount} stale` : ""}
          </span>
        </div>
      </section>

      <section className="sharing-card">
        <div className="sharing-card__header">
          <div>
            <span className="sharing-kicker">Trust store</span>
            <h3 className="sharing-card__title">Paired peers</h3>
          </div>
        </div>
        {share.pairedPeers.length === 0 ? (
          <p className="sharing-empty">
            No paired peers yet. Pair by address or use a fresh discovery result below.
          </p>
        ) : (
          <ul className="sharing-peer-list">
            {share.pairedPeers.map((p) => {
              const mountedId = mountedByFp.get(p.fingerprint);
              const armedRevoke = pendingRevoke === p.fingerprint;
              const armedDisconnect =
                mountedId !== undefined && pendingDisconnect === mountedId;
              const liveDiscovered = discovered.find(
                (peer) =>
                  peer.pubkeyFp === p.fingerprint &&
                  Math.max(0, now - peer.lastSeen) <= STALE_AFTER_MS,
              );
              const canConnectLive = !mountedId && liveDiscovered;
              const canRetryLast = !mountedId && !liveDiscovered && p.lastHost && p.lastSftpPort;
              return (
                <li key={p.fingerprint} className="sharing-peer">
                  <div className="sharing-peer__avatar">{p.name.slice(0, 1).toUpperCase()}</div>
                  <div className="sharing-peer__body">
                    <div className="sharing-peer__topline">
                      <strong>{p.name}</strong>
                      {p.os && <span>{p.os}</span>}
                      {mountedId && (
                        <span
                          className="sharing-badge sharing-badge--live"
                          title="A remote SFTP session from this peer is currently mounted."
                        >
                          mounted
                        </span>
                      )}
                    </div>
                    <div className="sharing-peer__meta">
                      <CopyMeta value={p.fingerprint} kind="fingerprint" />
                    </div>
                    {p.lastHost && p.lastSftpPort && (
                      <div className="sharing-peer__meta">
                        <CopyMeta value={`${p.lastHost}:${p.lastSftpPort}`} kind="endpoint" />
                        <span>· {formatLastEndpointAge(p.lastEndpointSeenAt)}</span>
                      </div>
                    )}
                  </div>
                  <div className="sharing-actions">
                    {canConnectLive && (
                      <button
                        className="sharing-button"
                        onClick={() => connect(liveDiscovered.id)}
                        title="Mount this peer using the current LAN discovery result."
                      >
                        Connect
                      </button>
                    )}
                    {canRetryLast && (
                      <button
                        className="sharing-button"
                        onClick={() => retryLastPeer(p.fingerprint)}
                        title="Try the last known SFTP address even though mDNS is quiet."
                      >
                        Retry last
                      </button>
                    )}
                    {mountedId && (
                      <button
                        className={`sharing-button sharing-button--danger${
                          armedDisconnect ? " sharing-button--armed" : ""
                        }`}
                        onClick={() => disconnect(mountedId)}
                        title={
                          armedDisconnect
                            ? `Click again to disconnect (${activeJobCount} active job${activeJobCount === 1 ? "" : "s"} will be cancelled)`
                            : "Unmount this peer's sources and tear down the SFTP session."
                        }
                      >
                        {armedDisconnect
                          ? `Cancel ${activeJobCount} job${activeJobCount === 1 ? "" : "s"}`
                          : "Disconnect"}
                      </button>
                    )}
                    <button
                      className={`sharing-button sharing-button--danger${
                        armedRevoke ? " sharing-button--armed" : ""
                      }`}
                      onClick={() => revoke(p.fingerprint)}
                      title={
                        armedRevoke
                          ? "Click again to confirm revoke"
                          : "Drop this peer's pubkey — subsequent connections will fail at auth"
                      }
                    >
                      {armedRevoke ? "Confirm revoke" : "Revoke"}
                    </button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="sharing-card">
        <div className="sharing-card__header">
          <div>
            <span className="sharing-kicker">LAN discovery</span>
            <h3 className="sharing-card__title">
              Discovered peers
              {refreshing && <span className="sharing-refreshing">refreshing</span>}
            </h3>
          </div>
          <button
            className="sharing-button"
            onClick={refresh}
            disabled={refreshing}
            title="Re-issue the mDNS browse query"
          >
            Refresh
          </button>
        </div>
        {discovered.length === 0 ? (
          <p className="sharing-empty">
            No fbro peers detected on the LAN. Direct pairing below does not depend on mDNS.
          </p>
        ) : (
          <ul className="sharing-peer-list">
            {discovered.map((p) => {
              const ageMs = Math.max(0, now - p.lastSeen);
              const liveMount = !!(p.pubkeyFp && liveByFp.has(p.pubkeyFp));
              // Don't grey out a peer we're actively connected to —
              // see the comment on `liveByFp`. mDNS may have stopped
              // re-resolving (often a Linux firewall thing — UFW
              // sometimes blocks the unicast leg of queries) but the
              // SFTP session itself proves the peer is alive.
              const stale = !liveMount && ageMs > STALE_AFTER_MS;
              return (
                <li
                  key={p.id}
                  className={`sharing-peer${stale && pairingPeerId !== p.id ? " sharing-peer--stale" : ""}${pairingPeerId === p.id ? " sharing-peer--pairing" : ""}`}
                  data-peer-id={p.id}
                  data-peer-fingerprint={p.pubkeyFp}
                >
                  <div className="sharing-peer__avatar">{p.name.slice(0, 1).toUpperCase()}</div>
                  <div className="sharing-peer__body">
                    <div className="sharing-peer__topline">
                      <strong>{p.name}</strong>
                      {p.os && <span>{p.os}</span>}
                      {p.paired && <span className="sharing-badge">paired</span>}
                      {stale && <span className="sharing-badge sharing-badge--stale">stale</span>}
                    </div>
                    <div className="sharing-peer__meta">
                      <CopyMeta value={`${p.host}:${p.port}`} kind="endpoint" />
                      {p.pubkeyFp && (
                        <>
                          {" — "}
                          <CopyMeta value={p.pubkeyFp} kind="fingerprint" />
                        </>
                      )}
                      {stale && (
                        <>
                          {" — "}
                          <span title={`last seen ${Math.round(ageMs / 1000)}s ago`}>
                            last seen {Math.round(ageMs / 1000)}s ago
                          </span>
                        </>
                      )}
                      {liveMount && !stale && ageMs > STALE_AFTER_MS && (
                        <>
                          {" — "}
                          <span title="mDNS hasn't re-resolved this peer recently, but the SFTP session is live">
                            connected (mDNS quiet)
                          </span>
                        </>
                      )}
                    </div>
                  </div>
                  <div className="sharing-actions">
                    {(() => {
                      const mountedId = p.pubkeyFp
                        ? mountedByFp.get(p.pubkeyFp)
                        : undefined;
                      if (mountedId) {
                        const armed = pendingDisconnect === mountedId;
                        return (
                          <button
                            className={`sharing-button sharing-button--danger${
                              armed ? " sharing-button--armed" : ""
                            }`}
                            onClick={() => disconnect(mountedId)}
                            title={
                              armed
                                ? `Click again to disconnect (${activeJobCount} active job${activeJobCount === 1 ? "" : "s"} will be cancelled)`
                                : "Unmount this peer's sources and tear down the SFTP session."
                            }
                          >
                            {armed
                              ? `Cancel ${activeJobCount} job${activeJobCount === 1 ? "" : "s"}`
                              : "Disconnect"}
                          </button>
                        );
                      }
                      if (p.paired) {
                        return (
                          <button
                            className="sharing-button"
                            onClick={() => connect(p.id)}
                            title="Mount this peer's shared sources"
                          >
                            Connect
                          </button>
                        );
                      }
                      if (p.pubkeyFp) {
                        return (
                          <button
                            className="sharing-button sharing-button--primary"
                            onClick={() => pair(p.id)}
                            disabled={pairing !== null}
                            title="Begin a pairing handshake — compare the 6-digit code on both sides"
                          >
                            {pairingPeerId === p.id ? "Pairing…" : "Pair"}
                          </button>
                        );
                      }
                      return <span className="sharing-peer__meta">non-fbro sshd</span>;
                    })()}
                  </div>
                  {pairing && pairingPeerId === p.id && (
                    <PairingConfirmation client={client} pairing={pairing} />
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="sharing-card">
        <div className="sharing-card__header">
          <div>
            <span className="sharing-kicker">Expose</span>
            <h3 className="sharing-card__title">Shared folders</h3>
          </div>
          <span className="sharing-chip">read-only</span>
        </div>
        <div className="sharing-folder-row">
          <input
            ref={folderHistory.inputRef as React.RefObject<HTMLInputElement>}
            className="sharing-input"
            data-testid="sharing-folder-path"
            aria-label="Folder to share"
            aria-keyshortcuts="Alt+ArrowDown"
            aria-invalid={!!folderInput.error}
            aria-describedby={folderInput.error ? "sharing-folder-error" : undefined}
            type="text"
            value={folderInput.path}
            readOnly={submittingFolder}
            onChange={(event) => { setFolderPath(event.target.value); setFolderSuggestion(""); }}
            onKeyDown={(event) => {
              folderHistory.onKeyDown(event);
              if (event.defaultPrevented) return;
              if (event.key === "Enter") { event.preventDefault(); addFolder(); }
            }}
            placeholder="Add a folder path…"
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
          />
          {folderHistory.isOpen && (
            <InputHistoryDropdown
              anchorEl={folderHistory.inputRef.current}
              items={folderHistory.items}
              activeIndex={folderHistory.activeIndex}
              onSelect={folderHistory.selectItem}
              onClose={folderHistory.close}
            />
          )}
          <select
            className="sharing-input sharing-folder-suggestions"
            data-testid="sharing-folder-suggestions"
            aria-label="Suggested folder to share"
            value={folderSuggestion}
            disabled={submittingFolder}
            onChange={(event) => {
              const id = event.target.value;
              setFolderSuggestion(id);
              const suggestion = folderSuggestions.find((candidate) => candidate.id === id);
              if (suggestion) setFolderPath(suggestion.path);
            }}
          >
            <option value="">Suggested folder…</option>
            {folderSuggestions.map((suggestion) => (
              <option key={suggestion.id} value={suggestion.id}>{suggestion.name}: {suggestion.path}</option>
            ))}
          </select>
          <button
            type="button"
            className="sharing-button sharing-button--primary"
            data-command-id="sharing.folder.add"
            disabled={!state.connected || submittingFolder || !folderInput.path.trim()}
            onClick={() => { focusFolderInput(); addFolder(); }}
          >{submittingFolder ? "Adding…" : "Add"}</button>
        </div>
        {folderInput.error && (
          <p id="sharing-folder-error" className="sharing-folder-error" role="alert" data-testid="sharing-folder-error">
            {folderInput.error}
          </p>
        )}
        <p className="sharing-note">Enter a folder path or choose a suggestion. Alt+Down shows previous paths.</p>
        {shareTargets.length === 0 ? (
          <p className="sharing-empty">No local folders to share.</p>
        ) : (
          <>
            {folderTargets.length > 0 && (
              <ShareTargetGroup label="Added folders" targets={folderTargets} sharedSet={sharedSet} onToggle={toggleShareTarget} />
            )}
            {sourceTargets.length > 0 && (
              <ShareTargetGroup
                label="Sources"
                targets={sourceTargets}
                sharedSet={sharedSet}
                onToggle={toggleShareTarget}
              />
            )}
            {suggestedTargets.length > 0 && (
              <ShareTargetGroup
                label="Suggestions"
                targets={suggestedTargets}
                sharedSet={sharedSet}
                onToggle={toggleShareTarget}
              />
            )}
          </>
        )}
      </section>

      <section className="sharing-card sharing-card--pair" data-testid="direct-pair-card">
        <div className="sharing-card__header">
          <div>
            <span className="sharing-kicker">Pair directly</span>
            <h3 className="sharing-card__title">Add a peer by address</h3>
          </div>
          <span className="sharing-chip">TCP 7880</span>
        </div>
        <div className="sharing-pair-row">
          <input
            ref={pairHistory.inputRef as React.RefObject<HTMLInputElement>}
            className="sharing-input"
            data-testid="direct-pair-address"
            type="text"
            value={directPairAddress}
            disabled={pairing !== null}
            onChange={(event) => setDirectPairAddress(event.target.value)}
            onKeyDown={(event) => {
              pairHistory.onKeyDown(event);
              if (event.defaultPrevented) return;
              if (event.key === "Enter") pairDirect();
            }}
            placeholder="host or IP, e.g. 192.168.1.20"
          />
          {pairHistory.isOpen && (
            <InputHistoryDropdown
              anchorEl={pairHistory.inputRef.current}
              items={pairHistory.items}
              activeIndex={pairHistory.activeIndex}
              onSelect={pairHistory.selectItem}
              onClose={pairHistory.close}
            />
          )}
          <button
            className="sharing-button sharing-button--primary"
            data-command-id="sharing.pair-direct"
            onClick={pairDirect}
            disabled={pairing !== null || directPairAddress.trim().length === 0}
            title="Pair directly over TCP port 7880. The peer must have remote access enabled."
          >
            {pairing && !pairingPeerId ? "Pairing…" : "Pair"}
          </button>
        </div>
        {pairing && !pairingPeerId && (
          <PairingConfirmation client={client} pairing={pairing} />
        )}
        <p className="sharing-note">
          Use this when mDNS is empty or stale. Browsing switches to SFTP port
          7879 after trust is established.
        </p>
        <div className="sharing-option">
          <label className="sharing-source">
            <input
              type="checkbox"
              checked={share.allowIpv6}
              onChange={toggleIpv6}
            />
            <span className="sharing-source__main">
              <strong>Allow IPv6</strong>
              <span>Off by default; discovery, pairing, and remote SFTP prefer IPv4.</span>
            </span>
          </label>
        </div>
      </section>
    </div>
  );
}

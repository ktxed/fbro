//! mDNS browse + advertise for fbro peers.
//!
//! Service type: `_fbro-remote._tcp.local.`
//! TXT keys (advertise side): `fbro=1`, `name=<hostname>`,
//! `os=<linux|macos|windows>`, `fp=<pubkey-fingerprint>`.
//!
//! The spawned task owns a `ServiceDaemon` and drives two streams:
//!  - the mDNS `Receiver<ServiceEvent>` (browse results)
//!  - a command `mpsc::Receiver<DiscoveryCommand>` (advertise toggles)
//!
//! On every change to the discovered set, it updates `AppState.discovered_peers`
//! and emits a `StatePatch { domain: "discovered_peers" }` to all GUI clients.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;
use tokio::sync::mpsc;

use crate::state::{DiscoveredPeer, SharedState};
use crate::ws::protocol::{now, ServerMessage};

pub const SERVICE_TYPE: &str = "_fbro-remote._tcp.local.";

#[derive(Debug)]
pub enum DiscoveryCommand {
    /// Begin advertising this fbro on the LAN.
    EnableAdvertise {
        /// Instance name (typically the hostname). Must be unique on the LAN.
        instance: String,
        port: u16,
        /// TXT records to publish — caller is expected to include
        /// `fbro=1` plus optional `name`, `os`, `fp` keys.
        txt: Vec<(String, String)>,
    },
    /// Drop the current discovered set and re-issue the mDNS browse query,
    /// which triggers fresh `ServiceResolved` events from peers that are
    /// still advertising. Useful when a peer started up *after* we did and
    /// our cached browse hasn't picked it up yet.
    Refresh,
    /// Tear down the daemon and exit the task.
    Shutdown,
}

#[derive(Clone)]
pub struct DiscoveryHandle {
    tx: mpsc::Sender<DiscoveryCommand>,
}

impl DiscoveryHandle {
    pub async fn enable_advertise(&self, instance: String, port: u16, txt: Vec<(String, String)>) {
        let _ = self
            .tx
            .send(DiscoveryCommand::EnableAdvertise {
                instance,
                port,
                txt,
            })
            .await;
    }

    pub async fn refresh(&self) {
        let _ = self.tx.send(DiscoveryCommand::Refresh).await;
    }

    pub async fn shutdown(&self) {
        let _ = self.tx.send(DiscoveryCommand::Shutdown).await;
    }
}

/// JSON projection of a `DiscoveredPeer` for the FE. Kept here (not in
/// `protocol.rs`) because it's specific to the discovery domain.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPeerSchema {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey_fp: Option<String>,
    /// Convenience flag for the FE — set when the peer's fingerprint is in
    /// `share_config.paired_peers`. Saves the FE a cross-domain lookup.
    pub paired: bool,
    /// Unix millis of the last `ServiceResolved` for this peer. The FE
    /// dims rows whose `last_seen` is older than a few seconds — the
    /// "browse keeps running, peers may have gone offline silently" cue.
    pub last_seen: u64,
}

/// Age-out threshold: drop discovered entries that haven't been
/// re-resolved in this long. Generous because mDNS announcement
/// cadence is unpredictable; the FE staleness indicator catches
/// transient gaps.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(300);
/// How often the supervisor sweeps for stale entries.
const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// How often the supervisor re-issues `stop_browse → browse` on its own,
/// without user intervention. mdns-sd's internal query schedule backs
/// off aggressively (1s, 2s, 4s, …, up to ~1 hour), so a peer that
/// comes online late may not be queried for a long time. Re-browsing
/// resets the schedule and forces fresh queries — keeps the discovered
/// set responsive to peers joining/leaving without making the user hit
/// Refresh.
const REBROWSE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Spawn the discovery task. Returns a handle for commanding advertise on/off.
///
/// The task runs until `Shutdown` is received or all command-channel senders
/// are dropped.
pub fn spawn(
    state: SharedState,
    broadcast: Arc<dyn Broadcaster>,
) -> std::io::Result<DiscoveryHandle> {
    let daemon = ServiceDaemon::new()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let browse_rx = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<DiscoveryCommand>(16);

    tokio::spawn(run(daemon, browse_rx, cmd_rx, state, broadcast));

    Ok(DiscoveryHandle { tx: cmd_tx })
}

/// Trait wrapping how this module pushes patches out. Decouples discovery
/// from the WS server's exact broadcast plumbing — tests pass a recording
/// stub; production passes a sender that fans out to every GUI client.
pub trait Broadcaster: Send + Sync {
    fn broadcast(&self, msg: ServerMessage);
}

/// Production broadcaster: clones `state.gui_senders` under the lock and
/// sends to each. Synchronous-ish — uses `try_send` so a stuck client can't
/// stall the discovery task. Patches are idempotent (full snapshot) so a
/// dropped one is fine.
pub struct GuiFanout {
    pub state: SharedState,
}

impl Broadcaster for GuiFanout {
    fn broadcast(&self, msg: ServerMessage) {
        let state = self.state.clone();
        tokio::spawn(async move {
            let senders: Vec<_> = {
                let guard = state.lock().await;
                guard.gui_senders.clone()
            };
            for tx in senders {
                let _ = tx.try_send(msg.clone());
            }
        });
    }
}

async fn run(
    daemon: ServiceDaemon,
    mut browse_rx: mdns_sd::Receiver<ServiceEvent>,
    mut cmd_rx: mpsc::Receiver<DiscoveryCommand>,
    state: SharedState,
    broadcast: Arc<dyn Broadcaster>,
) {
    // fullname (as returned by ServiceInfo::get_fullname) of the currently
    // advertised instance, so we can unregister it cleanly when asked.
    let mut advertised_fullname: Option<String> = None;
    // Map fullname → our internal peer id, so we can remove on ServiceRemoved.
    let mut fullname_to_id: HashMap<String, String> = HashMap::new();

    // Our own fingerprint. Prefer the identity loaded in AppState so self
    // filtering is active even before the advertise command is processed.
    // EnableAdvertise refreshes it from the TXT payload after identity reloads.
    let mut our_advertised_fp: Option<String> = {
        let guard = state.lock().await;
        guard
            .identity
            .as_ref()
            .map(|identity| identity.fingerprint.clone())
    };
    if our_advertised_fp.is_some() {
        let removed = remove_self_discovery_entries(
            &state,
            &mut fullname_to_id,
            advertised_fullname.as_deref(),
            our_advertised_fp.as_deref(),
        )
        .await;
        if removed > 0 {
            let snapshot = snapshot_peers(&state).await;
            broadcast.broadcast(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "discoveredPeers".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({ "peers": snapshot }),
            });
        }
    }

    // Periodic sweep that ages out entries whose last_seen is older than
    // STALE_AFTER. Discovery itself doesn't reliably get
    // `ServiceRemoved` for peers that crash, so we need our own clock.
    let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Auto re-browse so peers that came online after our initial query
    // schedule backed off still get noticed.
    let mut rebrowse = tokio::time::interval(REBROWSE_INTERVAL);
    rebrowse.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick — the daemon's own initial queries
    // are still active at startup, no need to double up.
    rebrowse.tick().await;

    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => match cmd {
                Some(DiscoveryCommand::EnableAdvertise { instance, port, txt }) => {
                    if let Some(prev) = advertised_fullname.take() {
                        let _ = daemon.unregister(&prev);
                    }
                    our_advertised_fp = txt
                        .iter()
                        .find(|(k, _)| k == "fp")
                        .map(|(_, v)| v.clone());
                    match build_service_info(&instance, port, &txt) {
                        Ok(info) => {
                            let fullname = info.get_fullname().to_string();
                            eprintln!(
                                "[discovery] advertising '{fullname}' on port {port} (fp={})",
                                our_advertised_fp.as_deref().unwrap_or("?")
                            );
                            if let Err(e) = daemon.register(info) {
                                eprintln!("[discovery] register failed: {e}");
                            } else {
                                advertised_fullname = Some(fullname);
                                let removed = remove_self_discovery_entries(
                                    &state,
                                    &mut fullname_to_id,
                                    advertised_fullname.as_deref(),
                                    our_advertised_fp.as_deref(),
                                )
                                .await;
                                if removed > 0 {
                                    let snapshot = snapshot_peers(&state).await;
                                    broadcast.broadcast(ServerMessage::StatePatch {
                                        timestamp: now(),
                                        domain: "discoveredPeers".to_string(),
                                        entity_id: None,
                                        version: 0,
                                        patch: serde_json::json!({ "peers": snapshot }),
                                    });
                                }
                            }
                        }
                        Err(e) => eprintln!("[discovery] build_service_info: {e}"),
                    }
                }
                Some(DiscoveryCommand::Refresh) => {
                    eprintln!("[discovery] manual refresh — re-browsing");
                    // Soft refresh: keep existing discovered peers (their
                    // `last_seen` will update as ServiceResolved fires
                    // anew), just re-issue the browse query. Clearing the
                    // set on Refresh made the UI flash empty before peers
                    // re-announced, which felt like the button did nothing.
                    let _ = daemon.stop_browse(SERVICE_TYPE);
                    match daemon.browse(SERVICE_TYPE) {
                        Ok(rx) => browse_rx = rx,
                        Err(e) => eprintln!("[discovery] re-browse failed: {e}"),
                    }
                }
                Some(DiscoveryCommand::Shutdown) | None => {
                    if let Some(name) = advertised_fullname.take() {
                        let _ = daemon.unregister(&name);
                    }
                    let _ = daemon.shutdown();
                    return;
                }
            },
            event = browse_rx.recv_async() => {
                let Ok(event) = event else {
                    // mDNS channel closed — daemon is gone, nothing to do.
                    return;
                };
                if handle_event(
                    event,
                    &state,
                    &mut fullname_to_id,
                    our_advertised_fp.as_deref(),
                ).await {
                    let snapshot = snapshot_peers(&state).await;
                    broadcast.broadcast(ServerMessage::StatePatch {
                        timestamp: now(),
                        domain: "discoveredPeers".to_string(),
                        entity_id: None,
                        version: 0,
                        patch: serde_json::json!({ "peers": snapshot }),
                    });
                }
            }
            _ = sweep.tick() => {
                let removed = sweep_stale(&state, &mut fullname_to_id).await;
                if removed > 0 {
                    eprintln!("[discovery] swept {removed} stale peer(s)");
                    let snapshot = snapshot_peers(&state).await;
                    broadcast.broadcast(ServerMessage::StatePatch {
                        timestamp: now(),
                        domain: "discoveredPeers".to_string(),
                        entity_id: None,
                        version: 0,
                        patch: serde_json::json!({ "peers": snapshot }),
                    });
                }
            }
            _ = rebrowse.tick() => {
                let _ = daemon.stop_browse(SERVICE_TYPE);
                match daemon.browse(SERVICE_TYPE) {
                    Ok(rx) => browse_rx = rx,
                    Err(e) => eprintln!("[discovery] auto re-browse failed: {e}"),
                }
            }
        }
    }
}

async fn sweep_stale(state: &SharedState, fullname_to_id: &mut HashMap<String, String>) -> usize {
    let cutoff = SystemTime::now() - STALE_AFTER;
    let mut guard = state.lock().await;
    let stale: Vec<String> = guard
        .discovered_peers
        .iter()
        .filter(|(_, p)| p.last_seen < cutoff)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &stale {
        guard.discovered_peers.remove(id);
    }
    // Also drop the reverse lookup entries for cleanly removed peers.
    fullname_to_id.retain(|_, peer_id| !stale.contains(peer_id));
    stale.len()
}

async fn remove_self_discovery_entries(
    state: &SharedState,
    fullname_to_id: &mut HashMap<String, String>,
    our_fullname: Option<&str>,
    our_fp: Option<&str>,
) -> usize {
    let mut ids = Vec::<String>::new();
    if let Some(fullname) = our_fullname {
        if let Some(id) = fullname_to_id.remove(fullname) {
            ids.push(id);
        }
    }

    let mut guard = state.lock().await;
    if let Some(fp) = our_fp {
        ids.extend(
            guard
                .discovered_peers
                .iter()
                .filter(|(_, peer)| peer.pubkey_fp.as_deref() == Some(fp))
                .map(|(id, _)| id.clone()),
        );
    }

    ids.sort();
    ids.dedup();
    let mut removed = 0usize;
    for id in ids {
        if guard.discovered_peers.remove(&id).is_some() {
            removed += 1;
        }
    }
    removed
}

fn build_service_info(
    instance: &str,
    port: u16,
    txt: &[(String, String)],
) -> Result<ServiceInfo, String> {
    let host = hostname_or_fallback(instance);
    let host_name = format!("{}.local.", host);

    let props: HashMap<String, String> = txt.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    // Empty IPs + `enable_addr_auto()` tells mdns-sd to discover local
    // interface addresses itself and keep them up to date as interfaces
    // come and go. Without this the registered service has no A/AAAA
    // records and remote peers can't reach us.
    let info = ServiceInfo::new(SERVICE_TYPE, instance, &host_name, (), port, props)
        .map_err(|e| e.to_string())?
        .enable_addr_auto();
    Ok(info)
}

fn hostname_or_fallback(fallback: &str) -> String {
    // Prefer the OS-reported hostname (uname / /etc/hostname / GetComputerName).
    // Fall back through HOSTNAME / COMPUTERNAME env vars, then the caller's
    // hint. A unique-per-host name matters here: mDNS instance names must
    // not collide on a LAN or one peer's advertisement evicts the other.
    if let Ok(name) = read_hostname() {
        if !name.trim().is_empty() {
            return name;
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| fallback.to_string())
}

fn read_hostname() -> std::io::Result<String> {
    #[cfg(unix)]
    {
        // Read /proc/sys/kernel/hostname on Linux; /etc/hostname on most
        // unices; fall back to `hostname` command output. /etc/hostname is
        // present on macOS too but may not match the runtime name; try
        // `uname -n` via libc as the canonical source.
        let mut buf = [0u8; 256];
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
        if rc == 0 {
            let cstr = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const _) };
            return Ok(cstr.to_string_lossy().into_owned());
        }
        Err(std::io::Error::last_os_error())
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "hostname lookup not implemented on this platform",
        ))
    }
}

/// Returns `true` if the discovered set changed and a patch should be emitted.
async fn handle_event(
    event: ServiceEvent,
    state: &SharedState,
    fullname_to_id: &mut HashMap<String, String>,
    our_fp: Option<&str>,
) -> bool {
    match event {
        ServiceEvent::SearchStarted(ty) => {
            eprintln!("[discovery] search started for {ty}");
            false
        }
        ServiceEvent::ServiceFound(_, fullname) => {
            eprintln!("[discovery] found '{fullname}' (resolving…)");
            false
        }
        ServiceEvent::ServiceResolved(info) => {
            let fullname = info.get_fullname();
            let fp = info.get_property_val_str("fp").map(|s| s.to_string());
            // Skip our own advertisement. Do not use the mDNS fullname here:
            // two fbros can legitimately advertise the same fallback instance
            // name (`fbro`) before mdns-sd resolves the conflict.
            if matches!((our_fp, fp.as_deref()), (Some(ours), Some(theirs)) if ours == theirs) {
                return remove_self_discovery_entries(
                    state,
                    fullname_to_id,
                    Some(fullname),
                    our_fp,
                )
                .await
                    > 0;
            }
            let allow_ipv6 = {
                let guard = state.lock().await;
                guard.share_config.allow_ipv6
            };
            // Prefer IPv4 (smaller link-local fan-out, no zone-id
            // hassles). IPv6 is opt-in for now because some local fbro
            // deployments fail to connect reliably over v6.
            let host = if let Some(v4) = info.get_addresses_v4().iter().next() {
                v4.to_string()
            } else if allow_ipv6 {
                if let Some(v6) = info
                    .get_addresses()
                    .iter()
                    .find(|ip| ip.is_ipv6())
                {
                    // IPv6 string with brackets when used in URLs/hostname
                    // forms, but `russh::client::connect` takes a
                    // `(host, port)` tuple — no brackets needed there. We
                    // also strip the zone-id suffix (`%enX`) since russh
                    // doesn't handle it; the user can still reach the peer
                    // via the global v6 if both are on the same /64.
                    let raw = v6.to_string();
                    raw.split('%').next().unwrap_or(&raw).to_string()
                } else {
                    eprintln!(
                        "[discovery] resolved '{}' but it has no usable address — skipping",
                        info.get_fullname()
                    );
                    return false;
                }
            } else {
                eprintln!(
                    "[discovery] resolved '{}' with no IPv4 address and IPv6 disabled — skipping",
                    info.get_fullname()
                );
                return false;
            };
            eprintln!(
                "[discovery] resolved '{}' at {host}:{} (fp={})",
                fullname,
                info.get_port(),
                fp.as_deref().unwrap_or("?")
            );
            let port = info.get_port();
            let name = info
                .get_property_val_str("name")
                .map(|s| s.to_string())
                .unwrap_or_else(|| info.get_fullname().to_string());
            let os = info.get_property_val_str("os").map(|s| s.to_string());
            let id = match fp.as_deref() {
                Some(fp) => format!("fbro:{fp}"),
                None => format!("mdns:{host}:{port}"),
            };
            let peer = DiscoveredPeer {
                id: id.clone(),
                name,
                host: host.clone(),
                port,
                os,
                pubkey_fp: fp.clone(),
                last_seen: SystemTime::now(),
            };
            fullname_to_id.insert(fullname.to_string(), id.clone());
            let peers_to_persist = {
                let mut guard = state.lock().await;
                guard.discovered_peers.insert(id, peer);
                if let Some(fp) = fp.as_deref() {
                    if let Some(paired) = guard.share_config.paired_peers.get_mut(fp) {
                        let now = SystemTime::now();
                        paired.last_host = Some(host);
                        paired.last_sftp_port = Some(port);
                        paired.last_pair_port = Some(crate::remote::pair_transport::PAIR_PORT);
                        paired.last_endpoint_seen_at = Some(now);
                        Some(guard.share_config.paired_peers.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(peers) = peers_to_persist {
                if let Err(e) = crate::remote::pairing::save_peers(
                    &crate::remote::pairing::default_peers_path(),
                    &peers,
                ) {
                    eprintln!("[discovery] failed to persist paired endpoint: {e}");
                }
            }
            // Always emit a patch on every successful resolve. Previously
            // we only emitted on structural changes (host/port/name),
            // which meant `last_seen` updates never reached the FE —
            // peers got dimmed as "stale" forever even though mDNS kept
            // resolving them.
            true
        }
        ServiceEvent::ServiceRemoved(_ty, fullname) => {
            eprintln!("[discovery] removed '{fullname}'");
            let Some(id) = fullname_to_id.remove(&fullname) else {
                return false;
            };
            let mut guard = state.lock().await;
            guard.discovered_peers.remove(&id).is_some()
        }
        ServiceEvent::SearchStopped(ty) => {
            eprintln!("[discovery] search stopped for {ty}");
            false
        }
        // mdns-sd's event enum is non-exhaustive.
        _ => false,
    }
}

async fn snapshot_peers(state: &SharedState) -> Vec<DiscoveredPeerSchema> {
    let guard = state.lock().await;
    let local_fp = guard
        .identity
        .as_ref()
        .map(|identity| identity.fingerprint.as_str());
    let paired_fps: std::collections::HashSet<&str> = guard
        .share_config
        .paired_peers
        .values()
        .map(|p| p.fingerprint.as_str())
        .collect();
    let mut out: Vec<DiscoveredPeerSchema> = guard
        .discovered_peers
        .values()
        .filter(|p| p.pubkey_fp.as_deref() != local_fp)
        .map(|p| DiscoveredPeerSchema {
            id: p.id.clone(),
            name: p.name.clone(),
            host: p.host.clone(),
            port: p.port,
            os: p.os.clone(),
            pubkey_fp: p.pubkey_fp.clone(),
            paired: p
                .pubkey_fp
                .as_deref()
                .map(|fp| paired_fps.contains(fp))
                .unwrap_or(false),
            last_seen: p
                .last_seen
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

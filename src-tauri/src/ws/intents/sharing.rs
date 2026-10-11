//! Intent handlers for the sharing surface — toggling whether this fbro
//! advertises on the LAN, picking which sources to expose, and managing
//! the paired-peer trust list.
//!
//! Pairing runs over the sibling TCP listener in `remote::pair_transport`:
//! peers exchange pubkeys/nonces, display a 6-digit code, and persist trust
//! only after both users confirm.

use std::collections::HashSet;
use std::ffi::CStr;
use std::sync::Arc;

use serde_json::Value;

use crate::remote::keys::{self, Identity};
use crate::remote::pairing;
use crate::remote::share_state::{self, ShareState};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{build_sources_json, IntentContext};

pub async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "EnableSharing" => enable_sharing(args, ctx).await,
        "SetSharingIpv6" => set_ipv6_enabled(args, ctx).await,
        "SetSharedTargets" => set_shared_targets(args, ctx).await,
        "AddSharedFolder" => add_shared_folder(args, ctx).await,
        "SetSharedSources" => set_shared_sources(args, ctx).await,
        "StartPairing" => start_pairing(args, ctx).await,
        "ConfirmPairing" => confirm_pairing(args, ctx).await,
        "RevokePairing" => revoke_pairing(args, ctx).await,
        "RefreshDiscoveredPeers" => refresh_peers(ctx).await,
        other => eprintln!("[sharing] unknown intent {other}"),
    }
}

async fn start_pairing(args: &Value, ctx: &mut IntentContext<'_>) {
    // Dedupe parallel pair attempts. If both peers click Pair at once,
    // each side ends up with two parallel handshakes for the same peer
    // (one outbound, one inbound), which guarantees one of them times
    // out. Easier to refuse a second concurrent handshake on the same
    // side than to teach the transport to merge them.
    {
        let state = ctx.state.lock().await;
        if !state.active_handshakes.is_empty() {
            drop(state);
            send_error(
                ctx,
                "sharing.pair.in-progress",
                "another pairing handshake is already in progress — wait for it to finish or cancel it",
            )
            .await;
            return;
        }
    }

    // Prefer explicit host/IP pairing. mDNS is only a hint; pairing must
    // remain possible when multicast discovery is broken.
    let (host, pair_port, endpoint_hint) =
        if let Some(address) = args.get("address").and_then(Value::as_str) {
            let Some((host, pair_port)) =
                parse_host_port(address, crate::remote::pair_transport::PAIR_PORT)
            else {
                send_error(
                    ctx,
                    "sharing.pair.bad-address",
                    "address must be host, host:port, or [ipv6]:port",
                )
                .await;
                return;
            };
            if ipv6_disabled_for_host(ctx, &host).await {
                send_error(
                    ctx,
                    "sharing.pair.ipv6-disabled",
                    "IPv6 is disabled in Sharing settings",
                )
                .await;
                return;
            }
            let sftp_port = args
                .get("sftpPort")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .unwrap_or(crate::remote::server::DEFAULT_PORT);
            (host.clone(), pair_port, Some((host, sftp_port)))
        } else {
            let Some(peer_id) = args.get("peerId").and_then(Value::as_str) else {
                send_error(
                    ctx,
                    "sharing.pair.missing-peer",
                    "peerId or address required",
                )
                .await;
                return;
            };
            let state = ctx.state.lock().await;
            let Some(dp) = state.discovered_peers.get(peer_id) else {
                drop(state);
                send_error(
                    ctx,
                    "sharing.pair.unknown-peer",
                    "peer not currently discoverable on the LAN",
                )
                .await;
                return;
            };
            (
                dp.host.clone(),
                crate::remote::pair_transport::PAIR_PORT,
                Some((dp.host.clone(), dp.port)),
            )
        };

    let identity = match ensure_identity(ctx).await {
        Ok(id) => id,
        Err(e) => {
            send_error(ctx, "sharing.pair.identity", &e).await;
            return;
        }
    };

    // Spawn the handshake task; it drives the protocol and emits
    // PairingRequested/PairingResolved to the FE on its own.
    let state = ctx.state.clone();
    let tx = ctx.tx.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::remote::pair_transport::initiate(
            state.clone(),
            identity,
            host,
            pair_port,
            endpoint_hint,
        )
        .await
        {
            eprintln!("[sharing] initiate failed: {e}");
            let _ = crate::notifications::send_feedback(&state, &tx, ServerMessage::Error {
                    timestamp: now(),
                    code: "sharing.pair.connect".to_string(),
                    message: format!("Pairing connection failed: {e}"),
                    recoverable: true,
                    extra: None,
                })
                .await;
        }
    });
}

fn parse_host_port(input: &str, default_port: u16) -> Option<(String, u16)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']')?;
        let port = if let Some(port) = suffix.strip_prefix(':') {
            port.parse::<u16>().ok()?
        } else if suffix.is_empty() {
            default_port
        } else {
            return None;
        };
        return Some((host.to_string(), port));
    }
    match trimmed.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let parsed_port = port.parse::<u16>().ok()?;
            if host.trim().is_empty() {
                None
            } else {
                Some((host.trim().to_string(), parsed_port))
            }
        }
        _ => Some((trimmed.to_string(), default_port)),
    }
}

async fn confirm_pairing(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(handshake_id) = args.get("handshakeId").and_then(Value::as_str) else {
        send_error(ctx, "sharing.confirm.missing-id", "handshakeId required").await;
        return;
    };
    let accept = args.get("accept").and_then(Value::as_bool).unwrap_or(false);
    let tx = {
        let mut state = ctx.state.lock().await;
        state.active_handshakes.remove(handshake_id)
    };
    if let Some(tx) = tx {
        // Errors from `send` only happen if the receiver was dropped —
        // i.e. the handshake task already concluded (timeout). Nothing
        // useful to surface in that case.
        let _ = tx.send(accept);
    } else {
        send_error(
            ctx,
            "sharing.confirm.unknown",
            "no active handshake with that id (likely already resolved or timed out)",
        )
        .await;
    }
}

async fn refresh_peers(ctx: &mut IntentContext<'_>) {
    let (enabled, handle) = {
        let state = ctx.state.lock().await;
        (state.share_config.enabled, state.discovery_handle.clone())
    };
    if !enabled {
        send_error(ctx, "sharing.refresh.disabled", "remote access is off").await;
        return;
    }
    let Some(h) = handle else {
        send_error(
            ctx,
            "sharing.refresh.no-discovery",
            "discovery task is not running",
        )
        .await;
        return;
    };
    h.refresh().await;
}

async fn enable_sharing(args: &Value, ctx: &mut IntentContext<'_>) {
    let enabled = args
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Ensure identity is loaded — needed both to start the server and to
    // advertise our fingerprint in mDNS TXT records.
    if ensure_identity(ctx).await.is_err() {
        send_error(
            ctx,
            "sharing.identity-load",
            "failed to load or generate ~/.config/fbro/id_ed25519",
        )
        .await;
        return;
    }

    {
        let mut state = ctx.state.lock().await;
        state.share_config.enabled = enabled;
    }
    persist_share_state(ctx).await;

    if enabled {
        if let Err(e) = start_sharing_server(ctx).await {
            send_error(ctx, "sharing.server-start", &e).await;
            return;
        }
    } else {
        stop_sharing_server(ctx).await;
    }

    broadcast_share_status(ctx).await;
}

async fn persist_share_state(ctx: &mut IntentContext<'_>) {
    let snapshot = {
        let state = ctx.state.lock().await;
        ShareState {
            enabled: state.share_config.enabled,
            allow_ipv6: state.share_config.allow_ipv6,
            shared_source_ids: state.share_config.shared_source_ids.clone(),
            shared_roots: state.share_config.shared_roots.clone(),
        }
    };
    let saved = tokio::task::spawn_blocking(move || share_state::save(&share_state::default_share_path(), &snapshot)).await;
    if !matches!(&saved, Ok(Ok(()))) {
        // Don't fail the intent — sharing still works in memory; the user
        // just loses the auto-restore on next launch.
        eprintln!("[sharing] failed to persist share.json: {saved:?}");
    }
}

async fn set_ipv6_enabled(args: &Value, ctx: &mut IntentContext<'_>) {
    let allow_ipv6 = args
        .get("allowIpv6")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut dropped_discovered = false;
    {
        let mut state = ctx.state.lock().await;
        state.share_config.allow_ipv6 = allow_ipv6;
        if !allow_ipv6 {
            let before = state.discovered_peers.len();
            state
                .discovered_peers
                .retain(|_, peer| !is_ipv6_literal(&peer.host));
            dropped_discovered = state.discovered_peers.len() != before;
        }
    }
    persist_share_state(ctx).await;
    if dropped_discovered {
        let peers = {
            let state = ctx.state.lock().await;
            build_discovered_peers_json(&state)
        };
        let _ = ctx
            .tx
            .send(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "discoveredPeers".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({ "peers": peers }),
            })
            .await;
    }
    broadcast_share_status(ctx).await;
}

async fn ipv6_disabled_for_host(ctx: &mut IntentContext<'_>, host: &str) -> bool {
    if !is_ipv6_literal(host) {
        return false;
    }
    let state = ctx.state.lock().await;
    !state.share_config.allow_ipv6
}

fn is_ipv6_literal(host: &str) -> bool {
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_ipv6())
        .unwrap_or(false)
}

fn build_discovered_peers_json(state: &crate::state::AppState) -> Vec<serde_json::Value> {
    let paired_fps: std::collections::HashSet<&str> = state
        .share_config
        .paired_peers
        .values()
        .map(|p| p.fingerprint.as_str())
        .collect();
    let mut peers: Vec<serde_json::Value> = state
        .discovered_peers
        .values()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "host": p.host,
                "port": p.port,
                "os": p.os,
                "pubkeyFp": p.pubkey_fp,
                "paired": p
                    .pubkey_fp
                    .as_deref()
                    .map(|fp| paired_fps.contains(fp))
                    .unwrap_or(false),
                "lastSeen": p
                    .last_seen
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            })
        })
        .collect();
    peers.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });
    peers
}

async fn set_shared_sources(args: &Value, ctx: &mut IntentContext<'_>) {
    let ids: HashSet<String> = args
        .get("sourceIds")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let roots = {
        let state = ctx.state.lock().await;
        ids.into_iter()
            .filter_map(|sid| state.sources.get(&sid))
            .filter(|src| src.kind == "filesystem" && src.mounted && !src.shadow)
            .map(crate::state::shared_root_for_source)
            .collect()
    };
    update_shared_roots(roots, ctx).await;
}

async fn set_shared_targets(args: &Value, ctx: &mut IntentContext<'_>) {
    let ids: HashSet<String> = args
        .get("targetIds")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let roots = {
        let state = ctx.state.lock().await;
        let home = state.platform.home_dir();
        let mut candidates =
            crate::state::share_suggestions(&state.sources, &state.bookmarks, &home);
        for root in &state.share_config.shared_roots {
            if !candidates.iter().any(|candidate| candidate.id == root.id) {
                candidates.push(root.clone());
            }
        }
        candidates
            .into_iter()
            .filter(|root| ids.contains(&root.id))
            .filter_map(|mut root| {
                if matches!(root.origin, crate::state::SharedRootOrigin::Folder) {
                    return Some(root);
                }
                let source_id = crate::state::source_id_for_local_path(&state.sources, &root.path)?;
                root.source_id = source_id;
                Some(root)
            })
            .collect::<Vec<_>>()
    };
    update_shared_roots(roots, ctx).await;
}

async fn update_shared_roots(roots: Vec<crate::state::SharedRoot>, ctx: &mut IntentContext<'_>) {
    let filtered_source_ids: HashSet<String> = roots
        .iter()
        .filter(|root| matches!(root.origin, crate::state::SharedRootOrigin::Source))
        .map(|root| root.source_id.clone())
        .collect();
    {
        let mut state = ctx.state.lock().await;
        crate::remote::shared_folder::sync_drivers(&mut state.drivers, &roots);
        state.share_config.shared_source_ids = filtered_source_ids;
        state.share_config.shared_roots = roots;
    }
    persist_share_state(ctx).await;
    broadcast_share_status(ctx).await;
}

async fn add_shared_folder(args: &Value, ctx: &mut IntentContext<'_>) {
    use super::arg_str;
    let request_id = arg_str(args, "requestId").unwrap_or_default().to_string();
    let raw = arg_str(args, "path").unwrap_or_default().to_string();
    let (base, home, existing) = {
        let state = ctx.state.lock().await;
        let home = state.platform.home_dir();
        let panel = arg_str(args, "panelId").and_then(|id| state.panels.get(id));
        let base = panel.filter(|panel| {
            state.sources.get(&panel.source_id).is_some_and(|source|
                source.kind == "filesystem" && source.mounted && !source.shadow)
        }).map(|panel| panel.path.clone()).unwrap_or_else(|| home.clone());
        (base, home, state.share_config.shared_roots.clone())
    };
    let result = tokio::task::spawn_blocking(move ||
        crate::remote::shared_folder::validate(&raw, &base, &home, &existing)
    ).await;
    let root = match result {
        Ok(Ok(root)) => root,
        result => {
            let (code, message) = match result {
                Ok(Err(error)) => (error.code, error.message),
                Err(error) => ("FOLDER_UNAVAILABLE", format!("Cannot validate folder: {error}")),
                Ok(Ok(_)) => unreachable!(),
            };
            let _ = ctx.tx.send(ServerMessage::AddSharedFolderFailed {
                timestamp: now(), request_id, code: code.into(), message,
            }).await;
            return;
        }
    };
    // Merge with live state after validation; don't replace another connection's picks.
    let root = {
        let mut state = ctx.state.lock().await;
        if let Some(existing) = state.share_config.shared_roots.iter()
            .find(|existing| existing.id == root.id || existing.path == root.path)
        {
            existing.clone()
        } else {
            state.share_config.shared_roots.push(root.clone());
            let roots = state.share_config.shared_roots.clone();
            crate::remote::shared_folder::sync_drivers(&mut state.drivers, &roots);
            root
        }
    };
    persist_share_state(ctx).await;
    broadcast_share_status(ctx).await;
    let _ = ctx.send_feedback(ServerMessage::AddSharedFolderOk {
        timestamp: now(), request_id, target_id: root.id, path: root.path,
    }).await;
}

async fn revoke_pairing(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(fp) = args.get("fingerprint").and_then(Value::as_str) else {
        send_error(ctx, "sharing.revoke.missing-fp", "fingerprint required").await;
        return;
    };
    let fp = fp.to_string();
    // Before touching state, snapshot where the peer is on the LAN so we
    // can send them a Revoke notice. After we drop the pairing locally
    // we no longer have a stable way to find them.
    let target_endpoint: Option<(String, u16)> = {
        let state = ctx.state.lock().await;
        state
            .discovered_peers
            .values()
            .find(|p| p.pubkey_fp.as_deref() == Some(fp.as_str()))
            .map(|p| (p.host.clone(), p.port))
    };
    let identity = ensure_identity(ctx).await.ok();

    {
        let mut state = ctx.state.lock().await;
        state.share_config.paired_peers.remove(&fp);
        // Best-effort persist; if the write fails, the revoke is still
        // effective in-memory for this session.
        let snapshot = state.share_config.paired_peers.clone();
        if let Err(e) = pairing::save_peers(&pairing::default_peers_path(), &snapshot) {
            eprintln!("[sharing] failed to persist peers.json after revoke: {e}");
        }
    }

    // Fire-and-forget: tell the other side. They use the signed payload
    // to verify the notice is genuine before tearing down their mount.
    // Pair port matches our own (constant). Best-effort — if the peer is
    // offline the notice never arrives and their session will just sit
    // there failing to reconnect until they restart fbro.
    if let (Some(id), Some((host, _sftp_port))) = (identity, target_endpoint) {
        let pair_port = crate::remote::pair_transport::PAIR_PORT;
        let fp_for_task = fp.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::remote::pair_transport::send_revoke(id, host, pair_port, fp_for_task.clone())
                    .await
            {
                eprintln!("[sharing] revoke notice to {fp_for_task} failed: {e}");
            }
        });
    }
    // Also drop any active remote sources that were owned by this peer,
    // since the next reconnect would fail at auth anyway and leaving a
    // ghost in the volume bar is confusing.
    let removed_sources: Vec<String> = {
        let mut state = ctx.state.lock().await;
        let ids: Vec<String> = state
            .sources
            .iter()
            .filter(|(_, s)| s.peer_id.as_deref() == Some(fp.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            state.sources.remove(id);
            state.drivers.remove(id);
        }
        ids
    };
    if !removed_sources.is_empty() {
        let _ = ctx
            .tx
            .send(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "sources".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({
                    "sources": build_sources_json(&*ctx.state.lock().await),
                }),
            })
            .await;
    }
    broadcast_share_status(ctx).await;
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Load the local Ed25519 identity into `state.identity` if not already
/// present. Returns the `Arc<Identity>` either way.
pub async fn ensure_identity(ctx: &mut IntentContext<'_>) -> Result<Arc<Identity>, String> {
    ensure_identity_for(&ctx.state).await
}

/// Variant that works against a raw `SharedState` — used by startup code in
/// `main.rs` which doesn't have an `IntentContext` yet.
pub async fn ensure_identity_for(
    state: &crate::state::SharedState,
) -> Result<Arc<Identity>, String> {
    {
        let g = state.lock().await;
        if let Some(id) = &g.identity {
            return Ok(id.clone());
        }
    }
    let path = keys::default_key_path();
    let identity = keys::load_or_generate(&path).map_err(|e| e.to_string())?;
    let arc = Arc::new(identity);
    let mut g = state.lock().await;
    g.identity = Some(arc.clone());
    Ok(arc)
}

async fn start_sharing_server(ctx: &mut IntentContext<'_>) -> Result<(), String> {
    start_sharing_server_for(&ctx.state).await
}

async fn ensure_discovery_for(state: &crate::state::SharedState) -> Result<(), String> {
    {
        let g = state.lock().await;
        if !g.share_config.enabled {
            return Err("remote access is off".to_string());
        }
        if g.discovery_handle.is_some() {
            return Ok(());
        }
    }

    let broadcaster = Arc::new(crate::remote::discovery::GuiFanout {
        state: state.clone(),
    });
    let handle =
        crate::remote::discovery::spawn(state.clone(), broadcaster).map_err(|e| e.to_string())?;
    let should_keep = {
        let mut g = state.lock().await;
        if g.discovery_handle.is_none() {
            g.discovery_handle = Some(handle.clone());
            true
        } else {
            false
        }
    };
    if !should_keep {
        handle.shutdown().await;
    }
    Ok(())
}

/// State-only variant of the listener bring-up. Idempotent: returns
/// `Ok(())` immediately if the server handle is already populated.
pub async fn start_sharing_server_for(state: &crate::state::SharedState) -> Result<(), String> {
    use crate::remote::server::{spawn, GuiSessionEvents, DEFAULT_PORT};

    // Avoid double-spawning if the user re-toggles quickly, or if the
    // startup auto-restore races with a user click.
    {
        let g = state.lock().await;
        if g.remote_server_handle.is_some() {
            return Ok(());
        }
    }

    let identity = ensure_identity_for(state).await?;
    let events = Arc::new(GuiSessionEvents {
        state: state.clone(),
    });
    let handle = spawn(state.clone(), identity.clone(), DEFAULT_PORT, events)
        .await
        .map_err(|e| e.to_string())?;
    {
        let mut g = state.lock().await;
        g.remote_server_handle = Some(handle);
    }

    // Bring up the pairing TCP listener on its sister port so peers can
    // initiate handshakes against us. Failure here is fatal for the
    // EnableSharing call — without a listener, nobody can pair with us.
    let pair_handle =
        crate::remote::pair_transport::spawn_listener(state.clone(), identity.clone())
            .await
            .map_err(|e| format!("pair listener: {e}"))?;
    {
        let mut g = state.lock().await;
        g.pair_listener_handle = Some(pair_handle);
    }

    // Start LAN discovery only while remote access is enabled. The listener is
    // still reachable by direct address if discovery startup fails.
    if let Err(e) = ensure_discovery_for(state).await {
        eprintln!("[sharing] discovery startup failed: {e}");
    }

    // Also advertise via mDNS if discovery is running.
    let advertise = {
        let g = state.lock().await;
        g.discovery_handle.clone()
    };
    if let Some(d) = advertise {
        let os = os_label();
        let hostname = system_hostname().unwrap_or_else(|| "fbro".to_string());
        let display_name = advertised_display_name(&hostname, os, &identity.fingerprint);
        let txt = vec![
            ("fbro".to_string(), "1".to_string()),
            ("name".to_string(), display_name.clone()),
            ("os".to_string(), os.to_string()),
            ("fp".to_string(), identity.fingerprint.clone()),
        ];
        d.enable_advertise(display_name, DEFAULT_PORT, txt).await;
    }
    Ok(())
}

fn os_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

fn system_hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
        if rc == 0 {
            let raw = unsafe { CStr::from_ptr(buf.as_ptr() as *const _) }
                .to_string_lossy()
                .trim()
                .trim_end_matches(".local")
                .to_string();
            if !raw.is_empty() {
                return Some(raw);
            }
        }
    }

    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .map(|s| s.trim().trim_end_matches(".local").to_string())
        .filter(|s| !s.is_empty())
}

fn advertised_display_name(hostname: &str, os: &str, fingerprint: &str) -> String {
    let suffix = fingerprint
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(6)
        .collect::<String>();
    if suffix.is_empty() {
        format!("{hostname} ({os})")
    } else {
        format!("{hostname} ({os}, {suffix})")
    }
}

async fn stop_sharing_server(ctx: &mut IntentContext<'_>) {
    let (handle, pair_handle, discovery) = {
        let mut state = ctx.state.lock().await;
        (
            state.remote_server_handle.take(),
            state.pair_listener_handle.take(),
            state.discovery_handle.take(),
        )
    };
    if let Some(h) = handle {
        h.abort();
    }
    if let Some(h) = pair_handle {
        h.abort();
    }
    if let Some(d) = discovery {
        d.shutdown().await;
    }
    {
        let mut state = ctx.state.lock().await;
        state.discovered_peers.clear();
    }
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "discoveredPeers".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "peers": [] }),
        })
        .await;
}

async fn broadcast_share_status(ctx: &mut IntentContext<'_>) {
    let (message, senders) = {
        let mut state = ctx.state.lock().await;
        (crate::ws::runtime::build_share_config_patch(&mut state), state.gui_senders.clone())
    };
    for sender in senders {
        let _ = sender.send(message.clone()).await;
    }
}

async fn send_error(ctx: &mut IntentContext<'_>, code: &str, message: &str) {
    let _ = ctx.send_feedback(ServerMessage::Error {
            timestamp: now(),
            code: code.to_string(),
            message: message.to_string(),
            recoverable: true,
            extra: None,
        })
        .await;
}

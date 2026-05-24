//! Intent handlers for connecting / disconnecting remote (SFTP) peers.
//!
//! `ConnectRemotePeer` flow:
//!  1. Look up the discovered peer by `peerId`. Must carry a fingerprint
//!     (i.e. `fbro=1` in TXT) and be present in `paired_peers`.
//!  2. Build one `SftpFsDriver` per shared source the peer exposes. In v1
//!     we discover those by listing the SFTP root (`/`) — each directory
//!     there is one shared source on the peer side. (Future: use the
//!     `fbro@anthropic.com/list-sources` extended request for richer
//!     metadata.)
//!  3. Register a `Source { kind: "remote", read_only: true, peer_id, … }`
//!     for each one and push a `sources` patch.
//!
//! `DisconnectRemotePeer` flow: drop the driver(s) + source(s) for that
//! peer and push a patch.

use std::sync::Arc;

use serde_json::Value;

use crate::remote::keys::Identity;
use crate::state::{PairedPeer, Source};
use crate::vfs::sftp::{SftpDriverConfig, SftpFsDriver};
use crate::vfs::VfsDriver;
use crate::ws::intents::sharing::ensure_identity;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{build_sources_json, IntentContext};

pub async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ConnectRemotePeer" => connect(args, ctx).await,
        "DisconnectRemotePeer" => disconnect(args, ctx).await,
        "ReconnectRemoteSource" => reconnect_source(args, ctx).await,
        "RetryLastPeer" => retry_last_peer(args, ctx).await,
        other => eprintln!("[remote] unknown intent {other}"),
    }
}

async fn reconnect_source(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(source_id) = args.get("sourceId").and_then(Value::as_str) else {
        send_error(ctx, "remote.reconnect.missing-source", "sourceId required").await;
        return;
    };
    // Grab the driver Arc under the lock, downcast to SftpFsDriver, then
    // fire force_reconnect. The driver itself owns the runtime + supervisor,
    // so we don't need to hold the lock past the downcast.
    let driver = {
        let state = ctx.state.lock().await;
        state.drivers.get(source_id).cloned()
    };
    let Some(driver) = driver else {
        send_error(
            ctx,
            "remote.reconnect.unknown-source",
            "no driver registered for that source",
        )
        .await;
        return;
    };
    match driver
        .as_any()
        .downcast_ref::<crate::vfs::sftp::SftpFsDriver>()
    {
        Some(sftp) => sftp.force_reconnect(),
        None => {
            send_error(
                ctx,
                "remote.reconnect.not-remote",
                "source is not a remote SFTP mount",
            )
            .await;
        }
    }
}

async fn connect(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(peer_id) = args.get("peerId").and_then(Value::as_str) else {
        send_error(ctx, "remote.connect.missing-peer-id", "peerId required").await;
        return;
    };

    // Resolve peer endpoint + paired-peer record. Both must be present:
    // we need an mDNS-resolved (host, port) and an entry in paired_peers
    // (which gives us the expected host fingerprint = the peer's pubkey
    // fingerprint, since the same key is host & client identity).
    let (host, port, paired) = {
        let state = ctx.state.lock().await;
        let Some(dp) = state.discovered_peers.get(peer_id) else {
            send_error(
                ctx,
                "remote.connect.unknown-peer",
                "peer not currently discoverable on the LAN",
            )
            .await;
            return;
        };
        let Some(fp) = dp.pubkey_fp.clone() else {
            send_error(
                ctx,
                "remote.connect.no-fingerprint",
                "discovered peer does not advertise a fingerprint (likely a non-fbro sshd; not supported in v1)",
            )
            .await;
            return;
        };
        let Some(paired) = state.share_config.paired_peers.get(&fp).cloned() else {
            send_error(
                ctx,
                "remote.connect.not-paired",
                "peer is not in paired_peers — pair via the Sharing settings first",
            )
            .await;
            return;
        };
        (dp.host.clone(), dp.port, paired)
    };

    match connect_to_endpoint(ctx, host.clone(), port, paired.clone()).await {
        Ok(()) => {
            update_paired_endpoint(ctx, &paired.fingerprint, host, port).await;
            push_share_config_patch(ctx).await;
        }
        Err(e) => send_error(ctx, "remote.connect.failed", &e).await,
    }
}

async fn retry_last_peer(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(fingerprint) = args.get("fingerprint").and_then(Value::as_str) else {
        send_error(
            ctx,
            "remote.retry.missing-fingerprint",
            "fingerprint required",
        )
        .await;
        return;
    };

    let (host, port, paired) = {
        let state = ctx.state.lock().await;
        let Some(paired) = state.share_config.paired_peers.get(fingerprint).cloned() else {
            send_error(
                ctx,
                "remote.retry.unknown-peer",
                "peer is not in paired_peers",
            )
            .await;
            return;
        };
        let Some(host) = paired.last_host.clone() else {
            send_error(
                ctx,
                "remote.retry.no-endpoint",
                "paired peer has no last known address yet",
            )
            .await;
            return;
        };
        let Some(port) = paired.last_sftp_port else {
            send_error(
                ctx,
                "remote.retry.no-endpoint",
                "paired peer has no last known SFTP port yet",
            )
            .await;
            return;
        };
        (host, port, paired)
    };

    match connect_to_endpoint(ctx, host.clone(), port, paired.clone()).await {
        Ok(()) => {
            update_paired_endpoint(ctx, &paired.fingerprint, host, port).await;
            push_share_config_patch(ctx).await;
        }
        Err(e) => send_error(ctx, "remote.retry.failed", &e).await,
    }
}

async fn connect_to_endpoint(
    ctx: &mut IntentContext<'_>,
    host: String,
    port: u16,
    paired: PairedPeer,
) -> Result<(), String> {
    let identity = ensure_identity(ctx)
        .await
        .map_err(|e| format!("identity: {e}"))?;
    let allow_ipv6 = {
        let state = ctx.state.lock().await;
        state.share_config.allow_ipv6
    };

    // First, list the SFTP root with a probe driver to discover the shared
    // source ids. Empty path_prefix points the SFTP path at "/".
    // The probe is a short-lived driver — pass NoopStatusBroadcaster so
    // its connect/disconnect noise doesn't show up in the FE.
    let probe = match build_driver(
        &host,
        port,
        &identity,
        &paired,
        "",
        "probe",
        allow_ipv6,
        Arc::new(crate::vfs::sftp::NoopStatusBroadcaster),
    ) {
        Ok(d) => d,
        Err(e) => return Err(format!("driver build: {e}")),
    };
    let probe_arc: Arc<dyn VfsDriver> = Arc::new(probe);
    let listing = {
        let probe = probe_arc.clone();
        tokio::task::spawn_blocking(move || {
            let sort = crate::ws::protocol::SortSpec {
                by: "name".to_string(),
                order: "asc".to_string(),
            };
            probe.list("", &sort, None, true)
        })
        .await
        .map_err(|e| e.to_string())
    };
    let listing = match listing {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => return Err(format!("list remote root: {e}")),
        Err(e) => return Err(format!("spawn blocking: {e}")),
    };
    // Drop the probe driver — we'll spawn fresh per-source drivers below so
    // their path_prefix is scoped correctly.
    drop(probe_arc);

    let supports = vec![
        "ListEntries".to_string(),
        "OpenFolder".to_string(),
        "CopySelection".to_string(),
        "InspectItem".to_string(),
    ];

    let mut registered: Vec<String> = Vec::new();
    for entry in listing
        .iter()
        .filter(|e| matches!(e.kind, crate::vfs::VfsKind::Directory) && !e.name.starts_with('.'))
    {
        let remote_source_id = entry.name.clone();
        let local_source_id = format!("remote:{}:{remote_source_id}", paired.fingerprint);
        let status: Arc<dyn crate::vfs::sftp::StatusBroadcaster> =
            Arc::new(crate::remote::server::GuiStatusBroadcaster {
                state: ctx.state.clone(),
            });
        let driver = match build_driver(
            &host,
            port,
            &identity,
            &paired,
            &format!("/{remote_source_id}"),
            &local_source_id,
            allow_ipv6,
            status,
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[remote] driver build for {local_source_id} failed: {e}");
                continue;
            }
        };
        let source = Source {
            id: local_source_id.clone(),
            name: format!("{} @ {}", remote_source_id, paired.name),
            kind: "remote".to_string(),
            supports: supports.clone(),
            root_path: String::new(),
            mounted: true,
            device_path: None,
            shadow: false,
            icon_name: Some("network-server".to_string()),
            sort_priority: 200,
            read_only: true,
            peer_id: Some(paired.fingerprint.clone()),
        };
        let mut state = ctx.state.lock().await;
        state.sources.insert(local_source_id.clone(), source);
        state.drivers.insert(
            local_source_id.clone(),
            Arc::new(driver) as Arc<dyn VfsDriver>,
        );
        registered.push(local_source_id);
    }

    if registered.is_empty() {
        return Err("peer is reachable but exposes no shared sources".to_string());
    }
    push_sources_patch(ctx).await;
    Ok(())
}

async fn disconnect(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(source_id) = args.get("sourceId").and_then(Value::as_str) else {
        send_error(ctx, "remote.disconnect.missing-source", "sourceId required").await;
        return;
    };
    // If `sourceId` looks like a remote-id, also drop sibling sources from
    // the same peer so the disconnect UX is "tear down everything from this
    // peer" rather than "leave half of them connected". The naming scheme
    // is "remote:<fingerprint>:<remote_id>" so we can recover the fp.
    let peer_fp = source_id
        .strip_prefix("remote:")
        .and_then(|rest| rest.split_once(':').map(|(fp, _)| fp.to_string()));
    let mut state = ctx.state.lock().await;
    let to_remove: Vec<String> = if let Some(fp) = peer_fp.as_deref() {
        state
            .sources
            .iter()
            .filter(|(_, s)| s.peer_id.as_deref() == Some(fp))
            .map(|(id, _)| id.clone())
            .collect()
    } else {
        vec![source_id.to_string()]
    };
    for id in &to_remove {
        state.sources.remove(id);
        state.drivers.remove(id);
    }
    drop(state);
    push_sources_patch(ctx).await;
}

fn build_driver(
    host: &str,
    port: u16,
    identity: &Arc<Identity>,
    paired: &PairedPeer,
    path_prefix: &str,
    source_id: &str,
    allow_ipv6: bool,
    status: Arc<dyn crate::vfs::sftp::StatusBroadcaster>,
) -> Result<SftpFsDriver, String> {
    SftpFsDriver::new(
        SftpDriverConfig {
            host: host.to_string(),
            port,
            user: "fbro".to_string(),
            identity: identity.clone(),
            expected_peer_fp: paired.fingerprint.clone(),
            path_prefix: path_prefix.to_string(),
            source_id: source_id.to_string(),
            allow_ipv6,
        },
        status,
    )
    .map_err(|e| e.to_string())
}

async fn update_paired_endpoint(
    ctx: &mut IntentContext<'_>,
    fingerprint: &str,
    host: String,
    sftp_port: u16,
) {
    let peers = {
        let mut state = ctx.state.lock().await;
        let Some(peer) = state.share_config.paired_peers.get_mut(fingerprint) else {
            return;
        };
        peer.last_host = Some(host);
        peer.last_sftp_port = Some(sftp_port);
        peer.last_pair_port = Some(crate::remote::pair_transport::PAIR_PORT);
        peer.last_endpoint_seen_at = Some(std::time::SystemTime::now());
        state.share_config.paired_peers.clone()
    };
    if let Err(e) =
        crate::remote::pairing::save_peers(&crate::remote::pairing::default_peers_path(), &peers)
    {
        eprintln!("[remote] failed to persist last endpoint: {e}");
    }
}

async fn push_sources_patch(ctx: &mut IntentContext<'_>) {
    let payload = {
        let state = ctx.state.lock().await;
        serde_json::json!({ "sources": build_sources_json(&state) })
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: payload,
        })
        .await;
}

async fn push_share_config_patch(ctx: &mut IntentContext<'_>) {
    let payload = {
        let state = ctx.state.lock().await;
        let paired: Vec<serde_json::Value> = state
            .share_config
            .paired_peers
            .values()
            .map(|p| {
                serde_json::json!({
                    "fingerprint": p.fingerprint,
                    "name": p.name,
                    "os": p.os,
                    "pairedAt": p
                        .paired_at
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                    "lastSeen": p
                        .last_seen
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                    "lastHost": p.last_host.clone(),
                    "lastSftpPort": p.last_sftp_port,
                    "lastPairPort": p.last_pair_port,
                    "lastEndpointSeenAt": p
                        .last_endpoint_seen_at
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                })
            })
            .collect();
        serde_json::json!({
            "enabled": state.share_config.enabled,
            "allowIpv6": state.share_config.allow_ipv6,
            "sharedSourceIds": state
                .share_config
                .shared_source_ids
                .iter()
                .collect::<Vec<_>>(),
            "pairedPeers": paired,
            "fingerprint": state.identity.as_ref().map(|i| i.fingerprint.clone()),
        })
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "shareConfig".to_string(),
            entity_id: None,
            version: 0,
            patch: payload,
        })
        .await;
}

async fn send_error(ctx: &mut IntentContext<'_>, code: &str, message: &str) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: code.to_string(),
            message: message.to_string(),
            recoverable: true,
            extra: None,
        })
        .await;
}

//! Pairing transport — line-delimited JSON over plain TCP.
//!
//! Lives on a sister port (7880) alongside the russh SFTP listener (7879).
//! The TCP socket carries both the pairing handshake AND the revoke
//! notice that lets one peer tear down the other side's trust + mount
//! when the user clicks Revoke.
//!
//! Wire framing: each side reads/writes one JSON object per line. Three
//! frame kinds:
//!
//! ```text
//!     { "type": "Hello", "v": 1, "pubkey": "<b64>", "nonce": "<b64>",
//!       "name": "<str>", "os": "<str>?", "role": "initiator" | "responder" }
//!     { "type": "Decision", "accept": true | false }
//!     { "type": "Revoke", "fromFp": "<fp>", "toFp": "<fp>",
//!       "timestamp": <u64 millis>, "signature": "<b64 ed25519 sig>" }
//! ```
//!
//! The responder reads the first frame on a fresh connection and
//! dispatches: `Hello` → handshake flow; `Revoke` → signature check +
//! local cleanup; `Decision` → protocol error.
//!
//! Flow (symmetric except who connects first):
//!  1. TCP accept / connect.
//!  2. Each side sends `Hello`. Initiator sends first, then waits; the
//!     responder reads first, then sends — but each treats the other side
//!     as opaque past the initial role tag.
//!  3. Both sides compute the same 6-digit code over the two pubkeys + the
//!     two nonces, register a handshake id in `state.active_handshakes`,
//!     and emit `PairingRequested` to their respective FE.
//!  4. Both wait (up to `HANDSHAKE_TTL`) for the local user's
//!     `ConfirmPairing { accept }`, then send a `Decision` frame.
//!  5. On *both-accept*: each side stores the other's pubkey in
//!     `paired_peers`, persists `peers.json`, and emits
//!     `PairingResolved { outcome: "paired", fingerprint }`.
//!  6. Anything else (either side rejects, timeout, socket drop) →
//!     `PairingResolved { outcome: "rejected" | "timeout" }` on both sides.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::remote::keys::{fingerprint_of, Identity};
use crate::remote::pairing::{default_peers_path, derive_code, save_peers, HANDSHAKE_TTL};
use crate::state::{PairedPeer, SharedState};
use crate::ws::protocol::{now, ServerMessage};

pub const PAIR_PORT: u16 = 7880;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum Frame {
    Hello {
        v: u32,
        pubkey: String,
        nonce: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        os: Option<String>,
        role: String,
    },
    Decision {
        accept: bool,
    },
    /// One-shot revoke notice: A tells B "I have removed you from my
    /// trust list, please reciprocate". `signature` is Ed25519 over
    /// `revoke_payload(from_fp, to_fp, timestamp)` using `from_fp`'s
    /// private key — verified on receipt against `paired_peers[from_fp].pubkey`.
    Revoke {
        #[serde(rename = "fromFp")]
        from_fp: String,
        #[serde(rename = "toFp")]
        to_fp: String,
        timestamp: u64,
        signature: String,
    },
}

/// Maximum age of a revoke notice we'll accept (5 minutes). Bounds the
/// replay window — a captured Revoke can't be redelivered indefinitely.
const REVOKE_MAX_AGE_MS: u64 = 5 * 60 * 1000;

/// Canonical bytes signed/verified for a Revoke. Stable across both
/// peers — keep this format frozen or version it explicitly.
fn revoke_payload(from_fp: &str, to_fp: &str, timestamp: u64) -> Vec<u8> {
    format!("fbro-revoke-v1\n{from_fp}\n{to_fp}\n{timestamp}").into_bytes()
}

/// Start the TCP listener on `0.0.0.0:7880`. Returns the JoinHandle so the
/// caller can abort on `EnableSharing { enabled: false }`.
pub async fn spawn_listener(
    state: SharedState,
    identity: Arc<Identity>,
) -> io::Result<tokio::task::JoinHandle<()>> {
    let listener = TcpListener::bind(("0.0.0.0", PAIR_PORT)).await?;
    eprintln!("[pair] listening on 0.0.0.0:{PAIR_PORT}");
    Ok(tokio::spawn(async move {
        loop {
            let (sock, addr) = match listener.accept().await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[pair] accept failed: {e} — exiting listener");
                    return;
                }
            };
            eprintln!("[pair] inbound from {addr}");
            let state = state.clone();
            let identity = identity.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_inbound(sock, state, identity).await {
                    eprintln!("[pair] inbound error: {e}");
                }
            });
        }
    }))
}

/// Initiate a pairing handshake against a discovered peer. Returns once
/// the protocol terminates (paired / rejected / timeout / error); the FE
/// is driven entirely via `PairingRequested` / `PairingResolved` events
/// over the WS, not via the return value.
pub async fn initiate(
    state: SharedState,
    identity: Arc<Identity>,
    host: String,
    port: u16,
    endpoint_hint: Option<(String, u16)>,
) -> Result<(), String> {
    eprintln!("[pair] connecting to {host}:{port}");
    let allow_ipv6 = {
        let guard = state.lock().await;
        guard.share_config.allow_ipv6
    };
    let endpoint = resolve_endpoint(&host, port, allow_ipv6).await?;
    let sock = timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(endpoint),
    )
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map_err(|e| format!("connect: {e}"))?;
    drive_handshake(sock, state, identity, true, endpoint_hint)
        .await
        .map_err(|e| e.to_string())
}

async fn resolve_endpoint(host: &str, port: u16, allow_ipv6: bool) -> Result<SocketAddr, String> {
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?;
    addrs
        .find(|addr| allow_ipv6 || addr.is_ipv4())
        .ok_or_else(|| {
            if allow_ipv6 {
                format!("no address found for {host}:{port}")
            } else {
                format!("no IPv4 address found for {host}:{port}")
            }
        })
}

/// First-frame dispatch for inbound connections.
async fn handle_inbound(
    sock: TcpStream,
    state: SharedState,
    identity: Arc<Identity>,
) -> io::Result<()> {
    let (read_half, mut write_half) = sock.into_split();
    let mut reader = BufReader::new(read_half);
    let first = read_frame(&mut reader).await?;
    match first {
        Frame::Hello { .. } => {
            run_handshake(
                state,
                identity,
                &mut reader,
                &mut write_half,
                /* initiator */ false,
                /* peer_hello_already_read */ Some(first),
                None,
            )
            .await
        }
        Frame::Revoke {
            from_fp,
            to_fp,
            timestamp,
            signature,
        } => {
            if let Err(e) = handle_revoke(&state, &from_fp, &to_fp, timestamp, &signature).await {
                eprintln!("[pair] revoke rejected: {e}");
            }
            Ok(())
        }
        Frame::Decision { .. } => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Decision is not a valid first frame",
        )),
    }
}

async fn drive_handshake(
    sock: TcpStream,
    state: SharedState,
    identity: Arc<Identity>,
    initiator: bool,
    endpoint_hint: Option<(String, u16)>,
) -> io::Result<()> {
    let (read_half, mut write_half) = sock.into_split();
    let mut reader = BufReader::new(read_half);
    run_handshake(
        state,
        identity,
        &mut reader,
        &mut write_half,
        initiator,
        None,
        endpoint_hint,
    )
    .await
}

async fn run_handshake<R, W>(
    state: SharedState,
    identity: Arc<Identity>,
    reader: &mut R,
    write_half: &mut W,
    initiator: bool,
    peer_hello_already_read: Option<Frame>,
    endpoint_hint: Option<(String, u16)>,
) -> io::Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Generate our nonce up front so both Hello frames are deterministic.
    let our_nonce = random_nonce();
    let our_name = hostname_or_fbro();
    let our_os = current_os();
    let our_hello = Frame::Hello {
        v: 1,
        pubkey: b64(&identity.public_bytes()),
        nonce: b64(&our_nonce),
        name: our_name.clone(),
        os: Some(our_os.clone()),
        role: if initiator { "initiator" } else { "responder" }.to_string(),
    };

    // Initiator writes first; responder reads first. After that both have
    // each other's Hello and the order doesn't matter. The dispatch
    // wrapper may have already consumed the peer's Hello for us.
    let peer_hello: Frame = if let Some(frame) = peer_hello_already_read {
        // We're the responder and the dispatcher already read the Hello.
        // Just send ours.
        write_frame(write_half, &our_hello).await?;
        frame
    } else if initiator {
        write_frame(write_half, &our_hello).await?;
        read_frame(reader).await?
    } else {
        let frame = read_frame(reader).await?;
        write_frame(write_half, &our_hello).await?;
        frame
    };

    let Frame::Hello {
        v,
        pubkey: their_pk_b64,
        nonce: their_nonce_b64,
        name: their_name,
        os: their_os,
        ..
    } = peer_hello
    else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello"));
    };
    if v != 1 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unknown pairing protocol version {v}"),
        ));
    }
    let their_pubkey = decode_32(&their_pk_b64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed peer pubkey"))?;
    let their_nonce = decode_16(&their_nonce_b64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed peer nonce"))?;
    let their_fp = fingerprint_of(&their_pubkey);

    let our_pubkey = identity.public_bytes();
    let code = derive_code(&our_pubkey, &our_nonce, &their_pubkey, &their_nonce);
    let handshake_id = uuid::Uuid::new_v4().to_string();
    let started_at = SystemTime::now();
    let expires_at = unix_millis(started_at) + HANDSHAKE_TTL.as_millis() as u64;

    eprintln!("[pair] handshake {handshake_id} with '{their_name}' (fp={their_fp}) code={code}");

    // Register the handshake + open a oneshot for the FE's decision.
    let (decision_tx, decision_rx) = oneshot::channel::<bool>();
    {
        let mut guard = state.lock().await;
        guard
            .active_handshakes
            .insert(handshake_id.clone(), decision_tx);
    }

    // Notify the local FE. (No matching `entityId` since the modal is
    // singleton — only one pending pairing at a time, in v1.)
    emit_to_fe(
        &state,
        ServerMessage::PairingRequested {
            timestamp: now(),
            handshake_id: handshake_id.clone(),
            code: code.clone(),
            name: their_name.clone(),
            os: their_os.clone(),
            pubkey_algo: "ssh-ed25519".to_string(),
            peer_fingerprint: their_fp.clone(),
            expires_at,
        },
    )
    .await;

    // Wait for the user's decision (or timeout). Pull the local decision
    // and write it over the wire concurrently with the peer's decision.
    let local_decision = match timeout(HANDSHAKE_TTL, decision_rx).await {
        Ok(Ok(accept)) => accept,
        Ok(Err(_)) => {
            // Sender dropped — likely a revoke or shutdown.
            eprintln!("[pair] {handshake_id} decision channel closed");
            cleanup_handshake(&state, &handshake_id).await;
            emit_resolved(&state, &handshake_id, "rejected", None).await;
            return Ok(());
        }
        Err(_) => {
            eprintln!("[pair] {handshake_id} timed out waiting for local user");
            cleanup_handshake(&state, &handshake_id).await;
            emit_resolved(&state, &handshake_id, "timeout", None).await;
            return Ok(());
        }
    };
    write_frame(
        write_half,
        &Frame::Decision {
            accept: local_decision,
        },
    )
    .await?;

    // Now read the peer's decision. Bound by the same TTL — if the peer
    // takes longer than 60s we assume they ghosted.
    let remaining = HANDSHAKE_TTL.saturating_sub(
        started_at
            .elapsed()
            .unwrap_or(std::time::Duration::from_secs(0)),
    );
    let peer_frame: io::Result<Frame> = match timeout(remaining, read_frame(reader)).await {
        Ok(r) => r,
        Err(_) => {
            cleanup_handshake(&state, &handshake_id).await;
            emit_resolved(&state, &handshake_id, "timeout", None).await;
            return Ok(());
        }
    };
    let peer_accept = match peer_frame {
        Ok(Frame::Decision { accept }) => accept,
        Ok(_) => {
            cleanup_handshake(&state, &handshake_id).await;
            emit_resolved(&state, &handshake_id, "rejected", None).await;
            return Ok(());
        }
        Err(e) => {
            eprintln!("[pair] {handshake_id} peer decision read failed: {e}");
            cleanup_handshake(&state, &handshake_id).await;
            emit_resolved(&state, &handshake_id, "rejected", None).await;
            return Ok(());
        }
    };

    if local_decision && peer_accept {
        // Persist the peer.
        let endpoint_seen_at = endpoint_hint.as_ref().map(|_| SystemTime::now());
        let peer = PairedPeer {
            fingerprint: their_fp.clone(),
            pubkey: their_pubkey.to_vec(),
            name: their_name,
            os: their_os,
            paired_at: SystemTime::now(),
            last_seen: None,
            last_host: endpoint_hint.as_ref().map(|(host, _)| host.clone()),
            last_sftp_port: endpoint_hint.as_ref().map(|(_, port)| *port),
            last_pair_port: endpoint_hint
                .as_ref()
                .map(|_| crate::remote::pair_transport::PAIR_PORT),
            last_endpoint_seen_at: endpoint_seen_at,
        };
        {
            let mut guard = state.lock().await;
            guard
                .share_config
                .paired_peers
                .insert(their_fp.clone(), peer);
            let snapshot = guard.share_config.paired_peers.clone();
            // Persist outside the lock would be cleaner, but the write is
            // ~tens of µs and the lock is the only one in play here.
            if let Err(e) = save_peers(&default_peers_path(), &snapshot) {
                eprintln!("[pair] failed to write peers.json: {e}");
            }
        }
        cleanup_handshake(&state, &handshake_id).await;
        // Emit patches *before* PairingResolved so the FE's `discoveredPeers`
        // and `shareConfig` reducers run first — by the time the modal
        // closes on PairingResolved, the auto-connect side effect can
        // already see `paired: true` on the discovered entry.
        emit_share_config_patch(&state).await;
        emit_discovered_peers_patch(&state).await;
        emit_resolved(&state, &handshake_id, "paired", Some(their_fp.clone())).await;
        eprintln!("[pair] {handshake_id} paired with {their_fp}");
    } else {
        cleanup_handshake(&state, &handshake_id).await;
        emit_resolved(&state, &handshake_id, "rejected", None).await;
        eprintln!("[pair] {handshake_id} rejected (local={local_decision} peer={peer_accept})");
    }
    Ok(())
}

async fn cleanup_handshake(state: &SharedState, handshake_id: &str) {
    let mut guard = state.lock().await;
    guard.active_handshakes.remove(handshake_id);
}

/// Outbound: send a signed Revoke notice to a peer's pair port. Best-effort;
/// errors are logged but don't fail the local revoke (which has already
/// taken effect by the time this is called).
pub async fn send_revoke(
    identity: Arc<Identity>,
    target_host: String,
    target_port: u16,
    target_fp: String,
) -> Result<(), String> {
    use ed25519_dalek::Signer;
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let payload = revoke_payload(&identity.fingerprint, &target_fp, timestamp);
    let sig = identity.signing.sign(&payload);
    let frame = Frame::Revoke {
        from_fp: identity.fingerprint.clone(),
        to_fp: target_fp.clone(),
        timestamp,
        signature: b64(&sig.to_bytes()),
    };
    eprintln!("[pair] sending revoke to {target_host}:{target_port} (fp={target_fp})");
    let mut sock = timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect((target_host.as_str(), target_port)),
    )
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map_err(|e| format!("connect: {e}"))?;
    write_frame(&mut sock, &frame)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Inbound: someone says they revoked us. If the signature checks out
/// against the pubkey we have for `from_fp`, drop them from our
/// paired_peers + unmount any of their sources + persist + notify the FE.
async fn handle_revoke(
    state: &SharedState,
    from_fp: &str,
    to_fp: &str,
    timestamp: u64,
    signature_b64: &str,
) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    // Anti-replay window. Reject anything significantly older than now;
    // also reject far-future timestamps (clock skew bound).
    let now_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let age = now_ms.saturating_sub(timestamp);
    let future = timestamp.saturating_sub(now_ms);
    if age > REVOKE_MAX_AGE_MS || future > REVOKE_MAX_AGE_MS {
        return Err(format!(
            "stale/future revoke (age={age}ms future={future}ms)"
        ));
    }

    // Look up the alleged sender's pubkey from our paired_peers. If we
    // don't have an entry for from_fp, the revoke is meaningless — we
    // don't trust this peer anyway.
    let (their_pubkey, our_fp) = {
        let g = state.lock().await;
        let pk = g
            .share_config
            .paired_peers
            .get(from_fp)
            .map(|p| p.pubkey.clone())
            .ok_or_else(|| format!("no paired entry for sender {from_fp}"))?;
        let our_fp = g
            .identity
            .as_ref()
            .map(|i| i.fingerprint.clone())
            .unwrap_or_default();
        (pk, our_fp)
    };

    // Make sure they're actually revoking *us*, not some other peer.
    if to_fp != our_fp {
        return Err(format!(
            "revoke target {to_fp} does not match our fp {our_fp}"
        ));
    }

    // Verify the signature.
    let pk_bytes: [u8; 32] = their_pubkey
        .as_slice()
        .try_into()
        .map_err(|_| "stored pubkey is not 32 bytes".to_string())?;
    let verifying = VerifyingKey::from_bytes(&pk_bytes).map_err(|e| e.to_string())?;
    let sig_bytes_vec = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_b64.as_bytes())
        .map_err(|e| format!("malformed signature b64: {e}"))?;
    let sig_bytes: [u8; 64] = sig_bytes_vec
        .as_slice()
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    let sig = Signature::from_bytes(&sig_bytes);
    let payload = revoke_payload(from_fp, to_fp, timestamp);
    verifying
        .verify(&payload, &sig)
        .map_err(|e| format!("signature verify failed: {e}"))?;

    eprintln!("[pair] honoring revoke from {from_fp}");

    // Tear down trust + mounts for this peer, mirror the local revoke
    // intent. Drop sources first (so the FE sees them gone), then update
    // paired_peers + persist.
    let removed_any_source = {
        let mut g = state.lock().await;
        let ids: Vec<String> = g
            .sources
            .iter()
            .filter(|(_, s)| s.peer_id.as_deref() == Some(from_fp))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            g.sources.remove(id);
            g.drivers.remove(id);
        }
        g.share_config.paired_peers.remove(from_fp);
        let snapshot = g.share_config.paired_peers.clone();
        drop(g);
        if let Err(e) = save_peers(&default_peers_path(), &snapshot) {
            eprintln!("[pair] failed to persist peers.json after inbound revoke: {e}");
        }
        !ids.is_empty()
    };

    // Push the standard set of patches so the FE updates the volume bar
    // and Sharing tab immediately.
    if removed_any_source {
        emit_sources_patch(state).await;
    }
    emit_share_config_patch(state).await;
    emit_discovered_peers_patch(state).await;
    Ok(())
}

async fn emit_sources_patch(state: &SharedState) {
    let payload = {
        let g = state.lock().await;
        serde_json::json!({
            "sources": crate::ws::runtime::build_sources_json(&g),
        })
    };
    emit_to_fe(
        state,
        ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: payload,
        },
    )
    .await;
}

async fn emit_resolved(
    state: &SharedState,
    handshake_id: &str,
    outcome: &str,
    fingerprint: Option<String>,
) {
    emit_to_fe(
        state,
        ServerMessage::PairingResolved {
            timestamp: now(),
            handshake_id: handshake_id.to_string(),
            outcome: outcome.to_string(),
            fingerprint,
        },
    )
    .await;
}

async fn emit_discovered_peers_patch(state: &SharedState) {
    let payload = {
        let g = state.lock().await;
        let paired_fps: std::collections::HashSet<&str> = g
            .share_config
            .paired_peers
            .values()
            .map(|p| p.fingerprint.as_str())
            .collect();
        let mut peers: Vec<serde_json::Value> = g
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
        serde_json::json!({ "peers": peers })
    };
    emit_to_fe(
        state,
        ServerMessage::StatePatch {
            timestamp: now(),
            domain: "discoveredPeers".to_string(),
            entity_id: None,
            version: 0,
            patch: payload,
        },
    )
    .await;
}

async fn emit_share_config_patch(state: &SharedState) {
    let payload = {
        let g = state.lock().await;
        crate::ws::runtime::build_share_config(&g)
    };
    emit_to_fe(
        state,
        ServerMessage::StatePatch {
            timestamp: now(),
            domain: "shareConfig".to_string(),
            entity_id: None,
            version: 0,
            patch: payload,
        },
    )
    .await;
}

async fn emit_to_fe(state: &SharedState, msg: ServerMessage) {
    let senders = {
        let g = state.lock().await;
        g.gui_senders.clone()
    };
    for tx in senders {
        let _ = tx.try_send(msg.clone());
    }
}

// ─── wire helpers ─────────────────────────────────────────────────────────────

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let mut line =
        serde_json::to_vec(frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncBufReadExt + Unpin>(r: &mut R) -> io::Result<Frame> {
    let mut line = String::new();
    let n = r.read_line(&mut line).await?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "peer closed before sending frame",
        ));
    }
    serde_json::from_str(line.trim()).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn random_nonce() -> [u8; 16] {
    use rand::RngCore;
    let mut n = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_32(s: &str) -> Option<[u8; 32]> {
    let v = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()?;
    v.try_into().ok()
}

fn decode_16(s: &str) -> Option<[u8; 16]> {
    let v = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s.as_bytes())
        .ok()?;
    v.try_into().ok()
}

fn unix_millis(t: SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hostname_or_fbro() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut _, buf.len()) };
        if rc == 0 {
            let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr() as *const _) }
                .to_string_lossy()
                .into_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "fbro".to_string()
}

fn current_os() -> String {
    if cfg!(target_os = "macos") {
        "macos".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else if cfg!(target_os = "windows") {
        "windows".to_string()
    } else {
        "unknown".to_string()
    }
}

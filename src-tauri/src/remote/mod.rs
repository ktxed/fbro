//! Remote-fbro: LAN-discovery, pairing, and SSH-tunnelled SFTP between
//! fbro instances. See `spex/plans/remote-fbro.md` for the design.
//!
//! Module layout:
//! - `keys`     — Ed25519 identity at `~/.config/fbro/id_ed25519`,
//!                fingerprint helpers.
//! - `discovery` (TBD) — mDNS browse/advertise of `_fbro-remote._tcp.local`.
//! - `pairing`  (TBD) — 6-digit pairing handshake, `peers.json` I/O.
//! - `server`   (TBD) — embedded `russh` listener + SFTP subsystem.

pub mod discovery;
pub mod keys;
pub mod pair_transport;
pub mod pairing;
pub mod server;
pub mod share_state;

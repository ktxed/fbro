import { useEffect, useRef, useState } from "react";
import { useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import type { PendingPairing } from "../ws/types";

interface PairingConfirmationProps {
  client: WsClient;
  pairing: PendingPairing;
}

/** Security-sensitive pairing decision rendered next to the peer it applies to. */
export function PairingConfirmation({ client, pairing }: PairingConfirmationProps) {
  const dispatch = useAppDispatch();
  const confirmRef = useRef<HTMLButtonElement>(null);
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(id);
  }, [pairing.handshakeId]);

  useEffect(() => {
    if (pairing.localDecision === null) confirmRef.current?.focus({ preventScroll: true });
  }, [pairing.handshakeId, pairing.localDecision]);

  const respond = (accept: boolean) => {
    if (pairing.localDecision !== null) return;
    client.sendIntent("ConfirmPairing", undefined, {
      handshakeId: pairing.handshakeId,
      accept,
    });
    dispatch({ type: "SET_PAIRING_DECISION", handshakeId: pairing.handshakeId, accept });
  };

  const secondsLeft = Math.max(0, Math.ceil((pairing.expiresAt - now) / 1000));
  const submittedMessage = pairing.localDecision === true
    ? "Confirmed here — waiting for the other fbro…"
    : "Pairing cancelled…";
  const headingId = `pairing-confirmation-${pairing.handshakeId}`;

  return (
    <section
      className="pairing-confirmation"
      data-testid="pairing-confirmation"
      data-peer-fingerprint={pairing.peerFingerprint}
      role="group"
      aria-labelledby={headingId}
    >
      <div className="pairing-confirmation__copy">
        <strong id={headingId}>Confirm pairing with {pairing.name}</strong>
        <span>
          Compare this code with the one shown on {pairing.name}
          {pairing.os && <> ({pairing.os})</>}.
        </span>
        <span className="pairing-confirmation__warning">
          If the codes do not match, cancel — the connection may be intercepted.
        </span>
      </div>
      <div className="pairing-confirmation__code" aria-label={`Pairing code ${pairing.code}`}>
        {pairing.code.slice(0, 3)} {pairing.code.slice(3)}
      </div>
      <div className="pairing-confirmation__actions">
        {pairing.localDecision === null ? (
          <>
            <span className="pairing-confirmation__timer">{secondsLeft}s</span>
            <button
              className="sharing-button"
              data-command-id="pairing.cancel"
              onClick={() => respond(false)}
            >
              Cancel
            </button>
            <button
              ref={confirmRef}
              className="sharing-button sharing-button--primary"
              data-command-id="pairing.confirm"
              onClick={() => respond(true)}
              autoFocus
            >
              Confirm
            </button>
          </>
        ) : (
          <span className="pairing-confirmation__status" role="status">{submittedMessage}</span>
        )}
      </div>
    </section>
  );
}

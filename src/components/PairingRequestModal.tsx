import { useEffect, useState } from "react";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";

interface PairingRequestModalProps {
  client: WsClient;
}

/**
 * Modal shown while a pairing handshake is in progress. Surfaces the
 * 6-digit code and the remote machine's self-reported name; the user
 * compares the code with what's on the other device and clicks Confirm
 * (or Cancel). Auto-dismisses on `PairingResolved` from the BE.
 */
export function PairingRequestModal({ client }: PairingRequestModalProps) {
  const state = useAppState();
  const pp = state.pendingPairing;
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!pp) return;
    const id = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(id);
  }, [pp?.handshakeId]);

  if (!pp) return null;

  const secondsLeft = Math.max(0, Math.ceil((pp.expiresAt - now) / 1000));

  const respond = (accept: boolean) => {
    client.sendIntent("ConfirmPairing", undefined, {
      handshakeId: pp.handshakeId,
      accept,
    });
  };

  return (
    <div className="modal-overlay" role="dialog" aria-modal="true">
      <div className="modal modal--pairing">
        <div className="modal__header">Pair with {pp.name}</div>
        <div className="modal__body">
          <p>
            Confirm this 6-digit code matches what's shown on{" "}
            <strong>{pp.name}</strong>
            {pp.os && <> ({pp.os})</>}:
          </p>
          <div className="pairing-code" aria-label={`Pairing code ${pp.code}`}>
            {pp.code.slice(0, 3)} {pp.code.slice(3)}
          </div>
          <p className="settings-hint">
            If the codes don't match, cancel — someone may be intercepting
            the connection. {secondsLeft}s remaining.
          </p>
        </div>
        <div className="modal__footer">
          <button className="settings-button" onClick={() => respond(false)}>
            Cancel
          </button>
          <button
            className="settings-button settings-button--primary"
            onClick={() => respond(true)}
            autoFocus
          >
            Confirm
          </button>
        </div>
      </div>
    </div>
  );
}

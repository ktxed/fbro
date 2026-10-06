/** Decode a hex string (as sent by FileChunk) back to bytes. */
export function hexDecode(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** Returns true if the byte array appears to be binary (not plain text). */
export function detectBinary(bytes: Uint8Array): boolean {
  const sample = bytes.slice(0, 4096);
  let nonText = 0;
  for (const byte of sample) {
    if (byte === 0) return true;
    if (byte < 9 || (byte > 13 && byte < 32) || byte === 127) nonText++;
  }
  return sample.length > 0 && nonText / sample.length > 0.3;
}

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
  for (let i = 0; i < sample.length; i++) {
    const b = sample[i];
    if (b === 0) return true;
    if (b < 8 || (b > 13 && b < 32)) return true;
  }
  return false;
}

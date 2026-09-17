/**
 * Checking a receipt's device signature in this window, with WebCrypto's
 * Ed25519 and nothing else: no network, no daemon.
 *
 * A receipt does not carry the key that signed it, so a check needs one from
 * the reader. Without a key the answer is "not checked", never "verified",
 * which is the opposite of what vitna-receipt-verify prints today (it reports
 * the integrity check as PASSED when no key is given).
 *
 * One known difference from the Rust side: ed25519-dalek's verify_strict also
 * refuses small-order public keys. WebCrypto implementations are not required
 * to, so this check is at most as strict as RFC 8032 verification.
 */

import { canonicalBytes, type Receipt } from './receipt';

export type SignatureCheck =
  | { status: 'valid' }
  | { status: 'invalid' }
  | { status: 'not_checked' }
  | { status: 'no_signature' }
  | { status: 'placeholder' }
  | { status: 'bad_key'; detail: string }
  | { status: 'bad_signature'; detail: string }
  | { status: 'unsupported'; detail: string };

export function hexToBytes(hex: string): Uint8Array | null {
  const clean = hex.trim().replace(/^0x/i, '');
  if (clean.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(clean)) return null;
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i += 1) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** An empty or all-zero signature is a placeholder, whatever it claims to sign. */
export function signatureShape(signatureHex: string): 'empty' | 'placeholder' | 'malformed' | 'ed25519' {
  if (signatureHex.trim() === '') return 'empty';
  const bytes = hexToBytes(signatureHex);
  if (!bytes || bytes.length !== 64) return 'malformed';
  return bytes.every((b) => b === 0) ? 'placeholder' : 'ed25519';
}

export async function checkDeviceSignature(receipt: Receipt, publicKeyHex: string): Promise<SignatureCheck> {
  const shape = signatureShape(receipt.device_signature);
  if (shape === 'empty') return { status: 'no_signature' };
  if (shape === 'placeholder') return { status: 'placeholder' };
  if (shape === 'malformed') {
    return { status: 'bad_signature', detail: 'an Ed25519 signature is 64 bytes, written as 128 hex characters' };
  }
  if (publicKeyHex.trim() === '') return { status: 'not_checked' };

  const key = hexToBytes(publicKeyHex);
  if (!key || key.length !== 32) {
    return { status: 'bad_key', detail: 'an Ed25519 public key is 32 bytes, written as 64 hex characters' };
  }
  const signature = hexToBytes(receipt.device_signature) as Uint8Array;

  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return { status: 'unsupported', detail: 'this window has no WebCrypto' };

  let imported: CryptoKey;
  try {
    imported = await subtle.importKey('raw', key, { name: 'Ed25519' }, false, ['verify']);
  } catch (error) {
    const name = error instanceof DOMException ? error.name : '';
    if (name === 'NotSupportedError') {
      return { status: 'unsupported', detail: 'this browser has no Ed25519 in WebCrypto' };
    }
    return { status: 'bad_key', detail: error instanceof Error ? error.message : 'the key was refused' };
  }

  const ok = await subtle.verify({ name: 'Ed25519' }, imported, signature, canonicalBytes(receipt));
  return { status: ok ? 'valid' : 'invalid' };
}

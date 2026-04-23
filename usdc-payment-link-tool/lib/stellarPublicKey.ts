import { StrKey } from 'stellar-sdk';

/** True when `key` is a checksum-valid Stellar Ed25519 account id (`G...`), suitable for settlement destinations. */
export function isValidSettlementPublicKey(key: string | null | undefined): boolean {
  if (key == null) return false;
  const trimmed = key.trim();
  if (!trimmed) return false;
  return StrKey.isValidEd25519PublicKey(trimmed);
}

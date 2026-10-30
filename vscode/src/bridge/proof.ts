// Preuves d'appairage du pont (HMAC-SHA256 du secret partagé sur les deux
// nonces). Contrat : .tools/research/87-lot1a-contract.md.
import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";

const CLIENT = Buffer.from("chatonlm-bridge/1 client");
const SERVER = Buffer.from("chatonlm-bridge/1 server");

function hmac(secret: Buffer, ...parts: Buffer[]): Buffer {
  const mac = createHmac("sha256", secret);
  for (const part of parts) mac.update(part);
  return mac.digest();
}

/** Nonce de l'éditeur, 32 octets. */
export function newNonce(): Buffer {
  return randomBytes(32);
}

/** Preuve de l'éditeur : HMAC(secret, « … client » ‖ serverNonce ‖ clientNonce). */
export function clientProof(
  secret: Buffer,
  serverNonce: Buffer,
  clientNonce: Buffer,
): Buffer {
  return hmac(secret, CLIENT, serverNonce, clientNonce);
}

/** Preuve attendue de l'app : HMAC(secret, « … server » ‖ clientNonce ‖ serverNonce). */
export function serverProof(
  secret: Buffer,
  clientNonce: Buffer,
  serverNonce: Buffer,
): Buffer {
  return hmac(secret, SERVER, clientNonce, serverNonce);
}

/** Preuve de l'app vérifiée en temps constant. */
export function verifyServerProof(
  secret: Buffer,
  clientNonce: Buffer,
  serverNonce: Buffer,
  proofHex: unknown,
): boolean {
  if (typeof proofHex !== "string" || !/^[0-9a-f]{64}$/i.test(proofHex)) {
    return false;
  }
  return timingSafeEqual(
    Buffer.from(proofHex, "hex"),
    serverProof(secret, clientNonce, serverNonce),
  );
}

/** Octets d'une chaîne hexadécimale, ou null. */
export function fromHex(value: unknown): Buffer | null {
  if (typeof value !== "string" || !/^([0-9a-f]{2})+$/i.test(value)) {
    return null;
  }
  return Buffer.from(value, "hex");
}

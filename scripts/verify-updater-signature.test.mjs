import {
  generateKeyPairSync,
  createHash,
  randomBytes,
  sign,
} from "node:crypto";
import assert from "node:assert/strict";
import test from "node:test";
import { verifyUpdaterSignature } from "./verify-updater-signature.mjs";

function sample() {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const keyId = randomBytes(8);
  const key = Buffer.concat([
    Buffer.from("Ed"),
    keyId,
    publicKey.export({ type: "spki", format: "der" }).subarray(-32),
  ]);
  const data = Buffer.from("Deployment signature contract sample");
  const signature = sign(
    null,
    createHash("blake2b512").update(data).digest(),
    privateKey,
  );
  const packet = Buffer.concat([Buffer.from("ED"), keyId, signature]);
  const comment = "timestamp:1\tfile:sample.bin\thashed";
  const globalSignature = sign(
    null,
    Buffer.concat([signature, Buffer.from(comment)]),
    privateKey,
  );
  const pub = `untrusted comment: test public key\n${key.toString("base64")}\n`;
  const sig = `untrusted comment: signature\n${packet.toString("base64")}\ntrusted comment: ${comment}\n${globalSignature.toString("base64")}\n`;
  return { data, pub, sig };
}

test("raw minisign and Tauri wrapped signatures validate both signatures", () => {
  const s = sample();
  const result = verifyUpdaterSignature(s.data, s.pub, s.sig);
  assert.match(result.publicKeySha256, /^[0-9a-f]{64}$/);
  assert.deepEqual(
    verifyUpdaterSignature(
      s.data,
      Buffer.from(s.pub).toString("base64"),
      Buffer.from(s.sig).toString("base64"),
    ),
    result,
  );
});

test("unrelated public key and absent signature are rejected", () => {
  const s = sample();
  assert.throws(() => verifyUpdaterSignature(s.data, sample().pub, s.sig));
  assert.throws(() => verifyUpdaterSignature(s.data, s.pub, ""));
});

test("an unauthenticated trusted comment cannot be accepted", () => {
  const s = sample();
  assert.throws(() =>
    verifyUpdaterSignature(
      s.data,
      s.pub,
      s.sig.replace("timestamp:1", "timestamp:2"),
    ),
  );
});

import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

function lines(value) {
  let text = value.trim();
  if (!text.startsWith("untrusted comment:")) {
    text = Buffer.from(text, "base64").toString("utf8").trim();
  }
  return text.split(/\r?\n/);
}

export function verifyUpdaterSignature(file, publicKey, signature) {
  const keyLines = lines(publicKey);
  const sigLines = lines(signature);
  const key = Buffer.from(keyLines[1] ?? "", "base64");
  const sig = Buffer.from(sigLines[1] ?? "", "base64");
  if (
    key.length !== 42 ||
    key.subarray(0, 2).toString() !== "Ed" ||
    sig.length !== 74 ||
    sig.subarray(0, 2).toString() !== "ED" ||
    !key.subarray(2, 10).equals(sig.subarray(2, 10)) ||
    !sigLines[2]?.startsWith("trusted comment: ")
  ) {
    throw new Error("Invalid updater public key/signature format or key ID");
  }
  const spki = createPublicKey({
    key: Buffer.concat([
      Buffer.from("302a300506032b6570032100", "hex"),
      key.subarray(10),
    ]),
    format: "der",
    type: "spki",
  });
  const digest = createHash("blake2b512").update(file).digest();
  const signedComment = Buffer.concat([
    sig.subarray(10),
    Buffer.from(sigLines[2].slice("trusted comment: ".length), "utf8"),
  ]);
  if (
    !verify(null, digest, spki, sig.subarray(10)) ||
    !verify(null, signedComment, spki, Buffer.from(sigLines[3] ?? "", "base64"))
  ) {
    throw new Error("Updater signature verification failed");
  }
  return { publicKeySha256: createHash("sha256").update(key).digest("hex") };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    const [file, publicKey, signature = `${file}.sig`] = process.argv.slice(2);
    if (!file || !publicKey)
      throw new Error("Expected file, public key and signature paths");
    const result = verifyUpdaterSignature(
      readFileSync(file),
      readFileSync(publicKey, "utf8"),
      readFileSync(signature, "utf8"),
    );
    console.log(JSON.stringify(result));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

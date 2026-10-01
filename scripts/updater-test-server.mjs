// 로컬 시험 전용. 운영 키를 읽거나 설치 프로그램을 만들지 않는다.
import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { createServer } from "node:http";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const root = path.resolve(process.argv[2]);
const port = Number(process.argv[3] ?? 18784);
mkdirSync(root, { recursive: true });
const data = Buffer.concat([Buffer.from("MZ updater boundary sample; this is NOT an executable.\n"), randomBytes(8 * 1024 * 1024)]);
function key() {
  const pair = generateKeyPairSync("ed25519");
  const id = randomBytes(8);
  const packet = Buffer.concat([Buffer.from("Ed"), id, pair.publicKey.export({ type: "spki", format: "der" }).subarray(-32)]);
  return { ...pair, id, encoded: Buffer.from(`untrusted comment: owned ephemeral test key\n${packet.toString("base64")}\n`).toString("base64") };
}
function signature(owner, version) {
  const value = sign(null, createHash("blake2b512").update(data).digest(), owner.privateKey);
  const comment = `timestamp:1\tfile:sample.exe\tversion:${version}`;
  const global = sign(null, Buffer.concat([value, Buffer.from(comment)]), owner.privateKey);
  return Buffer.from(`untrusted comment: owned test\n${Buffer.concat([Buffer.from("ED"), owner.id, value]).toString("base64")}\ntrusted comment: ${comment}\n${global.toString("base64")}\n`).toString("base64");
}
const owner = key(), wrong = key();
writeFileSync(path.join(root, "public.key.pub"), owner.encoded);
const signatures = { normal: signature(owner, "0.3.0"), wrong_key: signature(wrong, "0.3.0"), version_mismatch: signature(owner, "0.2.0") };
writeFileSync(path.join(root, "sample.exe"), data);
writeFileSync(path.join(root, "sample.exe.sig"), signatures.normal);
writeFileSync(path.join(root, "fixture-control.json"), JSON.stringify({ scenario: "normal", slow: false }));
const counters = { checks: 0, downloads: 0 };
const server = createServer((req, res) => {
  let control;
  try { control = JSON.parse(readFileSync(path.join(root, "fixture-control.json"), "utf8")); }
  catch { res.writeHead(500).end(); return; }
  const scenario = control.scenario;
  if (req.url === "/updates/test.json") {
    counters.checks++;
    writeFileSync(path.join(root, "counters.json"), JSON.stringify(counters));
    if (scenario === "network") { res.writeHead(503).end(); return; }
    if (scenario === "timeout") { setTimeout(() => res.writeHead(503).end(), 12000); return; }
    const notes = "한글 릴리즈 노트 — 새 버전에서는 업데이트를 확인하고, 동의 후 다운로드하며, 저장·보관을 마친 뒤 설치합니다.\n\n".repeat(2) + "시험 업데이트: 실제 설치와 자동 재실행은 생략합니다.";
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ version: "0.3.0", repository: "skais5384-jpg/dreamrugi-worldbuild-tool", channel: "test", prerelease: true, notes, platforms: { "windows-x86_64": { url: `http://127.0.0.1:${port}/skais5384-jpg/dreamrugi-worldbuild-tool/releases/download/v0.3.0/Dreamrugi.Worldbuild.Tool_0.3.0_x64-setup.exe`, signature: signatures[scenario] ?? signatures.normal } } }));
    return;
  }
  if (req.url?.endsWith("_x64-setup.exe")) {
    counters.downloads++;
    writeFileSync(path.join(root, "counters.json"), JSON.stringify(counters));
    let body = data;
    if (scenario === "tampered") { body = Buffer.from(data); body[100] ^= 1; }
    res.writeHead(200, { "content-type": "application/octet-stream", "content-length": body.length });
    if (scenario === "partial") { res.write(body.subarray(0, 2048)); setTimeout(() => res.destroy(), 30); return; }
    if (!control.slow) { res.end(body); return; }
    let offset = 0;
    const timer = setInterval(() => {
      if (offset >= body.length) { clearInterval(timer); res.end(); return; }
      res.write(body.subarray(offset, offset + 65536)); offset += 65536;
    }, 75);
    res.on("close", () => clearInterval(timer));
    return;
  }
  res.writeHead(404).end();
});
server.listen(port, "127.0.0.1", () => {
  writeFileSync(path.join(root, "ready.json"), JSON.stringify({ endpoint: `http://127.0.0.1:${port}/updates/test.json`, publicKeyFile: "public.key.pub", privateKeyWritten: false, dataSha256: createHash("sha256").update(data).digest("hex") }));
  process.stdout.write(`Owned updater fixture ready on loopback port ${port}.\n`);
});

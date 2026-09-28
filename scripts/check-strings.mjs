// 컴파일 전에 원 JSON과 typed 인자 계약을 검증한다. 문구 본문을 실패 출력에 덤프하지 않는다.
import fs from "node:fs";
import path from "node:path";
import Module from "node:module";
import ts from "typescript";
import { fileURLToPath } from "node:url";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), ".."),
  filename = path.join(root, "src/strings/index.ts");
try {
  const module = new Module(filename);
  module.filename = filename;
  module.paths = Module._nodeModulePaths(path.dirname(filename));
  module._compile(
    ts.transpileModule(fs.readFileSync(filename, "utf8"), {
      compilerOptions: {
        module: ts.ModuleKind.CommonJS,
        target: ts.ScriptTarget.ES2022,
        esModuleInterop: true,
      },
    }).outputText,
    filename,
  );
  const bytes = fs.readFileSync(
    process.argv[2] ?? path.join(root, "src/strings/ko.json"),
  );
  const resource = JSON.parse(
    new TextDecoder("utf-8", { fatal: true }).decode(bytes),
  );
  module.exports.validateResources(resource);
  console.log("String resources and named arguments validated.");
} catch {
  console.error(
    "String resource validation failed. Check UTF-8 JSON, keys, value types and named arguments.",
  );
  process.exitCode = 1;
}

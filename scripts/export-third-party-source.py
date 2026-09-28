"""Archive the exact installed npm runtime packages and locked Cargo sources.

Run after ``npm ci`` and ``cargo metadata --locked --offline`` succeed. This is
the companion to export-corresponding-source.py for a local binary candidate;
review notices and binary components before a public release.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo


ROOT = Path(__file__).resolve().parent.parent
SKIP_DIRS = {".git", "node_modules", "target", "__pycache__"}


def command(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8")


def source_files(prefix: str, package_dir: Path):
    if not package_dir.is_dir() or package_dir.is_symlink():
        raise SystemExit(f"Missing or redirected dependency source: {prefix}")
    for path in sorted(package_dir.rglob("*")):
        if SKIP_DIRS.intersection(path.relative_to(package_dir).parts):
            continue
        if path.is_symlink():
            raise SystemExit(f"Symlink in dependency source: {prefix}/{path.name}")
        if path.is_file():
            relative = path.relative_to(package_dir).as_posix()
            yield f"{prefix}/{relative}", path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()

    cargo = json.loads(command("cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--manifest-path", "src-tauri/Cargo.toml"))
    rust_packages = sorted(
        (package for package in cargo["packages"] if package.get("source")),
        key=lambda package: (package["name"], package["version"], package["source"]),
    )
    npm_root = (ROOT / "node_modules").resolve()
    npm_paths = sorted(set(command("npm.cmd", "ls", "--omit=dev", "--parseable", "--all").splitlines()))
    packages: list[tuple[str, Path]] = []
    for package in rust_packages:
        fingerprint = hashlib.sha256(package["source"].encode()).hexdigest()[:8]
        prefix = f"cargo/{package['name']}-{package['version']}-{fingerprint}"
        packages.append((prefix, Path(package["manifest_path"]).parent))
    for name in npm_paths:
        path = Path(name).resolve()
        if path == ROOT or path == npm_root:
            continue
        if not path.is_relative_to(npm_root) or not (path / "package.json").is_file():
            raise SystemExit(f"Unexpected npm package path: {name}")
        packages.append((f"npm/{path.relative_to(npm_root).as_posix()}", path))

    output.parent.mkdir(parents=True, exist_ok=True)
    manifest_files: list[dict[str, object]] = []
    seen: set[str] = set()
    with ZipFile(output, "w", compression=ZIP_DEFLATED, compresslevel=9, allowZip64=True) as archive:
        for prefix, directory in packages:
            for name, path in source_files(prefix, directory):
                if name in seen:
                    raise SystemExit(f"Duplicate dependency source path: {name}")
                seen.add(name)
                info = ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                digest = hashlib.sha256()
                byte_count = 0
                with path.open("rb") as source, archive.open(info, "w", force_zip64=True) as destination:
                    while chunk := source.read(1024 * 1024):
                        destination.write(chunk)
                        digest.update(chunk)
                        byte_count += len(chunk)
                manifest_files.append({"path": name, "bytes": byte_count, "sha256": digest.hexdigest()})

    manifest = {
        "package_lock_sha256": hashlib.sha256((ROOT / "package-lock.json").read_bytes()).hexdigest(),
        "cargo_lock_sha256": hashlib.sha256((ROOT / "src-tauri/Cargo.lock").read_bytes()).hexdigest(),
        "base_head": (
            json.loads((ROOT / "SOURCE.json").read_text(encoding="utf-8"))["base_head"]
            if (ROOT / "SOURCE.json").is_file()
            else command("git", "rev-parse", "HEAD").strip()
        ),
        "cargo_packages": len(rust_packages),
        "npm_packages": len(packages) - len(rust_packages),
        "zip_sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
        "files": manifest_files,
    }
    output.with_suffix(".manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(f"{output}: {len(packages)} packages, {len(manifest_files)} files, SHA-256 {manifest['zip_sha256']}")


if __name__ == "__main__":
    main()

"""Export a fixed commit plus explicitly fingerprinted overlays for local review.

The complete review archive includes development documents. Public assets use
release-candidate.py snapshot and its narrower public-source-policy instead.
Unrelated working-tree changes and untracked files are never collected implicitly.
"""
from __future__ import annotations
import argparse
import importlib.util
import json
from pathlib import Path

SCRIPT = Path(__file__).with_name('release-candidate.py')
spec = importlib.util.spec_from_file_location('release_candidate', SCRIPT)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--ref', required=True, help='Exact reviewed commit SHA')
    parser.add_argument('--overlay', type=Path, help='Explicit paths, byte counts and SHA-256')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() or output.with_suffix('.manifest.json').exists():
        raise SystemExit('Review archive output already exists; no overwrite')
    tree, files, overlays = release.fixed_files(args.ref, args.overlay)
    for name, data in files.items():
        release.safe_path(name)
        release.check_bytes(data)
    output.parent.mkdir(parents=True, exist_ok=True)
    release.write_zip(output, files)
    manifest = {'base_head': args.ref, 'base_tree': tree, 'overlay': overlays,
                'zip_sha256': release.digest(output.read_bytes()),
                'files': [{'path': n, 'bytes': len(d), 'sha256': release.digest(d)} for n, d in sorted(files.items())]}
    release.verify_archive(output, manifest['files'])
    output.with_suffix('.manifest.json').write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'files': len(files), 'zip_sha256': manifest['zip_sha256']}))


if __name__ == '__main__':
    main()

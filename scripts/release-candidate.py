"""Fixed-source Windows prerelease preparation. Publishing is explicit and draft only."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED

ROOT = Path(__file__).resolve().parent.parent
IDENTITY = 'com.dreamrugi.worldbuildtool'
DENIED = {'.git', 'node_modules', 'target', 'logs', 'secrets', '__pycache__', 'dist'}


def run(*args, cwd=ROOT):
    return subprocess.check_output(args, cwd=cwd)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def load(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def safe_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or '\\' in name or ':' in name:
        raise ValueError('Unsafe path')
    if DENIED.intersection(path.parts) or any(p.startswith('.env') for p in path.parts):
        raise ValueError('Forbidden source path')
    if path.suffix.lower() in {'.key', '.pfx', '.p12', '.pem', '.dpapi', '.pyc'}:
        raise ValueError('Secret or cache file in source')
    return path


def check_bytes(data):
    if re.search(rb'(?m)^-----BEGIN [A-Z ]*PRIVATE KEY-----', data):
        raise ValueError('Private key in source')
    if re.search(rb'\b(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{50,})', data):
        raise ValueError('Credential in source')
    if len(data) < 4096:
        try:
            decoded = base64.b64decode(data.strip(), validate=True)
            if decoded.startswith(b'untrusted comment:') and b'secret key' in decoded.splitlines()[0]:
                raise ValueError('Encoded private key in source')
        except (base64.binascii.Error, UnicodeError):
            pass


def metadata(files, version):
    npm = json.loads(files['package.json'])
    lock = json.loads(files['package-lock.json'])
    tauri = json.loads(files['src-tauri/tauri.conf.json'])
    cargo = files['src-tauri/Cargo.toml'].decode('utf-8')
    cargo_version = re.search(r'^version = "([^"]+)"', cargo, re.M).group(1)
    if not re.fullmatch(r'0\.(0|[1-9]\d*)\.(0|[1-9]\d*)', version):
        raise ValueError('Only numeric 0.x prerelease versions are supported')
    if any(v != version for v in (npm['version'], lock['version'], lock['packages']['']['version'], tauri['version'], cargo_version)):
        raise ValueError('Version metadata mismatch')
    if npm['license'] != 'GPL-3.0-only' or tauri['bundle']['license'] != npm['license'] or 'license = "GPL-3.0-only"' not in cargo:
        raise ValueError('License metadata mismatch')
    if tauri['identifier'] != IDENTITY:
        raise ValueError('Test or unexpected release identity')
    required = {'LICENSE', 'scripts/package-windows.ps1', 'scripts/check-strings.mjs', 'package-lock.json', 'src-tauri/Cargo.lock'}
    required.update('src-tauri/' + name for name in tauri['bundle']['resources'])
    # Normalize resource paths containing ../ without accepting external paths.
    import posixpath
    required = {posixpath.normpath(p) for p in required}
    required.update('src-tauri/' + p for p in tauri['bundle']['icon'])
    if required - set(files):
        raise ValueError('Missing build/resources: ' + ', '.join(sorted(required - set(files))))
    return tauri


def write_zip(path, files):
    with ZipFile(path, 'w', ZIP_DEFLATED, compresslevel=9) as z:
        for name, data in sorted(files.items()):
            info = ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.external_attr = 0o644 << 16
            info.compress_type = ZIP_DEFLATED
            z.writestr(info, data)


def fixed_files(ref, overlay=None):
    if not re.fullmatch(r'[0-9a-f]{40}', ref):
        raise ValueError('Source must be an exact commit SHA')
    tree = run('git', 'rev-parse', ref + '^{tree}').decode().strip()
    raw = run('git', 'ls-tree', '-r', '-z', ref)
    files = {}
    for entry in raw.split(b'\0'):
        if entry:
            props, name = entry.split(b'\t', 1)
            mode, kind, oid = props.split()
            if mode not in {b'100644', b'100755'} or kind != b'blob':
                raise ValueError('Source symlink/submodule not supported')
            files[name.decode()] = run('git', 'cat-file', 'blob', oid.decode())
    changes = []
    seen_changes = set()
    if overlay:
        spec = load(overlay)
        if spec['base_head'] != ref:
            raise ValueError('Overlay base mismatch')
        for item in spec['files']:
            name = item['path']
            safe_path(name)
            if name in seen_changes:
                raise ValueError('Duplicate overlay path')
            seen_changes.add(name)
            source = ROOT / name
            if source.is_symlink() or not source.resolve().is_relative_to(ROOT):
                raise ValueError('Redirected overlay')
            data = source.read_bytes()
            if len(data) != item['bytes'] or digest(data) != item['sha256']:
                raise ValueError('Overlay fingerprint mismatch')
            files[name] = data
            changes.append(item)
    return tree, files, changes


def snapshot(ref, version, output, overlay=None):
    tree, files, changes = fixed_files(ref, overlay)
    policy = json.loads(files['packaging/public-source-policy.json'])
    public = {}
    for name, data in files.items():
        if name in policy['files'] or any(name.startswith(p) for p in policy['directories']):
            safe_path(name)
            check_bytes(data)
            public[name] = data
    metadata(public, version)
    file_manifest = [{'path': p, 'bytes': len(d), 'sha256': digest(d)} for p, d in sorted(public.items())]
    descriptor = {'base_head': ref, 'base_tree': tree, 'version': version, 'identifier': IDENTITY,
                  'overlay': [c for c in changes if c['path'] in public], 'files': file_manifest}
    descriptor['candidate_id'] = digest(json.dumps(descriptor, sort_keys=True).encode())
    descriptor['committed_source'] = not descriptor['overlay']
    output = Path(output).resolve()
    zip_path = output.parent / (output.name + '-source.zip')
    if output.exists() or zip_path.exists():
        raise ValueError('Snapshot destination must not exist')
    output.mkdir(parents=True)
    for name, data in public.items():
        dest = output / name
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(data)
    source_info = json.dumps(descriptor, ensure_ascii=False, indent=2).encode()
    (output / 'SOURCE.json').write_bytes(source_info)
    write_zip(zip_path, public | {'SOURCE.json': source_info})
    return descriptor


def verify_archive(path, file_manifest, extra=None, dependency=False):
    expected = {f['path']: f for f in file_manifest}
    if len(expected) != len(file_manifest):
        raise ValueError('Duplicate source manifest path')
    with ZipFile(path) as archive:
        names = archive.namelist()
        if len(names) != len(set(names)) or set(names) != set(expected) | set(extra or {}):
            raise ValueError('Source archive inventory mismatch')
        for name, item in expected.items():
            if dependency:
                p = PurePosixPath(name)
                if p.is_absolute() or '..' in p.parts or '\\' in name or ':' in name or p.parts[0] not in {'cargo', 'npm'}:
                    raise ValueError('Unsafe dependency source path')
            else:
                safe_path(name)
            data = archive.read(name)
            if len(data) != item['bytes'] or digest(data) != item['sha256']:
                raise ValueError('Source archive fingerprint mismatch')
        for name, data in (extra or {}).items():
            if archive.read(name) != data:
                raise ValueError('Source provenance mismatch')


def verify_snapshot(folder):
    folder = Path(folder).resolve()
    info = load(folder / 'SOURCE.json')
    descriptor = {k: info[k] for k in ['base_head', 'base_tree', 'version', 'identifier', 'overlay', 'files']}
    if info['candidate_id'] != digest(json.dumps(descriptor, sort_keys=True).encode()) or info['committed_source'] is not (not info['overlay']):
        raise ValueError('Invalid source candidate descriptor')
    files = {}
    for item in info['files']:
        name = item['path']
        safe_path(name)
        path = folder / name
        if path.is_symlink() or not path.resolve().is_relative_to(folder):
            raise ValueError('Redirected snapshot source')
        data = path.read_bytes()
        if len(data) != item['bytes'] or digest(data) != item['sha256']:
            raise ValueError('Snapshot source changed: ' + name)
        files[name] = data
    metadata(files, info['version'])
    return info


def bundle(folder, public_key, receipt, expected_version, technical=False):
    folder = Path(folder).resolve()
    provenance = load(folder / 'SOURCE.json')
    descriptor = {k: provenance[k] for k in ['base_head', 'base_tree', 'version', 'identifier', 'overlay', 'files']}
    if provenance['candidate_id'] != digest(json.dumps(descriptor, sort_keys=True).encode()):
        raise ValueError('Source candidate descriptor mismatch')
    if provenance['version'] != expected_version or provenance['identifier'] != IDENTITY:
        raise ValueError('Source/version mismatch')
    package = load(folder / 'package-metadata.json')
    if package['version'] != expected_version or package['identifier'] != IDENTITY or package['mode'] != 'Release':
        raise ValueError('Unexpected or Test package')
    if package.get('candidateId') != provenance['candidate_id']:
        raise ValueError('Old artifact belongs to another source candidate')
    if package.get('technicalOnly') is not technical:
        raise ValueError('Technical/signing mode mismatch')
    verify_archive(folder / 'corresponding-source.zip', provenance['files'],
                   {'SOURCE.json': (folder / 'SOURCE.json').read_bytes()})
    dependency_manifest = load(folder / 'third-party-source.manifest.json')
    source_hashes = {f['path']: f['sha256'] for f in provenance['files']}
    if dependency_manifest['package_lock_sha256'] != source_hashes['package-lock.json'] or dependency_manifest['cargo_lock_sha256'] != source_hashes['src-tauri/Cargo.lock']:
        raise ValueError('Dependency lockfile mismatch')
    if dependency_manifest['base_head'] != provenance['base_head'] or dependency_manifest['zip_sha256'] != digest((folder / 'third-party-source.zip').read_bytes()):
        raise ValueError('Dependency source provenance mismatch')
    verify_archive(folder / 'third-party-source.zip', dependency_manifest['files'], dependency=True)
    assets = []
    for item in package['artifacts']:
        if Path(item['name']).name != item['name']:
            raise ValueError('Unsafe artifact name')
        path = folder / item['name']
        data = path.read_bytes()
        if len(data) != item['bytes'] or digest(data) != item['sha256']:
            raise ValueError('Stale or changed package artifact')
        assets.append(item)
    installers = [a for a in assets if a['name'].endswith('-setup.exe')]
    if len(installers) != 1 or 'Local Test' in installers[0]['name']:
        raise ValueError('Expected one official identity NSIS installer')
    if len({a['name'] for a in assets}) != len(assets) or set(a['name'] for a in assets) != {installers[0]['name']} | (set() if technical else {installers[0]['name'] + '.sig'}):
        raise ValueError('Unexpected package artifact inventory')
    key_fingerprint = None
    if not technical:
        key_receipt = load(receipt)
        if key_receipt.get('role') != 'Release' or key_receipt.get('portableBackupVerified') is not True:
            raise ValueError('Deployment key/independent backup verification pending')
        result = json.loads(run('node', str(ROOT / 'scripts/verify-updater-signature.mjs'), str(folder / installers[0]['name']), str(public_key)))
        key_fingerprint = result['publicKeySha256']
        if key_fingerprint != key_receipt['publicKeySha256']:
            raise ValueError('Public key does not match deployment receipt')
        sig_name = installers[0]['name'] + '.sig'
        if sig_name not in [a['name'] for a in assets]:
            raise ValueError('Signature missing from package manifest')
    # Assets added by preparation/build are explicitly collected; unrelated files are never uploaded.
    for name in ['corresponding-source.zip', 'third-party-source.zip', 'third-party-source.manifest.json', 'LICENSE', 'README.md', 'release-notes.md']:
        data = (folder / name).read_bytes()
        assets.append({'name': name, 'bytes': len(data), 'sha256': digest(data)})
    if not technical:
        data = Path(public_key).read_bytes()
        (folder / 'updater-public.key.pub').write_bytes(data)
        assets.append({'name': 'updater-public.key.pub', 'bytes': len(data), 'sha256': digest(data)})
    manifest = {'candidate_id': provenance['candidate_id'], 'source': provenance,
                'version': expected_version, 'tag': 'v' + expected_version,
                'identifier': IDENTITY, 'mode': 'Release', 'technicalOnly': technical,
                'publicKeySha256': key_fingerprint, 'assets': assets}
    (folder / 'release-manifest.json').write_text(json.dumps(manifest, indent=2, ensure_ascii=False), encoding='utf-8')
    return manifest


def draft_request(folder, repo, target, tag, execute=False, expected_public_key=None):
    folder = Path(folder).resolve()
    manifest = load(folder / 'release-manifest.json')
    if manifest.get('identifier') != IDENTITY or manifest.get('mode') != 'Release':
        raise ValueError('Draft requires the official release identity')
    if not re.fullmatch(r'0\.(0|[1-9]\d*)\.(0|[1-9]\d*)', manifest['version']) or manifest['tag'] != 'v' + manifest['version']:
        raise ValueError('Release version/tag metadata mismatch')
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo) or not re.fullmatch(r'[0-9a-f]{40}', target):
        raise ValueError('Expected repository owner/name and exact target SHA')
    if tag != manifest['tag']:
        raise ValueError('Tag/version mismatch')
    if execute and (manifest['technicalOnly'] or not manifest['source']['committed_source'] or target != manifest['source']['base_head']):
        raise ValueError('Unsigned or uncommitted candidate cannot create a remote draft')
    if execute:
        required = {'corresponding-source.zip', 'third-party-source.zip', 'third-party-source.manifest.json', 'LICENSE', 'README.md', 'release-notes.md', 'updater-public.key.pub'}
        installers = [a['name'] for a in manifest['assets'] if a['name'].endswith('-setup.exe')]
        if len(installers) != 1 or set(a['name'] for a in manifest['assets']) != required | {installers[0], installers[0] + '.sig'}:
            raise ValueError('Draft asset inventory mismatch')
        verified = json.loads(run('node', str(ROOT / 'scripts/verify-updater-signature.mjs'),
                                  str(folder / next(a['name'] for a in manifest['assets'] if a['name'].endswith('-setup.exe'))),
                                  str(folder / 'updater-public.key.pub')))
        if not expected_public_key or verified['publicKeySha256'] != expected_public_key or verified['publicKeySha256'] != manifest['publicKeySha256']:
            raise ValueError('Draft key does not match trusted deployment fingerprint')
    for asset in manifest['assets']:
        if Path(asset['name']).name != asset['name']:
            raise ValueError('Unsafe asset name')
        data = (folder / asset['name']).read_bytes()
        if len(data) != asset['bytes'] or digest(data) != asset['sha256']:
            raise ValueError('Asset fingerprint changed after bundle validation')
    marker = '<!-- worldbuild-candidate:' + manifest['candidate_id'] + ' -->'
    request = {'tag_name': tag, 'target_commitish': target, 'name': 'Dreamrugi Worldbuild Tool ' + manifest['version'],
               'body': (folder / 'release-notes.md').read_text(encoding='utf-8-sig') + '\n\n' + marker,
               'draft': True, 'prerelease': True, 'make_latest': 'false'}
    preview = {'repository': repo, 'request': request, 'assets': manifest['assets'],
               'executeBlocked': manifest['technicalOnly'] or not manifest['source']['committed_source'] or target != manifest['source']['base_head']}
    (folder / 'draft-preview.json').write_text(json.dumps(preview, ensure_ascii=False, indent=2), encoding='utf-8')
    if not execute:
        return preview
    # Secrets are used only by gh; argument lists contain no credential or shell fragments.
    def gh(*args):
        return json.loads(run('gh', 'api', *args))
    def require_absent_tag():
        pages = gh('--paginate', '--slurp', f'repos/{repo}/git/matching-refs/tags/{tag}')
        if not isinstance(pages, list) or not pages or any(not isinstance(page, list) for page in pages):
            raise ValueError('Tag lookup returned an invalid response')
        for page in pages:
            for ref in page:
                if not isinstance(ref, dict) or not isinstance(ref.get('ref'), str):
                    raise ValueError('Tag lookup returned an invalid reference')
                if ref['ref'] == 'refs/tags/' + tag:
                    raise ValueError('Tag already exists; explicit reconciliation required')

    def require_candidate(release):
        if (release.get('draft') is not True or release.get('prerelease') is not True
                or release.get('tag_name') != tag or release.get('target_commitish') != target
                or not isinstance(release.get('body'), str) or marker not in release['body']):
            raise ValueError('Release metadata differs from candidate; uploaded assets are preserved')

    # Both creation and resumption require a successful exact-name lookup.
    # Matching refs can also contain prefix names such as v0.1.0-rc1.
    require_absent_tag()
    releases = [r for page in gh('--paginate', '--slurp', f'repos/{repo}/releases?per_page=100') for r in page]
    existing = [r for r in releases if r['tag_name'] == tag]
    if len(existing) > 1:
        raise ValueError('Multiple drafts with the same tag')
    if existing:
        release = existing[0]
        require_candidate(release)
    else:
        request_path = folder / 'draft-request.json'
        request_path.write_text(json.dumps(request, ensure_ascii=False), encoding='utf-8')
        release = gh(f'repos/{repo}/releases', '--method', 'POST', '--input', str(request_path))
        require_candidate(release)
    allowed = {a['name']: a for a in manifest['assets']}
    if len(allowed) != len(manifest['assets']):
        raise ValueError('Duplicate local asset name')
    manifest_bytes = (folder / 'release-manifest.json').read_bytes()
    allowed['release-manifest.json'] = {'name': 'release-manifest.json', 'bytes': len(manifest_bytes), 'sha256': digest(manifest_bytes)}
    remote_assets = {a['name']: a for a in release['assets']}
    if len(remote_assets) != len(release['assets']):
        raise ValueError('Duplicate remote asset name')
    if set(remote_assets) - set(allowed):
        raise ValueError('Remote draft has unexpected assets')
    for name, asset in remote_assets.items():
        if asset.get('digest') != 'sha256:' + allowed[name]['sha256']:
            raise ValueError('Remote asset differs; refusing overwrite')
    for name in allowed:
        if name not in remote_assets:
            run('gh', 'release', 'upload', tag, str(folder / name), '--repo', repo)
    fresh = gh(f'repos/{repo}/releases/{release["id"]}')
    require_candidate(fresh)
    remote_assets = {a['name']: a for a in fresh['assets']}
    if len(remote_assets) != len(fresh['assets']) or set(remote_assets) != set(allowed) or any(remote_assets[n].get('digest') != 'sha256:' + a['sha256'] for n, a in allowed.items()):
        raise ValueError('Remote draft asset verification failed')
    # A tag/release may change while assets are uploaded. This is a final
    # observation, not an atomic guard; explicit publication must recheck it.
    require_absent_tag()
    (folder / 'draft-result.json').write_text(json.dumps(fresh, ensure_ascii=False, indent=2), encoding='utf-8')
    return {'url': fresh['html_url'], 'draft': True, 'prerelease': True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    s = sub.add_parser('snapshot')
    s.add_argument('--ref', required=True); s.add_argument('--version', required=True)
    s.add_argument('--output', type=Path, required=True); s.add_argument('--overlay', type=Path)
    v = sub.add_parser('verify-source')
    v.add_argument('--folder', type=Path, required=True)
    b = sub.add_parser('bundle')
    b.add_argument('--folder', type=Path, required=True); b.add_argument('--version', required=True)
    b.add_argument('--public-key', type=Path); b.add_argument('--receipt', type=Path)
    b.add_argument('--technical', action='store_true')
    d = sub.add_parser('draft')
    d.add_argument('--folder', type=Path, required=True); d.add_argument('--repo', required=True)
    d.add_argument('--target', required=True); d.add_argument('--tag', required=True)
    d.add_argument('--execute', action='store_true')
    d.add_argument('--expected-public-key-sha256')
    args = parser.parse_args()
    if args.command == 'snapshot':
        result = snapshot(args.ref, args.version, args.output, args.overlay)
        print(json.dumps({k: result[k] for k in ['base_head', 'candidate_id', 'version', 'committed_source']}))
    elif args.command == 'verify-source':
        result = verify_snapshot(args.folder)
        print(json.dumps({'candidate_id': result['candidate_id'], 'sourceVerified': True}))
    elif args.command == 'bundle':
        result = bundle(args.folder, args.public_key, args.receipt, args.version, args.technical)
        print(json.dumps({'candidate_id': result['candidate_id'], 'technicalOnly': result['technicalOnly']}))
    else:
        result = draft_request(args.folder, args.repo, args.target, args.tag, args.execute, args.expected_public_key_sha256)
        print(json.dumps({'draft': True, 'execute': args.execute, 'repository': args.repo}))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit('Release candidate rejected: ' + str(error))

"""Boundary tests use local synthetic assets and a mocked GitHub API. No remote writes."""
import importlib.util
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('release-candidate.py'))
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
HEAD = 'a' * 40


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.folder = Path(self.temp.name)

    def save(self, name, value):
        (self.folder / name).write_text(json.dumps(value), encoding='utf-8')

    def candidate(self, technical=False, committed=True):
        payload = {'Dreamrugi Worldbuild Tool_0.1.0_x64-setup.exe': b'synthetic installer',
                   'release-notes.md': b'Trial release', 'updater-public.key.pub': b'synthetic public key'}
        for name in ['corresponding-source.zip','third-party-source.zip','third-party-source.manifest.json','LICENSE','README.md','Dreamrugi Worldbuild Tool_0.1.0_x64-setup.exe.sig']:
            payload[name] = b'local synthetic fixture'
        for name, data in payload.items():
            (self.folder / name).write_bytes(data)
        manifest = {'tag': 'v0.1.0', 'version': '0.1.0', 'candidate_id': 'c' * 64,
                    'technicalOnly': technical, 'publicKeySha256': 'f' * 64, 'identifier': r.IDENTITY, 'mode': 'Release',
                    'source': {'committed_source': committed, 'base_head': HEAD},
                    'assets': [{'name': n, 'bytes': len(d), 'sha256': r.digest(d)} for n, d in payload.items()]}
        self.save('release-manifest.json', manifest)
        return manifest

    def call_draft(self, **kwargs):
        return r.draft_request(self.folder, 'owner/repo', HEAD, 'v0.1.0', **kwargs)

    def test_paths_and_secret_boundary(self):
        for path in ['../secret', 'C:/secret', 'secrets/foo', 'logs/foo', 'key.dpapi', '.env.local']:
            with self.assertRaises(ValueError): r.safe_path(path)
        for data in [b'-----BEGIN PRIVATE KEY-----\n', b'ghp_' + b'x' * 36]:
            with self.assertRaises(ValueError): r.check_bytes(data)
        r.safe_path('scripts/build-release-candidate.ps1')

    def test_exact_archive_inventory_and_fingerprints(self):
        files = {'src/app.ts': b'fixed input'}
        r.write_zip(self.folder / 'source.zip', files)
        manifest = [{'path': p, 'bytes': len(d), 'sha256': r.digest(d)} for p, d in files.items()]
        r.verify_archive(self.folder / 'source.zip', manifest)
        with self.assertRaises(ValueError): r.verify_archive(self.folder / 'source.zip', [])
        manifest[0]['sha256'] = '0' * 64
        with self.assertRaises(ValueError): r.verify_archive(self.folder / 'source.zip', manifest)

    def test_version_identity_required_resource(self):
        tauri = {'version': '0.1.0', 'identifier': r.IDENTITY, 'bundle': {'license': 'GPL-3.0-only', 'resources': {'../README.md': 'README.md'}, 'icon': []}}
        files = {'package.json': json.dumps({'version':'0.1.0','license':'GPL-3.0-only'}).encode(),
                 'package-lock.json': json.dumps({'version':'0.1.0','packages':{'':{'version':'0.1.0'}}}).encode(),
                 'src-tauri/Cargo.toml': b'version = "0.1.0"\nlicense = "GPL-3.0-only"',
                 'src-tauri/tauri.conf.json': json.dumps(tauri).encode()}
        for name in ['LICENSE','README.md','scripts/package-windows.ps1','scripts/check-strings.mjs','src-tauri/Cargo.lock']: files[name] = b'fixture'
        r.metadata(files, '0.1.0')
        with self.assertRaises(ValueError): r.metadata(files, '0.1.2')
        tauri['identifier'] += '.e.localtest'
        files['src-tauri/tauri.conf.json'] = json.dumps(tauri).encode()
        with self.assertRaises(ValueError): r.metadata(files, '0.1.0')
        tauri['identifier'] = r.IDENTITY
        files['src-tauri/tauri.conf.json'] = json.dumps(tauri).encode()
        del files['README.md']
        with self.assertRaises(ValueError): r.metadata(files, '0.1.0')

    def test_overlay_is_explicit_and_hash_checked(self):
        (self.folder / 'README.md').write_bytes(b'candidate')
        self.save('overlay.json', {'base_head':HEAD, 'files':[{'path':'README.md','bytes':9,'sha256':r.digest(b'candidate')}]})
        def git(*args, **kwargs):
            if args[1] == 'rev-parse': return b'b'*40
            if args[1] == 'ls-tree': return b'100644 blob '+b'd'*40+b'\tREADME.md\0'
            return b'committed version'
        with patch.object(r, 'ROOT', self.folder), patch.object(r, 'run', git):
            self.assertEqual(r.fixed_files(HEAD)[1]['README.md'], b'committed version')
            self.assertEqual(r.fixed_files(HEAD, self.folder / 'overlay.json')[1]['README.md'], b'candidate')
            (self.folder / 'README.md').write_bytes(b'other')
            with self.assertRaises(ValueError): r.fixed_files(HEAD, self.folder / 'overlay.json')

    def test_remote_execution_rejects_technical_uncommitted_wrong_target_and_tag(self):
        for technical, committed in [(True, True),(False, False)]:
            self.candidate(technical, committed)
            with self.assertRaises(ValueError): self.call_draft(execute=True)
        self.candidate()
        with self.assertRaises(ValueError): r.draft_request(self.folder,'owner/repo','b'*40,'v0.1.0',True)
        with self.assertRaises(ValueError): r.draft_request(self.folder,'owner/repo',HEAD,'v0.1.2')

    def test_preview_is_draft_prerelease_and_never_latest(self):
        self.candidate(True)
        with patch.object(r,'run',side_effect=AssertionError('No remote or signing process expected')):
            preview = self.call_draft()
        self.assertTrue(preview['executeBlocked'])
        self.assertEqual([preview['request'][k] for k in ['draft','prerelease','make_latest']], [True,True,'false'])

    def test_stale_asset_rejected_before_remote(self):
        self.candidate()
        (self.folder / 'release-notes.md').write_bytes(b'older')
        with patch.object(r,'run',side_effect=AssertionError('No remote expected')):
            with self.assertRaises(ValueError): self.call_draft()

    def test_same_draft_resumes_without_upload_and_other_candidate_rejected(self):
        manifest = self.candidate()
        marker = '<!-- worldbuild-candidate:' + manifest['candidate_id'] + ' -->'
        assets = [{'name':a['name'],'digest':'sha256:'+a['sha256']} for a in manifest['assets']]
        assets.append({'name':'release-manifest.json','digest':'sha256:'+r.digest((self.folder/'release-manifest.json').read_bytes())})
        remote = {'id':1,'tag_name':'v0.1.0','target_commitish':HEAD,'draft':True,'prerelease':True,'body':marker,'assets':assets,'html_url':'https://example.invalid/draft'}
        calls = []
        def api(*args, **kwargs):
            calls.append(args)
            if args[0] == 'node': return json.dumps({'publicKeySha256':'f'*64}).encode()
            if args[1] != 'api': raise AssertionError('No upload or mutation expected')
            if 'git/matching-refs' in str(args): return b'[[]]'
            if '--slurp' in args: return json.dumps([[remote]]).encode()
            return json.dumps(remote).encode()
        with patch.object(r,'run',api):
            self.assertTrue(self.call_draft(execute=True,expected_public_key='f'*64)['draft'])
            remote['body'] = 'another candidate'
            with self.assertRaises(ValueError): self.call_draft(execute=True,expected_public_key='f'*64)
            remote['body'] = marker
            remote['draft'] = False
            with self.assertRaises(ValueError): self.call_draft(execute=True,expected_public_key='f'*64)

    def remote_case(self, *, existing=True, refs=None, changed=None, partial=False,
                    lookup_error=None, final_error=False, upload_error=False, corrupt_asset=False,
                    normalize_names=False, duplicate_alias=False, unknown_rename=False):
        manifest = self.candidate()
        assets = [{'name':a['name'], 'digest':'sha256:'+a['sha256']} for a in manifest['assets']]
        assets.append({'name':'release-manifest.json','digest':'sha256:'+r.digest((self.folder/'release-manifest.json').read_bytes())})
        remote = {'id':1,'tag_name':'v0.1.0','target_commitish':HEAD,'draft':True,'prerelease':True,
                  'body':'<!-- worldbuild-candidate:'+manifest['candidate_id']+' -->',
                  'assets':copy.deepcopy(assets[:2] if partial else assets),'html_url':'https://example.invalid/draft'}
        if normalize_names:
            for asset in remote['assets']:
                asset['name'] = asset['name'].replace(' ', '.')
        if duplicate_alias:
            remote['assets'].append(copy.deepcopy(assets[0]))
        if unknown_rename:
            remote['assets'][0]['name'] = assets[0]['name'].replace(' ', '_')
        if corrupt_asset:
            remote['assets'][-1]['digest'] = 'sha256:' + '0' * 64
        original = copy.deepcopy(remote['assets'])
        mutations = []
        lookups = 0
        def api(*args, **kwargs):
            nonlocal lookups
            if args[0] == 'node': return json.dumps({'publicKeySha256':'f'*64}).encode()
            if args[1:3] == ('release','upload'):
                mutations.append(('upload',Path(args[4]).name))
                if upload_error: raise RuntimeError('upload failed')
                uploaded = copy.deepcopy(next(a for a in assets if a['name']==Path(args[4]).name))
                if normalize_names:
                    uploaded['name'] = uploaded['name'].replace(' ', '.')
                remote['assets'].append(uploaded)
                if changed: remote.update(changed)
                return b''
            if 'git/matching-refs' in str(args):
                lookups += 1
                if lookup_error: raise RuntimeError(lookup_error)
                if final_error and lookups > 1: raise RuntimeError('final ref lookup failed')
                value = refs(lookups) if callable(refs) else ([[]] if refs is None else refs)
                return json.dumps(value).encode()
            if '--slurp' in args: return json.dumps([[remote] if existing else []]).encode()
            if '--method' in args:
                mutations.append(('create',))
                remote['assets'] = []
            if final_error and not partial and args[-1].endswith('/releases/1'):
                raise RuntimeError('final release lookup failed')
            return json.dumps(remote).encode()
        with patch.object(r,'run',api):
            try:
                result = self.call_draft(execute=True,expected_public_key='f'*64)
            except (ValueError,RuntimeError,KeyError,TypeError) as error:
                result = error
        self.assertTrue(all(a in remote['assets'] for a in original) if existing else True)
        return result, mutations, remote

    def test_exact_tag_guard_applies_to_new_and_resumed_drafts(self):
        for existing in [False, True]:
            for kind, sha in [('commit','b'*40),('commit',HEAD),('tag','d'*40)]:
                with self.subTest(existing=existing,kind=kind,sha=sha):
                    result, mutations, _ = self.remote_case(existing=existing,
                        refs=[[{'ref':'refs/tags/v0.1.0','object':{'type':kind,'sha':sha}}]])
                    self.assertIsInstance(result,ValueError)
                    self.assertEqual(mutations,[])

    def test_absent_and_prefix_tags_allow_creation_or_missing_asset_resume(self):
        for existing in [False,True]:
            for refs in [[[]], [[{'ref':'refs/tags/v0.1.0-rc1'}]]]:
                result, mutations, remote = self.remote_case(existing=existing,refs=refs,partial=True)
                self.assertIsInstance(result,dict)
                self.assertEqual(len(remote['assets']),10)
                if existing: self.assertNotIn(('upload',remote['assets'][0]['name']),mutations)
                else: self.assertIn(('create',),mutations)

    def test_tag_lookup_failure_or_malformed_response_prevents_mutation(self):
        for existing in [False,True]:
            for options in [{'lookup_error':'permission denied'},{'refs':{}},{'refs':[]},
                            {'refs':[[{}]]},{'refs':[{}]}]:
                result, mutations, _ = self.remote_case(existing=existing,**options)
                self.assertIsInstance(result,Exception)
                self.assertEqual(mutations,[])

    def test_mid_upload_metadata_or_tag_changes_are_incomplete_and_preserve_assets(self):
        for changed in [{'tag_name':'v0.1.2'},{'target_commitish':'b'*40},
                        {'body':'other candidate'},{'draft':False},{'prerelease':False}]:
            result, mutations, remote = self.remote_case(partial=True,changed=changed)
            self.assertIsInstance(result,ValueError)
            self.assertTrue(mutations)
            self.assertEqual(len(remote['assets']),10)
        result, mutations, remote = self.remote_case(partial=True,
            refs=lambda count: [[]] if count==1 else [[{'ref':'refs/tags/v0.1.0'}]])
        self.assertIsInstance(result,ValueError)
        self.assertTrue(mutations)
        self.assertEqual(len(remote['assets']),10)

    def test_upload_and_final_lookup_failures_never_report_success(self):
        for options in [{'partial':True,'upload_error':True},
                        {'partial':True,'final_error':True},{'final_error':True}]:
            result, _, _ = self.remote_case(**options)
            self.assertIsInstance(result,RuntimeError)

    def test_partial_draft_asset_mismatch_blocks_all_uploads(self):
        result, mutations, remote = self.remote_case(partial=True,corrupt_asset=True)
        self.assertIsInstance(result,ValueError)
        self.assertEqual(mutations,[])
        self.assertEqual(len(remote['assets']),2)

    def test_github_renamed_assets_support_create_resume_and_partial_uploads(self):
        # This name pair was observed in the real GitHub RELEASE001 draft.
        for existing, partial in [(False, False), (True, False), (True, True)]:
            with self.subTest(existing=existing, partial=partial):
                result, mutations, remote = self.remote_case(existing=existing,
                    partial=partial, normalize_names=True)
                self.assertIsInstance(result, dict)
                self.assertEqual(len(remote['assets']), 10)
                self.assertIn('Dreamrugi.Worldbuild.Tool_0.1.0_x64-setup.exe',
                    [a['name'] for a in remote['assets']])
                if existing and not partial:
                    self.assertEqual(mutations, [])

    def test_normalized_asset_corruption_prevents_all_uploads(self):
        result, mutations, _ = self.remote_case(partial=True,
            normalize_names=True, corrupt_asset=True)
        self.assertIsInstance(result, ValueError)
        self.assertIn('Remote asset differs', str(result))
        self.assertEqual(mutations, [])

    def test_duplicate_original_and_normalized_aliases_prevent_uploads(self):
        result, mutations, _ = self.remote_case(normalize_names=True, duplicate_alias=True)
        self.assertIsInstance(result, ValueError)
        self.assertIn('Duplicate remote asset', str(result))
        self.assertEqual(mutations, [])

    def test_unrecognized_rename_prevents_all_uploads(self):
        result, mutations, _ = self.remote_case(normalize_names=True, unknown_rename=True)
        self.assertIsInstance(result, ValueError)
        self.assertEqual(mutations, [])


if __name__ == '__main__': unittest.main()

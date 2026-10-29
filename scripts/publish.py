"""Publish only the six tested packages, pinned to the workflow commit.

Existing published versions are verified, never overwritten. Partial drafts can
resume missing uploads, but mismatched existing assets or tags fail closed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def gh(*args, missing_ok=False):
    result = subprocess.run(['gh', *args], text=True, capture_output=True)
    if result.returncode:
        if missing_ok and 'HTTP 404' in result.stderr:
            return None
        raise RuntimeError(f'GitHub operation failed ({args[0]}): {result.stderr.strip()}')
    return json.loads(result.stdout) if result.stdout.strip() else {}


def resolve_tag(repository, version):
    ref = gh('api', f'repos/{repository}/git/ref/tags/{version}', missing_ok=True)
    if ref is None:
        return None
    obj = ref['object']
    for _ in range(10):
        if obj['type'] == 'commit':
            return obj['sha']
        if obj['type'] != 'tag':
            raise RuntimeError('Release tag does not point to a commit')
        obj = gh('api', f'repos/{repository}/git/tags/{obj["sha"]}')['object']
    raise RuntimeError('Excessive nested annotated tags')


def check_source(release, tag_commit, expected_commit):
    if tag_commit is not None and tag_commit != expected_commit:
        raise RuntimeError('Version tag points to different source; bump the version')
    if tag_commit is None and release.get('target_commitish') != expected_commit:
        raise RuntimeError('Draft source is not the exact tested commit')


def check_assets(release, expected, repository, version):
    assets = release['assets']
    actual = {a['name']: a for a in assets}
    if len(actual) != len(assets) or set(actual) != set(expected):
        raise RuntimeError('Release asset inventory differs from the tested package inventory')
    for name, path in expected.items():
        asset = actual[name]
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if asset.get('size') != path.stat().st_size:
            raise RuntimeError(f'Release asset size differs: {name}')
        remote_digest = asset.get('digest')
        if remote_digest:
            if remote_digest != 'sha256:' + digest:
                raise RuntimeError(f'Release asset checksum differs: {name}')
        else:
            with tempfile.TemporaryDirectory() as directory:
                subprocess.run(['gh', 'release', 'download', version, '--repo', repository, '--pattern', name, '--dir', directory], check=True)
                if hashlib.sha256((Path(directory) / name).read_bytes()).hexdigest() != digest:
                    raise RuntimeError(f'Downloaded release asset checksum differs: {name}')


def find_release(repository, version):
    # The by-tag REST route is for published releases. Authenticated listing
    # includes drafts and prevents duplicate drafts on a resumed failed upload.
    pages = gh('api', '--paginate', '--slurp', f'repos/{repository}/releases?per_page=100')
    matches = [release for page in pages for release in page if release['tag_name'] == version]
    if len(matches) > 1:
        raise RuntimeError('Multiple releases share the version; refusing ambiguous publication')
    return matches[0] if matches else None


def publish_release(repository, commit, version, expected):
    tag_commit = resolve_tag(repository, version)
    if tag_commit is not None and tag_commit != commit:
        raise RuntimeError('Existing version tag points to different source; bump the version, never overwrite')
    release = find_release(repository, version)
    if release is None:
        release = gh('api', '--method', 'POST', f'repos/{repository}/releases',
                     '-f', f'tag_name={version}', '-f', f'target_commitish={commit}',
                     '-f', f'name={version}', '-F', 'generate_release_notes=true', '-F', 'draft=true')
    release_id = release['id']
    if not isinstance(release_id, int) or release_id <= 0:
        raise RuntimeError('GitHub did not return a valid release ID')
    endpoint = f'repos/{repository}/releases/{release_id}'
    release = gh('api', endpoint)
    check_source(release, resolve_tag(repository, version), commit)
    if release['draft']:
        existing = {a['name'] for a in release['assets']}
        if not existing.issubset(expected):
            raise RuntimeError('Draft has unexpected assets; refusing to replace them')
        missing = [str(path) for name, path in expected.items() if name not in existing]
        if missing:
            subprocess.run(['gh', 'release', 'upload', version, '--repo', repository, *missing], check=True)
        release = gh('api', endpoint)
        check_assets(release, expected, repository, version)
        check_source(release, resolve_tag(repository, version), commit)
        gh('api', '--method', 'PATCH', endpoint, '-F', 'draft=false')
    else:
        check_assets(release, expected, repository, version)
    if resolve_tag(repository, version) != commit:
        raise RuntimeError('Final release tag does not match the tested commit')
    final = gh('api', endpoint)
    if final['draft']:
        raise RuntimeError('Release is still a draft')
    check_assets(final, expected, repository, version)
    return final['html_url']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repository', default=os.environ.get('GITHUB_REPOSITORY'))
    parser.add_argument('--commit', default=os.environ.get('GITHUB_SHA'))
    args = parser.parse_args()
    if not args.repository or not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repository):
        parser.error('A verified owner/repository is required')
    if not args.commit or not re.fullmatch(r'[a-f0-9]{40}', args.commit):
        parser.error('The exact tested 40-character commit SHA is required')
    version = 'v' + tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    # Re-validate the exact six archives and regenerate the manifest before sending.
    from package import manifest
    manifest(version[1:], ROOT / 'dist')
    expected = {p.name: p for p in (ROOT / 'dist').iterdir()}
    print(publish_release(args.repository, args.commit, version, expected))


if __name__ == '__main__':
    main()

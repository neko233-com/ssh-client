"""Deterministic release packaging; run only after native tests pass."""
import argparse
import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = (
    'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu',
    'x86_64-apple-darwin', 'aarch64-apple-darwin',
    'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc',
)


def archive_name(version, target):
    if target not in TARGETS:
        raise ValueError('Unsupported release target')
    extension = 'zip' if 'windows' in target else 'tar.gz'
    return f'ssh-client-v{version}-{target}.{extension}'


def pack(version, target, binary_dir, destination):
    destination.mkdir(parents=True, exist_ok=True)
    extension = '.exe' if 'windows' in target else ''
    files = [(name + extension, binary_dir / (name + extension)) for name in ('ssh-client', 'ssh')]
    for _, path in files:
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f'Missing or empty binary: {path}')
        subprocess.run([str(path), '--version'], check=True, timeout=15)
    output = destination / archive_name(version, target)
    if extension:
        with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
            for name, path in files:
                info = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
                info.external_attr = 0o755 << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, path.read_bytes())
    else:
        import gzip
        with output.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', mtime=0, filename='') as compressed:
            with tarfile.open(fileobj=compressed, mode='w') as archive:
                for name, path in files:
                    data = path.read_bytes()
                    info = tarfile.TarInfo(name)
                    info.size, info.mode, info.mtime = len(data), 0o755, 0
                    archive.addfile(info, io.BytesIO(data))
    return output


def manifest(version, destination):
    expected = {archive_name(version, target) for target in TARGETS}
    actual = {p.name for p in destination.iterdir() if p.name != 'SHA256SUMS'}
    if actual != expected:
        raise ValueError(f'Release assets differ; missing={expected - actual}, extra={actual - expected}')
    lines = []
    for name in sorted(expected):
        path = destination / name
        if not path.stat().st_size:
            raise ValueError(f'Empty release asset: {name}')
        lines.append(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {name}\n')
    (destination / 'SHA256SUMS').write_text(''.join(lines), encoding='ascii')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', choices=TARGETS)
    parser.add_argument('--manifest', action='store_true')
    args = parser.parse_args()
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']['version']
    if args.manifest:
        manifest(version, ROOT / 'dist')
    elif args.target:
        print(pack(version, args.target, ROOT / 'target' / args.target / 'release', ROOT / 'dist'))
    else:
        parser.error('Specify --target or --manifest')


if __name__ == '__main__':
    main()

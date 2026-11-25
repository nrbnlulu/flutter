#!/usr/bin/env python3
"""Generate merged checksums and platform revisions for a beta release."""

import argparse
import hashlib
import json
from pathlib import Path
import re


PLATFORMS = ('linux', 'android', 'windows', 'macos')
ASSET_PATTERNS = (
    'flutter-rust-*.tar.gz',
    'rlutter-flutter-*.tar.gz',
    'rlutter.exe',
    'flutter-tools.snapshot',
)
# Older releases ran sha256sum from the repository root, recording build/ paths.
CHECKSUM_LINE = re.compile(r'([0-9a-fA-F]{64}) [ *](?:build/)?([A-Za-z0-9][A-Za-z0-9._-]*)')


def update_metadata(directory, revision, platforms, previous=None):
    """Retain prior entries and replace metadata supplied by this build."""
    checksums = {}
    revisions = {}
    if previous is not None:
        checksum_path = previous / 'SHA256SUMS'
        if checksum_path.exists():
            for number, line in enumerate(checksum_path.read_text().splitlines(), 1):
                match = CHECKSUM_LINE.fullmatch(line)
                if match is None:
                    raise ValueError(f'{checksum_path}:{number}: invalid checksum line')
                digest, name = match.groups()
                if name in checksums:
                    raise ValueError(f'{checksum_path}:{number}: duplicate checksum filename {name}')
                checksums[name] = digest.lower()
        revision_path = previous / 'platform-revisions.json'
        if revision_path.exists():
            revisions = json.loads(revision_path.read_text())
            if not isinstance(revisions, dict) or any(
                platform not in PLATFORMS or not isinstance(sha, str)
                for platform, sha in revisions.items()
            ):
                raise ValueError(f'{revision_path}: expected a platform-to-revision object')

    for pattern in ASSET_PATTERNS:
        for asset in directory.glob(pattern):
            if asset.is_file():
                digest = hashlib.sha256()
                with asset.open('rb') as source:
                    for chunk in iter(lambda: source.read(1024 * 1024), b''):
                        digest.update(chunk)
                checksums[asset.name] = digest.hexdigest()

    revisions.update(dict.fromkeys(platforms, revision))
    (directory / 'SHA256SUMS').write_text(
        ''.join(f'{checksums[name]}  {name}\n' for name in sorted(checksums)),
        encoding='utf-8',
    )
    (directory / 'platform-revisions.json').write_text(
        json.dumps(revisions, indent=2, sort_keys=True) + '\n', encoding='utf-8'
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path, help='directory containing current release assets')
    parser.add_argument('revision', help='revision SHA for the selected platforms')
    parser.add_argument('--platform', action='append', choices=PLATFORMS, required=True)
    parser.add_argument('--previous', type=Path, help='directory containing prior release metadata')
    args = parser.parse_args()
    if not args.directory.is_dir():
        parser.error(f'not a directory: {args.directory}')
    try:
        update_metadata(args.directory, args.revision, args.platform, args.previous)
    except (OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == '__main__':
    main()

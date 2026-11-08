#!/usr/bin/env python3
"""Check tracked and non-ignored files for private game inputs and outputs.

This is an accidental-inclusion check, not a determination of code ownership.
Run from any directory; only Git-visible files are checked.
"""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = (
    'assets/', 'reference/', 'gamefiles/', 'mods/', 'modkit/',
    'target/', 'crates/n3ds/romfs/', 'crates/n3ds/sdcard/',
    'crates/n3ds/target/', 'tools/assetripper/', 'tools/cia/bin/',
    'tools/cia/src/', 'dist/',
)
GAME_EXTENSIONS = {
    '.asset', '.anim', '.mat', '.meta', '.json', '.cs', '.dll', '.exe', '.assets', '.ress', '.resource', '.unity',
    '.prefab', '.bundle', '.png', '.jpg', '.jpeg', '.ogg', '.wav',
    '.mp3', '.adp', '.t3x', '.bin', '.ttf', '.otf', '.3dsx', '.cia',
    '.smdh', '.elf', '.cdc',
}

# Binary files deliberately published after manual review (exact paths only).
REVIEWED_BINARIES = {'banner/gameplay.webp'}


def problem(name, data):
    path = Path(name)
    if name in REVIEWED_BINARIES:
        return None
    if name.startswith(PRIVATE) or path.suffix.lower() in GAME_EXTENSIONS:
        return 'private game/tool/build path or asset extension'
    if b'\0' in data:
        return 'unexpected binary file'
    # Common signatures from ILSpy/AssetRipper script exports.
    if any(marker in data for marker in (
        b'// ' + b'Decompiled with', b'// ' + b'Assembly-CSharp, Version=',
        b'using ' + b'UnityEngine;', b'using ' + b'UnityEngine\r',
    )):
        return 'possible decompiled Unity source'
    return None


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args])


def main():
    failures = []
    names = set(git('ls-files', '--cached', '--others', '--exclude-standard', '-z').split(b'\0'))
    checked = 0
    for raw in sorted(names - {b''}):
        name = raw.decode('utf-8')
        path = ROOT / name
        if not path.exists():
            continue
        if path.is_symlink():
            failures.append(f'{name}: symlinks require manual publication review')
            continue
        reason = problem(name, path.read_bytes())
        if reason:
            failures.append(f'{name}: {reason}')
        checked += 1
    # Check staged contents too: an ignored or edited working copy must not
    # hide a private file already in the index.
    for record in git('ls-files', '--stage', '-z').split(b'\0'):
        if not record:
            continue
        info, raw = record.split(b'\t', 1)
        mode, oid, stage = info.split()
        name = raw.decode('utf-8')
        if mode != b'100644' and mode != b'100755':
            failures.append(f'index:{name}: unsupported file mode {mode.decode()}')
            continue
        reason = problem(name, git('cat-file', 'blob', oid.decode()))
        if reason:
            failures.append(f'index:{name}: {reason}')
    if failures:
        print('\n'.join(sorted(set(failures))), file=sys.stderr)
        return 1
    print(f'Publication file check passed ({checked} working-tree files plus staged blobs).')
    return 0


if __name__ == '__main__':
    sys.exit(main())

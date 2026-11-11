#!/usr/bin/env python3
"""Choose a monotonic patch release, or validate an explicit test prerelease."""
import os
import re
import subprocess
from pathlib import Path


def version():
    requested = os.environ.get('RELEASE_VERSION', '')
    if requested:
        if not re.fullmatch(r'0\.\d+\.\d+-rc\.\d+', requested):
            raise SystemExit('Manual releases must be 0.x.y-rc.N')
        return requested
    tags = subprocess.check_output(['git', 'tag', '--list', 'v*'], text=True)
    versions = [tuple(map(int, match.groups())) for tag in tags.splitlines()
                if (match := re.fullmatch(r'v(\d+)\.(\d+)\.(\d+)', tag))]
    current = tuple(map(int, re.search(r'(?m)^version = "(\d+\.\d+\.\d+)"', Path('Cargo.toml').read_text())[1].split('.')))
    major, minor, patch = max([current, *versions])
    return f'{major}.{minor}.{patch + 1}'


if __name__ == '__main__':
    chosen = version()
    path = Path('Cargo.toml')
    path.write_text(re.sub(r'(?m)^version = "[^"]+"', f'version = "{chosen}"', path.read_text(), count=1))
    print(chosen)

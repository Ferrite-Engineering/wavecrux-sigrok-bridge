#!/usr/bin/env python3
"""Check that README.md lists exactly the archives release.yaml builds.

The README's download list is what a user reads to pick an archive. If it
names a target the release workflow no longer builds (the Intel macOS
archive after v0.1.1, for example), the user goes looking for a file that
does not exist. If the workflow gains a target the README omits, nobody
finds it. This compares the two and fails on any difference.

Run from the repository root:  ./tool/check-release-targets.py
"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "release.yaml"
README = ROOT / "README.md"

# One matrix entry per line: `- { os: macos, arch: arm64, runner: ... }`.
MATRIX_RE = re.compile(r"^\s*-\s*\{\s*os:\s*(\w+)\s*,\s*arch:\s*(\w+)\s*,", re.M)
# README download list: `wavecrux-sigrok-bridge-vX.Y.Z-<os>-<arch>.<ext>`.
README_RE = re.compile(r"wavecrux-sigrok-bridge-vX\.Y\.Z-([a-z]+)-([a-z0-9_]+)\.(?:tar\.gz|zip)")


def matrix_targets(text: str) -> set[tuple[str, str]]:
    return set(MATRIX_RE.findall(text))


def readme_targets(text: str) -> set[tuple[str, str]]:
    return set(README_RE.findall(text))


def compare(workflow_text: str, readme_text: str) -> list[str]:
    built = matrix_targets(workflow_text)
    listed = readme_targets(readme_text)
    problems = []
    if not built:
        problems.append("no matrix targets found in release.yaml (pattern rotted?)")
    if not listed:
        problems.append("no archive names found in README.md (pattern rotted?)")
    for os_, arch in sorted(listed - built):
        problems.append(f"README.md lists {os_}-{arch}, which release.yaml does not build")
    for os_, arch in sorted(built - listed):
        problems.append(f"release.yaml builds {os_}-{arch}, which README.md does not list")
    return problems


def self_test() -> None:
    wf = (
        "        target:\n"
        "          - { os: linux,   arch: x86_64,  runner: ubuntu-latest, sigrok: true }\n"
        "          - { os: macos,   arch: arm64,   runner: macos-15, sigrok: true }\n"
    )
    good = (
        "* `wavecrux-sigrok-bridge-vX.Y.Z-macos-arm64.tar.gz`\n"
        "* `wavecrux-sigrok-bridge-vX.Y.Z-linux-x86_64.tar.gz`\n"
    )
    stale = good + "* `wavecrux-sigrok-bridge-vX.Y.Z-macos-x86_64.tar.gz`\n"
    missing = "* `wavecrux-sigrok-bridge-vX.Y.Z-linux-x86_64.tar.gz`\n"
    assert compare(wf, good) == [], "matching lists must pass"
    assert any("macos-x86_64" in p for p in compare(wf, stale)), "a stale README entry must fail"
    assert any("macos-arm64" in p for p in compare(wf, missing)), "an unlisted target must fail"
    assert compare("", good), "an empty matrix must fail"


def main() -> int:
    self_test()
    problems = compare(WORKFLOW.read_text(), README.read_text())
    for p in problems:
        print(f"error: {p}", file=sys.stderr)
    if problems:
        return 1
    built = sorted(matrix_targets(WORKFLOW.read_text()))
    print("README.md matches release.yaml: " + ", ".join(f"{o}-{a}" for o, a in built))
    return 0


if __name__ == "__main__":
    sys.exit(main())

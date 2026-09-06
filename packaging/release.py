#!/usr/bin/env python3
"""Bump Lahza's version and atomically push master and its release tag."""

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path


VERSION = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def version_key(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError(f"Invalid version: {value}; use X.Y.Z or X.Y.Z-rc.1")
    major, minor, patch, prerelease = match.groups()
    identifiers = ()
    if prerelease:
        for part in prerelease.split("."):
            if part.isdigit() and len(part) > 1 and part.startswith("0"):
                raise ValueError("Numeric prerelease identifiers cannot have leading zeros")
        identifiers = tuple(
            (0, int(part)) if part.isdigit() else (1, part)
            for part in prerelease.split(".")
        )
    return (int(major), int(minor), int(patch), prerelease is None, identifiers)


def git(root, *args):
    return subprocess.check_output(
        ["git", "-C", str(root), *args], text=True
    ).strip()


def replace_package_version(text, section, old, new):
    blocks = re.split(r"(?=^\[)", text, flags=re.MULTILINE)
    matches = 0
    for index, block in enumerate(blocks):
        if not block.startswith(section + "\n"):
            continue
        parsed = tomllib.loads(block)
        package = parsed["package"]
        if isinstance(package, list):
            package = package[0]
        if package.get("name") != "lahza":
            continue
        if package["version"] != old:
            raise ValueError("Cargo.toml and Cargo.lock versions do not match")
        blocks[index], count = re.subn(
            r'^version\s*=\s*"[^"\n]+"', f'version = "{new}"',
            block, count=1, flags=re.MULTILINE,
        )
        matches += count
    if matches != 1:
        raise ValueError(f"Expected exactly one Lahza package in {section}")
    return "".join(blocks)


def release(root, requested=None, dry_run=False):
    if git(root, "branch", "--show-current") != "master":
        raise ValueError("Switch to master before releasing")
    if git(root, "status", "--porcelain"):
        raise ValueError("Commit or stash your working changes before releasing")

    manifest = root / "Cargo.toml"
    current = tomllib.loads(manifest.read_text())["package"]["version"]
    current_key = version_key(current)
    new = requested or f"{current_key[0]}.{current_key[1]}.{current_key[2] + 1}"
    if version_key(new) <= current_key:
        raise ValueError(f"New version must be greater than {current}")
    if len(new) > 32:
        raise ValueError("Snap versions must be at most 32 characters")
    tag = f"v{new}"

    git(root, "fetch", "origin", "master", "--tags")
    remote = git(root, "rev-parse", "FETCH_HEAD")
    if git(root, "merge-base", "HEAD", remote) != remote:
        raise ValueError("master is behind or diverged from origin/master; update it first")
    if git(root, "tag", "--list", tag):
        raise ValueError(f"Tag {tag} already exists")

    updates = {
        manifest: replace_package_version(manifest.read_text(), "[package]", current, new),
        root / "Cargo.lock": replace_package_version(
            (root / "Cargo.lock").read_text(), "[[package]]", current, new
        ),
    }
    # Keep installation examples and the hand-written release notes in sync.
    for relative in ("README.md", "packaging/RELEASE-NOTES.md"):
        path = root / relative
        if path.exists():
            text = path.read_text()
            updated = text.replace(f"lahza_{current}_", f"lahza_{new}_")
            updated = updated.replace(f"lahza-{current}-", f"lahza-{new}-")
            updated = updated.replace(f"## Lahza v{current}\n", f"## Lahza v{new}\n")
            if updated != text:
                updates[path] = updated

    print(f"Release {current} -> {new} on master ({tag})", flush=True)
    if dry_run:
        print("Dry run: would update " + ", ".join(str(p.relative_to(root)) for p in updates))
        print("Would commit, create an annotated tag, and atomically push master and the tag.")
        return
    for path, content in updates.items():
        path.write_text(content)
    git(root, "add", "--", *(str(path.relative_to(root)) for path in updates))
    git(root, "commit", "-m", f"Release v{new}")
    git(root, "tag", "-a", tag, "-m", f"Lahza v{new}")
    try:
        git(root, "push", "--atomic", "origin", "HEAD:refs/heads/master", f"refs/tags/{tag}")
    except subprocess.CalledProcessError:
        print(
            "Push failed; the release commit and tag are retained locally.\n"
            f"After resolving the error, retry: git push --atomic origin master {tag}",
            file=sys.stderr,
        )
        raise
    print(f"Pushed {tag}. GitHub Actions will build, test, and publish the release.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", nargs="?", help="New version; defaults to the next patch")
    parser.add_argument("--dry-run", action="store_true", help="Validate without changing files or pushing")
    args = parser.parse_args()
    try:
        root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
        release(root, args.version, args.dry_run)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Release failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

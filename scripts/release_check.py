#!/usr/bin/env python3
"""Validate a single-crate release; require Python 3.11+ and no dependencies.

Without --check-registry this is a local, read-only metadata check. The optional
registry check reads the public sparse index so a retried publish can safely skip
a version only when its SHA-256 equals the local archive. Network failures and
conflicting archives fail closed. This script never publishes or changes tags.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen


SEMVER = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?",
    re.ASCII,
)
CRATE_NAME = re.compile(r"[a-z][a-z0-9_-]{0,63}")
PLACEHOLDER = re.compile(r"\b(?:todo|changeme|your[-_]org|your[-_]name)\b|[<>]", re.I)
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


class ReleaseError(Exception):
    """A condition that must be resolved before publishing."""


def validate_manifest(path: Path, expected_name: str, tag: str | None) -> tuple[str, str]:
    """Accept literal package metadata, valid SemVer, and no dependency tables."""
    with path.open("rb") as source:
        manifest = tomllib.load(source)
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise ReleaseError("Cargo.toml must contain a [package] table")

    name, version = package.get("name"), package.get("version")
    if not isinstance(name, str) or not CRATE_NAME.fullmatch(name):
        raise ReleaseError("package.name must be a literal lowercase crates.io name")
    if name != expected_name:
        raise ReleaseError(f"expected package {expected_name!r}, found {name!r}")
    if not isinstance(version, str) or not SEMVER.fullmatch(version):
        raise ReleaseError("package.version must be a literal SemVer 2.0 version")
    if tag is not None and tag != f"v{version}":
        raise ReleaseError(f"release tag must be exactly v{version}")

    for field in ("description", "license"):
        value = package.get(field)
        if not isinstance(value, str) or not value.strip() or PLACEHOLDER.search(value):
            raise ReleaseError(f"package.{field} is missing or contains a placeholder")
    # Repository identity is optional until an actual repository is created.
    for field in ("repository", "homepage", "documentation"):
        value = package.get(field)
        if value is not None and (
            not isinstance(value, str)
            or not value.startswith("https://")
            or "example.com" in value
            or PLACEHOLDER.search(value)
        ):
            raise ReleaseError(f"package.{field} must be a real HTTPS URL when supplied")
    if package.get("publish") is False:
        raise ReleaseError("package.publish = false prevents publication")
    if "publish" in package and package["publish"] != ["crates-io"]:
        raise ReleaseError("package.publish must be omitted or exactly ['crates-io']")

    # Enforce the zero-dependency policy, including development and target tables.
    scopes = [manifest, *manifest.get("target", {}).values()]
    for scope in scopes:
        if any(scope.get(table) for table in DEPENDENCY_TABLES):
            raise ReleaseError("the MVP must have zero runtime, build, and dev dependencies")
    return name, version


def index_path(name: str) -> str:
    """Cargo's documented sparse-index directory layout, including short names."""
    name = name.lower()
    if len(name) < 3:
        return f"{len(name)}/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def already_published(name: str, version: str, archive: Path) -> bool:
    """Skip only an identical, non-yanked version already present on crates.io."""
    expected_archive = f"{name}-{version}.crate"
    if archive.name != expected_archive or not archive.is_file():
        raise ReleaseError(f"archive must be an existing {expected_archive}")
    with archive.open("rb") as source:
        checksum = hashlib.file_digest(source, "sha256").hexdigest()
    request = Request(
        f"https://index.crates.io/{index_path(name)}",
        headers={"User-Agent": "recovertick-release-check/1.0", "Cache-Control": "no-cache"},
    )
    try:
        with urlopen(request, timeout=20) as response:
            # A bounded read avoids interpreting an unexpectedly large response.
            body = response.read(16 * 1024 * 1024 + 1)
    except HTTPError as error:
        if error.code == 404:
            return False
        raise ReleaseError(f"registry returned HTTP {error.code}; retry later") from error
    if len(body) > 16 * 1024 * 1024:
        raise ReleaseError("registry response exceeded the size limit")
    for line in body.decode("utf-8").splitlines():
        entry = json.loads(line)
        # Registries ignore build metadata when enforcing version uniqueness.
        if entry["vers"].split("+", 1)[0] == version.split("+", 1)[0]:
            if entry.get("yanked", False):
                raise ReleaseError("the existing version is yanked; use a new version")
            if entry["cksum"] != checksum:
                raise ReleaseError("this version exists with a different archive; bump the version")
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))
    parser.add_argument("--expected-name", default="recovertick")
    parser.add_argument("--tag", help="exact pushed tag, such as v0.1.0")
    parser.add_argument("--archive", type=Path)
    parser.add_argument("--check-registry", action="store_true")
    parser.add_argument("--github-output", type=Path, help="append safe GitHub step outputs")
    args = parser.parse_args()
    if args.check_registry and args.archive is None:
        parser.error("--check-registry requires --archive")

    try:
        name, version = validate_manifest(args.manifest, args.expected_name, args.tag)
        outputs = {"name": name, "version": version}
        if args.check_registry:
            exists = already_published(name, version, args.archive)
            outputs["already_published"] = str(exists).lower()
        if args.github_output is not None:
            with args.github_output.open("a", encoding="utf-8") as output:
                # Validated name/SemVer cannot inject additional output lines.
                output.writelines(f"{key}={value}\n" for key, value in outputs.items())
        print(json.dumps(outputs, sort_keys=True))
        return 0
    except (ReleaseError, OSError, ValueError, KeyError, URLError) as error:
        print(f"Release validation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

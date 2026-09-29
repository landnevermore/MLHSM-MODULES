#!/usr/bin/env python3
"""Validate the MLHSM module registry.

Used by CI on pull requests and runnable locally. The same rules are mirrored
by the Rust client in MLHSM (hsm-service/src/registry/validate.rs) - if the two
ever disagree, the Rust side is authoritative, because it is what actually
refuses bad data at runtime.

Checks, in order:
  1. registry.json parses and declares a schema version we know
  2. every module entry is structurally valid
  3. every referenced manifest exists and is valid
  4. the manifest id matches the catalog entry id
  5. module ids are unique across the catalog
  6. artifact URLs are absolute https
  7. dependencies reference modules that exist

Exit code 0 = valid, 1 = at least one error. Errors are printed to stderr, one
per line, so CI output stays readable.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "registry.json"

KNOWN_SCHEMAS = {"mlhsm-registry/1"}
KNOWN_MODULE_API = re.compile(r"^mlhsm-module/\d+$")

SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)

MODULE_ID = re.compile(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$")
REPO_PATH = re.compile(r"^[A-Za-z0-9._/-]+$")
ROUTE_PREFIX = re.compile(r"^/api/v\d+/[A-Za-z0-9/_-]+$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
TABLE_NAME = re.compile(r"^[A-Za-z0-9_]+$")

PERMISSIONS = {
    "docker",
    "network",
    "filesystem",
    "credentials",
    "notifications",
    "system",
}

# Routes the host owns, copied from `core_routes()` in
# hsm-core/src/module.rs. A module claiming one of these is rejected - the
# same rule the host enforces at registration time. Keep the two in sync.
CORE_ROUTE_PREFIXES = {
    "/api/v1/auth",
    "/api/v1/backups",
    "/api/v1/caddy",
    "/api/v1/certificates",
    "/api/v1/cloudflare",
    "/api/v1/dashboard",
    "/api/v1/db",
    "/api/v1/ddns",
    "/api/v1/diagnostics",
    "/api/v1/healthz",
    "/api/v1/metrics",
    "/api/v1/modules",
    "/api/v1/notifications",
    "/api/v1/security",
    "/api/v1/service",
    "/api/v1/settings",
    "/api/v1/setup",
    "/api/v1/status",
    "/api/v1/tls",
    "/api/v1/websites",
}

errors: list[str] = []


def err(msg: str) -> None:
    errors.append(msg)


def load_json(path: Path) -> object | None:
    try:
        with path.open(encoding="utf-8") as fh:
            return json.load(fh)
    except FileNotFoundError:
        err(f"{path.relative_to(ROOT)}: file not found")
    except json.JSONDecodeError as exc:
        err(f"{path.relative_to(ROOT)}: invalid JSON at line {exc.lineno}: {exc.msg}")
    return None


def check_semver(value: str, where: str) -> None:
    if not SEMVER.match(value or ""):
        err(f"{where}: not a valid SemVer version: {value!r}")


def check_module_id(value: str, where: str) -> None:
    if not MODULE_ID.match(value or ""):
        err(f"{where}: invalid module id {value!r} (lowercase, digits, dashes)")


def check_manifest(path: Path, entry_id: str) -> None:
    where = str(path.relative_to(ROOT))
    data = load_json(path)
    if not isinstance(data, dict):
        err(f"{where}: manifest must be an object")
        return

    for field in ("id", "name", "description", "author", "version"):
        if not data.get(field):
            err(f"{where}: missing required field {field!r}")

    mid = data.get("id", "")
    check_module_id(mid, f"{where}.id")
    if mid and mid != entry_id:
        err(f"{where}.id is {mid!r} but the catalog lists it as {entry_id!r}")

    check_semver(data.get("version", ""), f"{where}.version")

    mlhsm = data.get("mlhsm")
    if not isinstance(mlhsm, dict):
        err(f"{where}.mlhsm: missing or not an object")
    else:
        check_semver(mlhsm.get("min_version", ""), f"{where}.mlhsm.min_version")
        if mlhsm.get("max_version"):
            check_semver(mlhsm["max_version"], f"{where}.mlhsm.max_version")
            if SEMVER.match(mlhsm.get("min_version", "")) and SEMVER.match(
                mlhsm["max_version"]
            ):
                lo = mlhsm["min_version"].split("-")[0].split("+")[0]
                hi = mlhsm["max_version"].split("-")[0].split("+")[0]
                if tuple(map(int, lo.split("."))) > tuple(map(int, hi.split("."))):
                    err(f"{where}.mlhsm: min_version is greater than max_version")
        api = mlhsm.get("api")
        if api and not KNOWN_MODULE_API.match(api):
            err(f"{where}.mlhsm.api: unknown module API {api!r}")

    for perm in data.get("permissions", []):
        if not isinstance(perm, dict):
            err(f"{where}.permissions: entries must be objects")
            continue
        pid = perm.get("id")
        if pid not in PERMISSIONS:
            err(
                f"{where}.permissions: unknown permission {pid!r} "
                f"(known: {', '.join(sorted(PERMISSIONS))})"
            )

    for dep_key in ("dependencies", "optional_dependencies"):
        for dep in data.get(dep_key, []):
            if not isinstance(dep, dict) or not dep.get("id"):
                err(f"{where}.{dep_key}: entries need an id")
                continue
            check_module_id(dep["id"], f"{where}.{dep_key}.id")
            if dep.get("version"):
                check_semver(dep["version"], f"{where}.{dep_key}.version")

    for prefix in data.get("route_prefixes", []):
        if not ROUTE_PREFIX.match(prefix or ""):
            err(f"{where}.route_prefixes: malformed prefix {prefix!r}")
            continue
        for core in CORE_ROUTE_PREFIXES:
            if prefix == core or prefix.startswith(core + "/"):
                err(f"{where}.route_prefixes: claims Core route {prefix!r}")
                break

    for table in data.get("database_tables", []):
        if not TABLE_NAME.match(table or ""):
            err(f"{where}.database_tables: malformed table name {table!r}")

    for owned in data.get("owned_paths", []):
        if not REPO_PATH.match(owned or "") or owned.startswith("/") or ".." in owned:
            err(f"{where}.owned_paths: unsafe path {owned!r}")

    art = data.get("artifact")
    if isinstance(art, dict):
        url = art.get("url")
        if url and not url.startswith("https://"):
            err(f"{where}.artifact.url: must be absolute https, got {url!r}")
        digest = art.get("sha256")
        if digest and not SHA256.match(digest):
            err(f"{where}.artifact.sha256: must be 64 lowercase hex chars")


def main() -> int:
    data = load_json(REGISTRY)
    if not isinstance(data, dict):
        print("registry.json is not an object - aborting", file=sys.stderr)
        for e in errors:
            print(f"  error: {e}", file=sys.stderr)
        return 1

    schema = data.get("schema_version", "")
    if schema not in KNOWN_SCHEMAS:
        err(
            f"registry.json: unknown schema_version {schema!r} "
            f"(this validator knows: {', '.join(sorted(KNOWN_SCHEMAS))})"
        )

    modules = data.get("modules")
    if not isinstance(modules, list) or not modules:
        err("registry.json.modules: must be a non-empty array")
        modules = []

    seen: dict[str, int] = {}
    known_ids = set()

    for idx, entry in enumerate(modules):
        where = f"registry.json.modules[{idx}]"
        if not isinstance(entry, dict):
            err(f"{where}: must be an object")
            continue

        for field in ("id", "name", "description", "author", "version", "manifest"):
            if not entry.get(field):
                err(f"{where}: missing required field {field!r}")

        mid = entry.get("id", "")
        check_module_id(mid, f"{where}.id")
        if mid in seen:
            err(f"{where}: duplicate module id {mid!r} (also at index {seen[mid]})")
        elif mid:
            seen[mid] = idx
            known_ids.add(mid)

        check_semver(entry.get("version", ""), f"{where}.version")

        manifest_rel = entry.get("manifest", "")
        if manifest_rel and not REPO_PATH.match(manifest_rel):
            err(f"{where}.manifest: unsafe or malformed path {manifest_rel!r}")
        else:
            check_manifest(ROOT / manifest_rel, mid)

        mlhsm = entry.get("mlhsm")
        if not isinstance(mlhsm, dict):
            err(f"{where}.mlhsm: missing or not an object")
        else:
            check_semver(mlhsm.get("min_version", ""), f"{where}.mlhsm.min_version")

        rel = entry.get("release")
        if isinstance(rel, dict):
            url = rel.get("url")
            if url and not url.startswith("https://"):
                err(f"{where}.release.url: must be absolute https, got {url!r}")
            if rel.get("asset") and not rel.get("sha256"):
                err(f"{where}.release: asset without sha256 - CI must not allow this")
            if rel.get("sha256") and not SHA256.match(rel["sha256"]):
                err(f"{where}.release.sha256: must be 64 lowercase hex chars")

    # every hard dependency must exist in the catalog
    for idx, entry in enumerate(modules):
        if not isinstance(entry, dict):
            continue
        rel = entry.get("manifest", "")
        if not rel or not REPO_PATH.match(rel):
            continue
        mpath = ROOT / rel
        if not mpath.is_file():
            continue
        try:
            with mpath.open(encoding="utf-8") as fh:
                mdata = json.load(fh)
        except (json.JSONDecodeError, OSError):
            continue
        for dep in mdata.get("dependencies", []):
            dep_id = dep.get("id") if isinstance(dep, dict) else None
            if dep_id and dep_id not in known_ids:
                err(
                    f"{rel}: depends on {dep_id!r}, which is not in the catalog"
                )

    if errors:
        print(f"registry validation FAILED ({len(errors)} problem(s))", file=sys.stderr)
        for e in errors:
            print(f"  error: {e}", file=sys.stderr)
        return 1

    print(f"registry OK: {len(modules)} module(s), schema {schema}")
    for entry in modules:
        print(f"  - {entry['id']} {entry['version']} ({entry['manifest']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())

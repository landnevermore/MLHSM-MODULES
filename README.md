# MLHSM-MODULES

Module registry for [MLHSM](https://github.com/landnevermore/HomeServerManager).

This repository is the **source of truth for what modules exist** — not for what
is installed. Those are deliberately different things:

```
MLHSM-MODULES                      an MLHSM machine
─────────────────────────────      ─────────────────────
available modules                  installed modules
metadata                           enabled / disabled
versions                           installed version
manifests                          local state
releases                           configuration
```

Machine A may have MLGSM and ModelMesh enabled, machine B only ModelMesh, and a
third machine neither. They all read the same catalog, and the state stays on
each machine. **Nothing in this repository records what a machine has installed** —
that would be wrong the moment a second machine exists.

---

## Layout

```
registry.json              the catalog: one entry per module
schema/
  registry.schema.json     JSON Schema for registry.json
  manifest.schema.json     JSON Schema for a module manifest
modules/<id>/
  module.json              the module's own manifest
  CHANGELOG.md             optional
scripts/
  validate_registry.py     the validator, usable locally and run by CI
.github/workflows/
  validate-registry.yml    runs the validator on every relevant change
```

## Current modules

| Module | Version | Description |
|---|---|---|
| **MLGSM** | 0.1.0 | Container-based game servers: eggs, templates, console, files, backups |
| **ModelMesh** | 0.1.0 | Free-only AI routing across OpenRouter, Ollama, Groq and Google AI |

Both are currently **in-process**: compiled into the MLHSM binary, enabled or
disabled at runtime. Neither has a published release artifact yet, so their
entries carry no `release` block — see [Status](#status-what-works-and-what-does-not).

## Adding a module

You do not need to touch the MLHSM core.

1. **Fork this repository.**
2. **Create `modules/<your-id>/module.json`** following
   [`schema/manifest.schema.json`](schema/manifest.schema.json). Required:
   `id`, `name`, `description`, `author`, `version`, and an `mlhsm` block with
   `min_version`.
3. **Add an entry to `registry.json`** with at least `id`, `name`,
   `description`, `author`, `version`, `manifest` and `mlhsm.min_version`.
4. **Validate locally:**
   ```bash
   python scripts/validate_registry.py
   ```
5. **Open a pull request.** CI runs the same validator and fails on a schema
   violation, a duplicate id, a malformed version, a missing required field, an
   unknown permission, a dependency that does not exist, or a `route_prefixes`
   entry that claims a Core route. It also fails if a referenced file is not in
   the repository. Running it locally first still saves a round trip:
   ```bash
   python scripts/validate_registry.py
   ```
6. **Maintainer review**, then merge. The module appears in the MLHSM module
   browser on the next registry refresh.

## What the validator rejects

Run `python scripts/validate_registry.py` to see the full list. The ones that
bite most often:

| Rule | Example that fails |
|---|---|
| `schema_version` must be known | `mlhsm-registry/2` |
| `id` must match the manifest | catalog says `foo`, manifest says `bar` |
| ids must be unique | two entries both `mlgsm` |
| `version` must be SemVer | `0.1` — needs `0.1.0` |
| `min_version` ≤ `max_version` | `min 0.2.0`, `max 0.1.0` |
| permissions come from a closed list | `"root"` is not a permission |
| `route_prefixes` may not claim Core | `/api/v1/websites` |
| `artifact.url` must be absolute https | `http://…` or a relative path |
| a release asset needs a `sha256` | asset present, digest missing |
| hard dependencies must exist in the catalog | depends on a module that is not listed |

The Core-route list in the validator is copied from `core_routes()` in
`hsm-core/src/module.rs`. If you add a Core route in MLHSM, update the
validator too — otherwise a module could claim a route the host now owns.

## Permissions

A permission is a **declaration**, not a grant:

```json
"permissions": [
  { "id": "docker", "description": "…", "required": true }
]
```

The vocabulary is closed on purpose — an unknown permission fails CI, so a typo
cannot quietly become a new capability. The current set: `docker`, `network`,
`filesystem`, `credentials`, `notifications`, `system`.

**In this phase nothing is actually granted from this file.** Both current
modules are in-process, so their permissions document what they do rather than
requesting access. A future out-of-process runtime is where these become a
consent prompt.

## Security

Registry contents are **untrusted input**. A pull request here proposes
metadata; it does not grant anything. Specifically:

- The MLHSM core never executes a command, path or URL taken from this
  repository.
- Artifacts are not fetched yet. When they are, an `sha256` is required first.
- `permissions` are shown to the operator, not acted on automatically.
- No secrets belong in this repository. It holds public metadata only.

## Status: what works and what does not

Honest state of the pipeline, so nobody is surprised:

| | |
|---|---|
| Catalog + manifests + schema | ✅ in this repository |
| CI validation on pull requests | ✅ active — [run history](https://github.com/landnevermore/MLHSM-MODULES/actions/workflows/validate-registry.yml) |
| MLHSM reads the catalog | ✅ in MLHSM |
| Module browser in the UI | ✅ in MLHSM |
| Caching with fallback when GitHub is down | ✅ in MLHSM |
| **Downloading a module artifact** | ❌ **not implemented** |
| **Running an out-of-process module** | ❌ **not implemented** |

The last two are deliberately out of scope for now. MLGSM and ModelMesh are
compiled into the MLHSM binary today; this registry describes them, installs
nothing, and runs nothing. Do not read a green CI badge as "this module is
installable".

### The workflow was late, and that is on the record

`.github/workflows/validate-registry.yml` sat unpushed for weeks because the
local `gh` token lacked the `workflow` scope. While it was missing, this
README said **⚠️ not active** rather than showing a green CI badge that did not
exist — an audit of the schema passing is worth nothing if the audit itself was
not running. The workflow is active now (`2572079`, first green run
`36749519838`).

It triggers on any change to `registry.json`, `modules/`, `schema/`, `scripts/`
or the workflow itself, plus pushes to `main`, plus manual `workflow_dispatch`.
Beyond what the validator checks on its own it also verifies that every path
referenced in `registry.json` exists on disk — a `manifest` pointing at a file
that was never committed is the most common way to break this repository, and
the validator cannot catch it because it trusts the entry it was given.

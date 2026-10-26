# Social Uploader (MLHSM external module)

A self-hosted publishing queue service packaged and released from the central `MLHSM-MODULES` repository. The MLHSM host supervises the process; this service stores queue state under its module data directory.

## Current implementation

- Durable JSON-backed job store with atomic replacement and restrictive file permissions.
- Project-scoped tokens and per-project account allowlists; tokens and target accounts cannot cross project boundaries.
- Publishing job snapshots include project, platform, target account, media reference, payload snapshot and idempotency key.
- Duplicate requests with the same project + idempotency key return the existing job.
- If the process restarts while a job is marked `publishing`, its status becomes `unknown`; it is not blindly retried.
- Binds only to loopback and exposes a health endpoint.
- **Publishing adapters are not implemented yet.** The API deliberately returns `501 adapter_unavailable` for publish requests; it never pretends a post was sent.

## Configuration

- `MLHSM_MODULE_PORT`: required loopback port provided by MLHSM.
- `SOCIAL_UPLOADER_PROJECT_TOKENS`: required JSON object mapping project IDs to unique tokens. Each token must contain at least 32 characters. Keep tokens in host secret configuration; never commit real tokens.
- `SOCIAL_UPLOADER_PROJECT_ACCOUNTS`: required JSON object mapping project IDs to the exact allowed account IDs for that project.
- `SOCIAL_UPLOADER_DATA_FILE`: optional queue-store path. Defaults to `data/modules/social-uploader/jobs.json`.

Example token map for local development only:

```json
{"rapitup":"replace-with-a-random-token-of-at-least-32-characters","lyricamundo":"replace-with-a-different-random-token-of-at-least-32-characters"}
```

## API

All project endpoints require `X-Social-Uploader-Project-Token: <project-token>`. MLHSM's proxy strips standard `Authorization` and `Cookie` headers, so the module uses a dedicated header.

- `GET /health`
- `POST /api/v1/social-uploader/projects/{project}/jobs`
- `GET /api/v1/social-uploader/projects/{project}/jobs`
- `GET /api/v1/social-uploader/projects/{project}/jobs/{job-id}`
- `POST /api/v1/social-uploader/projects/{project}/jobs/{job-id}:publish` — currently returns `501 adapter_unavailable`.

Example create request:

```json
{
  "platform": "tiktok",
  "account_id": "lyricamundo-main",
  "media_ref": "media-asset-123",
  "payload": {"caption":"#lyrics #music"},
  "idempotency_key": "lyricamundo-2026-10-26-post-01"
}
```

The service checks `account_id` against the configured project allowlist. The allowlist is the boundary between Rap It Up, LYRICAMUNDO and other projects; platform ownership still needs to be verified when a real platform adapter is connected.

## Release

The central release workflow builds `modules/social-uploader` for Linux x86_64 from a `module-social-uploader-v<semver>` tag, creates a GitHub Release archive and updates registry metadata. No live MLHSM host is modified by a GitHub release.

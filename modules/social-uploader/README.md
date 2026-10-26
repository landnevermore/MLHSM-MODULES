# Social Uploader (MLHSM external module)

A self-hosted publishing queue service packaged and released from the central `MLHSM-MODULES` repository. The MLHSM host supervises the process; this service stores queue state under its module data directory.

## Current implementation

- Durable JSON-backed job store with atomic replacement and restrictive file permissions.
- Project-scoped bearer tokens; a token for one project cannot access another project's jobs.
- Publishing job snapshots include project, platform, target account, media reference, payload snapshot and idempotency key.
- Duplicate requests with the same project + idempotency key return the existing job.
- If the process restarts while a job is marked `publishing`, its status becomes `unknown`; it is not blindly retried.
- Binds only to loopback and exposes a health endpoint.
- **Publishing adapters are not implemented yet.** The API deliberately returns `501 adapter_unavailable` for publish requests; it never pretends a post was sent.

## Configuration

- `MLHSM_MODULE_PORT`: required loopback port provided by MLHSM.
- `SOCIAL_UPLOADER_PROJECT_TOKENS`: required JSON object mapping project IDs to unique bearer tokens. Each token must contain at least 32 characters. Keep tokens in the host's secret/credential configuration; never commit real tokens.
- `SOCIAL_UPLOADER_DATA_FILE`: optional queue-store path. Defaults to `data/modules/social-uploader/jobs.json`.

Example token map for local development only:

```json
{"rapitup":"replace-with-a-random-token-of-at-least-32-characters","lyricamundo":"replace-with-a-different-random-token-of-at-least-32-characters"}
```

## API

All project endpoints require `Authorization: Bearer <project-token>`.

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

The caller must ensure that `account_id` belongs to the project whose token is used. The service enforces project-level data access, but does not yet maintain an account registry or independently validate platform ownership.

## Release

The central release workflow builds `modules/social-uploader` for Linux x86_64 from a `module-social-uploader-v<semver>` tag, creates a GitHub Release archive and updates registry metadata. No live MLHSM host is modified by a GitHub release.

# Social Uploader (MLHSM external module)

A self-hosted publishing queue service packaged from the central `MLHSM-MODULES` repository. The Go service exposes the MLHSM module API; TikTok publishing is delegated to a local Python worker using Camoufox with the Playwright API.

## Current capabilities

- Durable JSON-backed queue with atomic file replacement and restrictive file permissions.
- Project-scoped tokens and per-project account allowlists; tokens and target accounts cannot cross project boundaries.
- Idempotent job creation and an atomic `queued -> publishing` claim so the same job cannot start two browser sessions.
- A visible Camoufox/Firefox browser profile per project/account, persisted under the module data directory.
- TikTok Studio upload flow for local video files and captions.
- Explicit status handling: `succeeded` only after an observed success message; uncertain post-submit outcomes become `unknown` and are never retried automatically.
- If the process restarts while a job is `publishing`, its status becomes `unknown`.
- The service binds only to loopback.

This is an initial implementation, not yet validated against a live TikTok account. TikTok's web UI can change and the worker deliberately fails closed if it cannot identify the upload controls or confirm publication. Login and security challenges must be completed by the operator in the visible browser; the worker does not bypass CAPTCHA or login controls.

## Runtime setup

Requirements on the MLHSM host:

- Go-built `social-uploader` executable.
- Python 3.10+.
- Python dependency: `camoufox` (which provides the compatible Playwright API).
- The Camoufox browser build installed for the same Python environment.

Install the worker dependencies from this directory:

```powershell
python -m pip install -r requirements.txt
python -m camoufox fetch
```

On first publish, a visible browser opens for the configured account profile. If login or verification is requested, complete it manually. Separate profiles are created per project and account.

## Configuration

- `MLHSM_MODULE_PORT`: required loopback port provided by MLHSM.
- `SOCIAL_UPLOADER_PROJECT_TOKENS`: required JSON object mapping project IDs to unique tokens, each at least 32 characters. Keep tokens in host secret configuration; never commit real tokens.
- `SOCIAL_UPLOADER_PROJECT_ACCOUNTS`: required JSON object mapping project IDs to exact allowed account IDs.
- `SOCIAL_UPLOADER_DATA_FILE`: optional queue-store path. Defaults to `data/modules/social-uploader/jobs.json`.
- `SOCIAL_UPLOADER_MEDIA_ROOT`: directory holding video files; defaults to `data/modules/social-uploader/media`.
- `SOCIAL_UPLOADER_PROFILE_ROOT`: root for persistent browser profiles; defaults to `data/modules/social-uploader/profiles`.
- `SOCIAL_UPLOADER_TIKTOK_WORKER`: optional path to `tiktok_worker.py`.
- `SOCIAL_UPLOADER_PYTHON`: optional Python executable. Defaults to `python` on Windows and `python3` elsewhere.
- `SOCIAL_UPLOADER_TIKTOK_HEADLESS`: set to `true` only if a headless workflow is intentionally desired; visible mode is the default.
- `SOCIAL_UPLOADER_TIKTOK_TIMEOUT_MS`: Browser operation timeout; defaults to `120000`.

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
- `POST /api/v1/social-uploader/projects/{project}/jobs/{job-id}:publish` — returns `202 Accepted` after atomically starting a TikTok job. Poll the job endpoint for its final status.

Example create request (the media file must already exist under the configured media root, and `media_ref` is its filename):

```json
{
  "platform": "tiktok",
  "account_id": "lyricamundo-main",
  "media_ref": "lyrics-video-001.mp4",
  "payload": {"caption": "#lyrics #music"},
  "idempotency_key": "lyricamundo-2026-10-26-post-01"
}
```

The service checks `account_id` against the configured project allowlist. Profile paths are partitioned by project and account. This is a key application-level boundary, but the external process is not an OS sandbox; host permissions should restrict its data directory to the module.

## Status semantics

- `queued`: persisted and ready to publish.
- `publishing`: a browser worker is active.
- `succeeded`: the worker observed an explicit success message in TikTok Studio.
- `failed`: a known pre-submit or upload setup error occurred.
- `unknown`: the publish click may have succeeded but confirmation was not observed. Check TikTok Studio manually before deciding what to do next.

Never automatically retry `unknown` jobs: that can create duplicate posts.

## Known limitations

- The upload selectors and success messages are based on TikTok Studio's current web UI and require maintenance when the UI changes.
- This initial adapter supports video + caption only. Scheduling, cover selection, sound selection, analytics and deletion are not implemented.
- No live-account smoke test has been run yet. The module must be tested on the Windows MLHSM host before production use.
- Publishing adapters for Instagram, YouTube and other platforms are not implemented.

## Release

The central release workflow currently packages the module according to its release target. A release does not modify a live MLHSM host.

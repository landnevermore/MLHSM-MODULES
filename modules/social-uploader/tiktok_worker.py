#!/usr/bin/env python3
"""Local TikTok Studio browser worker for the MLHSM Social Uploader.

The worker intentionally uses a visible browser by default. If TikTok requests
login, verification, or CAPTCHA, the operator completes it in that browser.
It never tries to solve or bypass platform security challenges.
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import re
import sys
import time
from pathlib import Path

from playwright.sync_api import TimeoutError as PlaywrightTimeout
from playwright.sync_api import sync_playwright

UPLOAD_URL = "https://www.tiktok.com/tiktokstudio/upload"
SUCCESS_RE = re.compile(
    r"(video uploaded|video has been uploaded|posted successfully|your video is live|published successfully)",
    re.IGNORECASE,
)
CHALLENGE_RE = re.compile(
    r"(captcha|verify to continue|security verification|unusual activity|confirm you are human)",
    re.IGNORECASE,
)
LOGIN_RE = re.compile(r"(log in|sign in|continue with)", re.IGNORECASE)
ALLOWED_VIDEO_SUFFIXES = {".mp4", ".mov", ".webm", ".m4v"}


def emit(status: str, error_code: str = "") -> None:
    print(json.dumps({"status": status, "error_code": error_code}), flush=True)


def set_caption(page, caption: str) -> None:
    selectors = [
        '[contenteditable="true"][data-text="true"]',
        '[contenteditable="true"]',
        'textarea[placeholder*="caption" i]',
        'textarea',
    ]
    for selector in selectors:
        loc = page.locator(selector).first
        try:
            if loc.count() and loc.is_visible():
                loc.click(timeout=3000)
                loc.fill(caption, timeout=5000)
                return
        except Exception:
            continue
    raise RuntimeError("caption_field_not_found")


def wait_for_upload_form(page, timeout_seconds: int = 300) -> bool:
    """Wait for the operator to finish login/verification in the visible browser."""
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        try:
            if page.locator('input[type="file"]').count() > 0:
                return True
        except Exception:
            pass
        # Challenge handling is deliberately manual. Keep the browser visible.
        time.sleep(2)
    return False


def run(args: argparse.Namespace) -> int:
    video = Path(args.media).resolve()
    profile = Path(args.profile).resolve()
    if not video.is_file():
        emit("failed", "media_not_found")
        return 1
    if video.suffix.lower() not in ALLOWED_VIDEO_SUFFIXES:
        emit("failed", "unsupported_media_type")
        return 1
    if not args.caption.strip():
        emit("failed", "caption_required")
        return 1
    profile.mkdir(parents=True, exist_ok=True)

    headless = os.getenv("SOCIAL_UPLOADER_TIKTOK_HEADLESS", "").lower() in {"1", "true", "yes"}
    timeout_ms = int(os.getenv("SOCIAL_UPLOADER_TIKTOK_TIMEOUT_MS", "120000"))
    logging.basicConfig(level=logging.WARNING, stream=sys.stderr)

    publish_clicked = False
    try:
        with sync_playwright() as p:
            context = p.chromium.launch_persistent_context(
                user_data_dir=str(profile),
                headless=headless,
                viewport={"width": 1440, "height": 1000},
                locale="en-US",
                accept_downloads=False,
            )
            try:
                page = context.pages[0] if context.pages else context.new_page()
                page.set_default_timeout(timeout_ms)
                page.goto(UPLOAD_URL, wait_until="domcontentloaded", timeout=60000)

                if not wait_for_upload_form(page, timeout_seconds=args.login_wait):
                    emit("failed", "login_or_verification_timeout")
                    return 1

                file_input = page.locator('input[type="file"]').first
                file_input.set_input_files(str(video), timeout=30000)

                # Wait until upload/processing is no longer visibly in progress.
                progress = page.locator('[role="progressbar"]')
                try:
                    progress.first.wait_for(state="hidden", timeout=180000)
                except PlaywrightTimeout:
                    # TikTok often removes progress indicators rather than hiding them.
                    pass

                set_caption(page, args.caption)

                # Only click an unambiguous publish action. Do not guess when UI labels
                # have changed; return a clear error so the UI can be updated safely.
                candidates = [
                    page.get_by_role("button", name=re.compile(r"^(post|publish)$", re.I)),
                    page.locator('button[data-e2e="post_video_button"]'),
                ]
                publish_button = None
                for candidate in candidates:
                    try:
                        loc = candidate.first
                        if loc.count() and loc.is_visible() and loc.is_enabled():
                            publish_button = loc
                            break
                    except Exception:
                        continue
                if publish_button is None:
                    emit("failed", "publish_button_not_found")
                    return 1

                # From this point onward, a timeout is ambiguous: the click may have
                # reached TikTok even if Playwright does not receive a response.
                publish_clicked = True
                publish_button.click(timeout=10000)

                # Publication may take a while; only mark success after an explicit
                # UI confirmation. Otherwise the caller must reconcile manually.
                deadline = time.monotonic() + args.publish_wait
                while time.monotonic() < deadline:
                    try:
                        body_text = page.locator("body").inner_text(timeout=3000)
                        if SUCCESS_RE.search(body_text):
                            emit("succeeded")
                            return 0
                        if CHALLENGE_RE.search(body_text):
                            time.sleep(3)
                            continue
                    except Exception:
                        pass
                    time.sleep(2)

                emit("unknown", "publish_confirmation_not_observed")
                return 2
            finally:
                context.close()
    except Exception as exc:
        # Do not print exception strings: browser errors can contain sensitive URLs.
        emit("unknown" if publish_clicked else "failed",
             "browser_error_after_submit" if publish_clicked else "browser_start_or_upload_error")
        return 2 if publish_clicked else 1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--media", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--caption", required=True)
    parser.add_argument("--login-wait", type=int, default=300)
    parser.add_argument("--publish-wait", type=int, default=180)
    args = parser.parse_args()
    return run(args)


if __name__ == "__main__":
    raise SystemExit(main())

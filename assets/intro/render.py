# /// script
# requires-python = ">=3.10"
# dependencies = ["playwright"]
# ///
"""Render assets/intro/annox-intro.html to a video, one frame at a time.

Each frame seeks the page's timeline with annoxSeek() and takes a screenshot,
so the video is smooth however long a frame takes to render. Frames are piped
straight into ffmpeg.

    uv run assets/intro/render.py OUT [--theme light|dark] [--fps 30] [--width 1920]
"""

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

PAGE = Path(__file__).with_name("annox-intro.html").resolve()


def browser_path() -> str | None:
    """Prefer a system Chromium, so Playwright needn't download its own."""
    for name in ("chromium", "chromium-browser", "google-chrome-stable", "google-chrome"):
        if path := shutil.which(name):
            return path
    return None


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--theme", choices=("light", "dark"), default="light")
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--width", type=int, default=1920)
    args = ap.parse_args()
    width, height = args.width, args.width * 9 // 16
    args.out.parent.mkdir(parents=True, exist_ok=True)

    if args.out.suffix == ".webm":
        codec = ["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "30", "-row-mt", "1"]
    elif args.out.suffix == ".gif":
        codec = ["-vf", "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none"]
    else:
        codec = ["-c:v", "libx264", "-crf", "18", "-preset", "slow", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]

    ffmpeg = subprocess.Popen(
        ["ffmpeg", "-y", "-loglevel", "error", "-f", "image2pipe", "-framerate", str(args.fps), "-i", "-", *codec, str(args.out)],
        stdin=subprocess.PIPE,
    )
    assert ffmpeg.stdin

    with sync_playwright() as p:
        browser = p.chromium.launch(executable_path=browser_path())
        page = browser.new_page(viewport={"width": width, "height": height}, color_scheme=args.theme)
        page.goto(f"{PAGE.as_uri()}?render")
        page.evaluate("document.fonts.ready")
        duration = page.evaluate("window.annoxDuration")
        frames = int(duration * args.fps)
        for i in range(frames):
            page.evaluate("t => window.annoxSeek(t)", i / args.fps)
            ffmpeg.stdin.write(page.screenshot(type="png"))
            print(f"\rframe {i + 1}/{frames}", end="", file=sys.stderr, flush=True)
        print(file=sys.stderr)
        browser.close()

    ffmpeg.stdin.close()
    if ffmpeg.wait() != 0:
        sys.exit("ffmpeg failed")
    print(args.out)


if __name__ == "__main__":
    main()

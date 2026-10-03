#!/usr/bin/env python3
"""Decodes the two exports scripts/engine-export-parity.mjs --formats wrote of one format (the engine's and the
JavaScript renderer's) and compares them frame by frame.

    python3 scripts/engine-export-compare.py <folder> [format...]   (formats: png png-sequence gif apng mp4 webm)

Needs Pillow and NumPy (independent of the app's decoders); videos are decoded by the bundled FFmpeg
(build/video-encoder/ffmpeg) into RGBA. Per format: frames, the mean of the per-frame mean |delta| (largest channel,
alpha included), the largest such mean, the share of pixels beyond 8 and 32 levels, and the largest difference.
Prints one JSON line per format and writes compare.json beside the exports.
"""
import io
import json
import os
import subprocess
import sys
import zipfile

import numpy as np
from PIL import Image, ImageSequence

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FFMPEG = os.path.join(ROOT, "build", "video-encoder", "ffmpeg.exe" if os.name == "nt" else "ffmpeg")
EXTENSION = {"png": "png", "png-sequence": "zip", "gif": "gif", "apng": "png", "mp4": "mp4", "webm": "webm"}


def rgba(image):
    return np.asarray(image.convert("RGBA"), dtype=np.int16)


def zip_frames(path):
    with zipfile.ZipFile(path) as archive:
        for name in sorted(n for n in archive.namelist() if n.lower().endswith(".png")):
            yield rgba(Image.open(io.BytesIO(archive.read(name))))


def animation_frames(path):
    with Image.open(path) as image:
        for frame in ImageSequence.Iterator(image):
            yield rgba(frame)


def video_frames(path):
    probe = subprocess.run([FFMPEG, "-hide_banner", "-i", path], capture_output=True, text=True).stderr
    size = next(token for line in probe.splitlines() if "Video:" in line for token in line.replace(",", " ").split() if "x" in token and token.split("x")[0].isdigit() and token.split("x")[1].isdigit())
    width, height = (int(v) for v in size.split("x"))
    # libvpx decodes the alpha of VP9 WebM; FFmpeg's own decoder (no alpha) where libvpx refuses the stream.
    decoder = ["-c:v", "libvpx-vp9"] if path.endswith(".webm") and subprocess.run([FFMPEG, "-hide_banner", "-loglevel", "error", "-c:v", "libvpx-vp9", "-i", path, "-frames:v", "1", "-f", "null", "-"], capture_output=True).returncode == 0 else []
    process = subprocess.Popen([FFMPEG, "-hide_banner", "-loglevel", "error", *decoder, "-i", path, "-f", "rawvideo", "-pix_fmt", "rgba", "-"], stdout=subprocess.PIPE)
    frame = width * height * 4
    while True:
        data = process.stdout.read(frame)
        if len(data) < frame:
            break
        yield np.frombuffer(data, dtype=np.uint8).reshape(height, width, 4).astype(np.int16)
    process.wait()


def frames(path, fmt):
    if fmt == "png-sequence":
        return zip_frames(path)
    if fmt in ("png", "gif", "apng"):
        return animation_frames(path)
    return video_frames(path)


def compare(folder, fmt):
    engine = os.path.join(folder, f"{fmt}-engine.{EXTENSION[fmt]}")
    javascript = os.path.join(folder, f"{fmt}-javascript.{EXTENSION[fmt]}")
    if not (os.path.exists(engine) and os.path.exists(javascript)):
        return {"format": fmt, "missing": True}
    means, beyond8, beyond32, largest, count = [], [], [], 0, 0
    for a, b in zip(frames(engine, fmt), frames(javascript, fmt)):
        if a.shape != b.shape:
            return {"format": fmt, "error": f"frame sizes differ: {a.shape} and {b.shape}"}
        d = np.abs(a - b).max(axis=2)
        means.append(float(d.mean()))
        beyond8.append(float((d > 8).mean() * 100))
        beyond32.append(float((d > 32).mean() * 100))
        largest = max(largest, int(d.max()))
        count += 1
    if not count:
        return {"format": fmt, "error": "no frames decoded"}
    return {"format": fmt, "frames": count, "mean": round(sum(means) / count, 3), "worstFrameMean": round(max(means), 3),
            "beyond8": round(sum(beyond8) / count, 3), "beyond32": round(sum(beyond32) / count, 3), "largest": largest}


def main():
    folder = sys.argv[1]
    formats = sys.argv[2:] or list(EXTENSION)
    results = []
    for fmt in formats:
        result = compare(folder, fmt)
        results.append(result)
        print(json.dumps(result))
    with open(os.path.join(folder, "compare.json"), "w") as out:
        json.dump(results, out, indent=2)


if __name__ == "__main__":
    main()

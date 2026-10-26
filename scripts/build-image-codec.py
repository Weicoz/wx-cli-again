#!/usr/bin/env python3
"""构建随 wx 携带的最小 LGPL FFmpeg；不查找或复用 PATH 中的 ffmpeg。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

VERSION = "8.0.1"
SOURCE_SHA256 = "05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41"
SOURCE_URL = f"https://ffmpeg.org/releases/ffmpeg-{VERSION}.tar.xz"
FLAGS = ["--disable-everything", "--disable-autodetect", "--disable-network",
         "--disable-doc", "--disable-debug", "--disable-shared", "--enable-static",
         "--disable-x86asm", "--enable-ffmpeg", "--enable-protocol=pipe",
         "--enable-demuxer=hevc", "--enable-parser=hevc", "--enable-decoder=hevc",
         "--enable-encoder=bmp", "--enable-muxer=image2pipe", "--enable-filter=format,scale",
         "--enable-swscale"]

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--install-dir", type=Path, required=True)
    p.add_argument("--target", default="native", choices=["native", "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc"])
    p.add_argument("--work-dir", type=Path, default=Path("target/image-codec-build"))
    args = p.parse_args()
    work = args.work_dir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    archive = work / f"ffmpeg-{VERSION}.tar.xz"
    if not archive.exists():
        with urllib.request.urlopen(SOURCE_URL, timeout=60) as r:
            archive.write_bytes(r.read())
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SOURCE_SHA256:
        raise SystemExit("FFmpeg source checksum mismatch")
    source = work / f"ffmpeg-{VERSION}"
    if not source.exists():
        with tarfile.open(archive) as t:
            # 官方归档亦只允许普通相对路径，不允许符号链接逃逸。
            for m in t.getmembers():
                if not (work / m.name).resolve().is_relative_to(work) or m.issym() or m.islnk():
                    raise SystemExit("unsafe source archive member")
            t.extractall(work)
    flags = list(FLAGS)
    if args.target.endswith("apple-darwin"):
        arch = "arm64" if args.target.startswith("aarch64") else "x86_64"
        flags += ["--enable-cross-compile", "--target-os=darwin", f"--arch={arch}",
                  "--cc=clang", f"--extra-cflags=-arch {arch}", f"--extra-ldflags=-arch {arch}"]
    elif args.target == "aarch64-unknown-linux-gnu":
        flags += ["--enable-cross-compile", "--arch=aarch64", "--target-os=linux", "--cross-prefix=aarch64-linux-gnu-"]
    elif args.target == "x86_64-pc-windows-msvc":
        flags += ["--target-os=mingw32", "--arch=x86_64", "--cc=gcc", "--disable-pthreads", "--enable-w32threads", "--extra-ldflags=-static"]
    stamp = source / "image-codec-configure.json"
    if not stamp.exists() or json.loads(stamp.read_text()) != flags:
        if (source / "ffbuild/config.mak").exists():
            subprocess.run(["make", "distclean"], cwd=source, check=True)
        subprocess.run(["sh", "configure", *flags], cwd=source, check=True)
        stamp.write_text(json.dumps(flags))
    subprocess.run(["make", f"-j{min(os.cpu_count() or 2, 8)}", "ffmpeg"], cwd=source, check=True)
    binary = source / ("ffmpeg.exe" if (source / "ffmpeg.exe").exists() else "ffmpeg")
    out = args.install_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    target = out / binary.name
    shutil.copy2(binary, target)
    target.chmod(0o755)
    # 附完整对应源代码、许可和构建参数，交付包可离线重建。
    shutil.copy2(archive, out / archive.name)
    for license_name in ["COPYING.LGPLv2.1", "COPYING.LGPLv3", "LICENSE.md"]:
        shutil.copy2(source / license_name, out / license_name)
    manifest = {"version": VERSION, "sourceSha256": SOURCE_SHA256,
                "binarySha256": hashlib.sha256(target.read_bytes()).hexdigest(),
                "sourceUrl": SOURCE_URL, "configure": flags}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"image codec {VERSION} installed: {out}")

if __name__ == "__main__":
    main()

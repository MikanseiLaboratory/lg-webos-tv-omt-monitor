#!/usr/bin/env python3
"""Build a webOS IPK for the OMT monitor.

The layout mirrors webOS Homebrew IPKs (Debian-style ar archive with
debian-binary / control.tar.gz / data.tar.gz).

usage:
  python3 build-ipk.py --bin <path/to/omt-monitor> [--out omt-monitor.ipk]
"""

import argparse
import gzip
import io
import os
import tarfile
import time

APP_ID = "org.mikansei.omt-monitor"
VERSION = "0.1.0"
ARCH = "arm"  # webOS uses "arm" for native packages, including aarch64

HERE = os.path.dirname(os.path.abspath(__file__))


def add_file(tar: tarfile.TarFile, name: str, data: bytes, mode: int = 0o644):
    ti = tarfile.TarInfo(name)
    ti.size = len(data)
    ti.mode = mode
    ti.mtime = int(time.time())
    ti.uid = 0
    ti.gid = 0
    ti.uname = "root"
    ti.gname = "root"
    tar.addfile(ti, io.BytesIO(data))


def make_tar_gz(entries):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.GNU_FORMAT) as tar:
        for name, data, mode in entries:
            add_file(tar, name, data, mode)
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0) as gz:
        gz.write(raw.getvalue())
    return out.getvalue()


def make_ar(entries):
    out = io.BytesIO()
    out.write(b"!<arch>\n")
    for name, data in entries:
        header = (
            f"{name:<16}"
            f"{int(time.time()):<12}"
            f"{0:<6}{0:<6}"
            f"{'100644':<8}"
            f"{len(data):<10}"
        ).encode("ascii") + b"`\n"
        assert len(header) == 60, len(header)
        out.write(header)
        out.write(data)
        if len(data) % 2:
            out.write(b"\n")
    return out.getvalue()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--out", default=os.path.join(HERE, f"{APP_ID}.ipk"))
    args = ap.parse_args()

    binary = open(args.bin, "rb").read()
    appinfo = open(os.path.join(HERE, "appinfo.json"), "rb").read()
    icon = open(os.path.join(HERE, "icon.png"), "rb").read()

    base = f"./usr/palm/applications/{APP_ID}"
    data_entries = [
        (f"{base}/appinfo.json", appinfo, 0o644),
        (f"{base}/icon.png", icon, 0o644),
        (f"{base}/omt-monitor", binary, 0o755),
    ]

    control = f"""Package: {APP_ID}
Version: {VERSION}
Section: misc
Priority: optional
Architecture: {ARCH}
Installed-Size: {len(binary)}
Maintainer: Mikansei Laboratory <noreply@example.com>
Description: OMT (Open Media Transport) monitor for LG webOS TVs
webOS-Package-Format-Version: 2
webOS-Packager-Version: x.y.x
""".encode()

    control_tar = make_tar_gz([("./control", control, 0o644)])
    data_tar = make_tar_gz(data_entries)

    ipk = make_ar([
        ("debian-binary", b"2.0\n"),
        ("control.tar.gz", control_tar),
        ("data.tar.gz", data_tar),
    ])
    with open(args.out, "wb") as f:
        f.write(ipk)
    print(f"wrote {args.out} ({len(ipk)} bytes)")


if __name__ == "__main__":
    main()

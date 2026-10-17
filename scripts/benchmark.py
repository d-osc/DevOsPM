"""Measure install throughput and list latency using an isolated temporary store."""
import argparse
import io
import json
import os
from pathlib import Path
import statistics
import subprocess
import tarfile
import tempfile
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("--payload-mib", type=int, default=64)
    parser.add_argument("--files", type=int, default=1000)
    args = parser.parse_args()
    if args.payload_mib < 1 or not 1 <= args.files <= 50000:
        parser.error("use --payload-mib >= 1 and --files between 1 and 50000")
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="devos-benchmark-") as tmp:
        base = Path(tmp)
        archive = base / "benchmark.dpk"
        env = dict(os.environ, DEV_ROOT=str(base / "store"))
        # Repeat a random 64 KiB block for a synthetic workload (gzip has a 32 KiB window).
        block = os.urandom(64 * 1024)
        total = args.payload_mib * 1024 * 1024
        sizes = [total // args.files + (i < total % args.files) for i in range(args.files)]
        with tarfile.open(archive, "w:gz", compresslevel=1) as tar:
            manifest = json.dumps({"name": "benchmark", "version": "1"}).encode()
            info = tarfile.TarInfo("package.json")
            info.size = len(manifest)
            tar.addfile(info, io.BytesIO(manifest))
            for index, size in enumerate(sizes):
                data = (block * ((size + len(block) - 1) // len(block)))[:size]
                info = tarfile.TarInfo(f"share/file-{index:05d}")
                info.size = size
                tar.addfile(info, io.BytesIO(data))

        def run(*command):
            subprocess.run([str(binary), *command], env=env, check=True,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=300)

        start = time.perf_counter()
        run("install", str(archive))
        install = time.perf_counter() - start
        start = time.perf_counter()
        run("verify")
        verify = time.perf_counter() - start
        samples = []
        for _ in range(10):
            start = time.perf_counter()
            run("list")
            samples.append((time.perf_counter() - start) * 1000)
        print(json.dumps({
            "workload": "synthetic repeated random 64 KiB block; results depend on CPU/filesystem/cache",
            "payload_mib": args.payload_mib,
            "files": args.files,
            "compressed_mib": round(archive.stat().st_size / 1024**2, 2),
            "install_seconds": round(install, 3),
            "install_mib_per_second": round(args.payload_mib / install, 2),
            "verify_seconds": round(verify, 3),
            "list_median_ms": round(statistics.median(samples), 2),
        }, indent=2))


if __name__ == "__main__":
    main()

"""Exercise the public CLI, including locking across processes, on Linux."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="devos-smoke-") as tmp:
        base = Path(tmp)
        root = base / "store"
        env = dict(os.environ, DEV_ROOT=str(root))
        install_env = dict(env, DEV_BIN_DIR=str(base / "installed-bin"))
        subprocess.run(["sh", str(Path(__file__).with_name("install.sh")), str(binary)],
                       env=install_env, check=True, capture_output=True, text=True, timeout=30)
        subprocess.run([str(base / "installed-bin" / "dev"), "--version"],
                       env=env, check=True, capture_output=True, timeout=30)

        def dev(*args, code=0, cwd=None):
            result = subprocess.run([str(binary), *map(str, args)], env=env,
                                    cwd=cwd, text=True, capture_output=True, timeout=30)
            assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
            return result

        source = base / "source"
        (source / "bin").mkdir(parents=True)
        executable = source / "bin" / "hello"
        executable.write_text('#!/bin/sh\nprintf "%s\\n" "$1"\n'
                              'if [ "$1" = "wait" ]; then touch "$DEV_PACKAGE_ROOT/ready"; sleep 2; fi\n'
                              'if [ "$1" = "exit" ]; then exit 7; fi\n')
        executable.chmod(0o755)
        manifest_path = source / "package.json"
        manifest = {"kind": "app", "name": "hello", "version": "1", "bin": {"hello": "bin/hello"}}
        manifest_path.write_text(json.dumps(manifest))
        first = base / "hello-1.dpk"
        dev("pack", source, cwd=base)
        assert first.is_file(), "default pack output must be <name>-<version>.dpk"
        assert first.read_bytes()[:2] == b"\x1f\x8b", ".dpk must contain gzip data"
        dev("pack", source, cwd=base, code=1)  # Never overwrite an existing package.
        legacy = base / "legacy.tar.gz"
        dev("pack", source, "--output", legacy)
        assert first.read_bytes() == legacy.read_bytes()
        malformed = base / "invalid.dpk"
        malformed.write_bytes(b"not a package")
        dev("install", malformed, code=1)
        digest = hashlib.sha256(first.read_bytes()).hexdigest()
        dev("install", first, "--sha256", "0" * 64, code=1)
        dev("install", first, "--sha256", digest)
        dev("verify")
        assert "spaces and --flags" in dev("run", "hello", "--", "spaces and --flags").stdout
        dev("run", "hello", "--", "exit", code=7)
        state = json.loads(dev("list", "--json").stdout)
        installed = root / "packages" / state["hello"]["active"]["directory"]

        child = subprocess.Popen([str(binary), "run", "hello", "--", "wait"], env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not (installed / "ready").exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            assert (installed / "ready").exists(), "child did not start"
            assert "busy" in dev("remove", "hello", code=1).stderr
            # Concurrent read-only commands remain available during dev run.
            dev("list")
            out, err = child.communicate(timeout=10)
            assert child.returncode == 0, (out, err)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
        (installed / "ready").unlink()

        manifest["version"] = "2"
        manifest_path.write_text(json.dumps(manifest))
        second = base / "hello-v2.tar.gz"
        dev("pack", source, "--output", second)
        dev("install", second, code=1)
        dev("install", second, "--replace")  # Legacy tar.gz can upgrade a dpk install.
        assert json.loads(dev("list", "--json").stdout)["hello"]["active"]["manifest"]["version"] == "2"
        dev("rollback", "hello")
        assert json.loads(dev("list", "--json").stdout)["hello"]["active"]["manifest"]["version"] == "1"
        dev("verify", "hello")
        executable_installed = installed / "bin" / "hello"
        executable_installed.write_text("#!/bin/sh\necho tampered\n")
        dev("verify", "hello", code=1)
        dev("run", "hello", code=1)
        dev("remove", "hello")
        assert json.loads(dev("list", "--json").stdout) == {}
        assert list((root / "packages").iterdir()) == []
        assert list((root / "staging").iterdir()) == []
        print("Linux CLI smoke test passed: binary installer, default dpk pack, tar.gz compatibility, corrupt dpk, checksum, install, run, exit codes, concurrent locks, upgrade, rollback, tampering, remove")


if __name__ == "__main__":
    main()

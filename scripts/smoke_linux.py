"""Exercise the public CLI, including locking across processes, on Linux."""
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time


def build_extension(path, ext_id, version):
    manifest = {
        "id": ext_id, "name": "Smoke Widget", "description": "Smoke test widget",
        "version": version, "apiVersion": 1, "kind": "ui", "system": False,
        "entry": f"{ext_id}.js", "order": 100, "settingsVersion": 1,
        "enabledByDefault": True, "settings": [],
    }
    with tarfile.open(path, "w:gz") as tar:
        def add(name, data, mode=0o644):
            info = tarfile.TarInfo(f"extension/{name}")
            info.size = len(data)
            info.mode = mode
            tar.addfile(info, io.BytesIO(data))
        add("extension.json", json.dumps(manifest, indent=2).encode())
        add(f"dist/{ext_id}.js", b"export default {};\n")


class ReleaseServer:
    """Serves a GitHub-shaped releases/latest API plus asset downloads."""

    def __init__(self, base):
        # A fake dev binary that reports the same version as the listing tag,
        # so dev self-update can verify it before swapping files.
        self.fake_name = "dev-linux-" + ("arm64" if platform.machine() == "aarch64" else "x86_64")
        fake = base / self.fake_name
        fake.write_text("#!/bin/sh\nprintf 'dev 3.0.0\\n'\n")
        fake.chmod(0o755)
        assets = {}
        for name in ["updog-3.0.0.dpk", "dev-os-extension.tar.gz", "extension.json", self.fake_name]:
            data = (base / name).read_bytes()
            assets[name] = data
        tag = "v3.0.0"
        listing = {
            "tag_name": tag, "draft": False, "prerelease": False,
            "assets": [
                {"name": name, "size": len(data), "state": "uploaded",
                 "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                 "browser_download_url": f"http://127.0.0.1:{{PORT}}/asset/{name}"}
                for name, data in assets.items()
            ],
        }
        handler = type("Handler", (http.server.BaseHTTPRequestHandler,), {
            "do_GET": lambda self, listing=listing, assets=assets: self.respond(listing, assets),
            "respond": self.respond,
            "log_message": lambda *args: None,
        })
        self.server = http.server.HTTPServer(("127.0.0.1", 0), handler)
        port = self.server.server_address[1]
        listing["assets"] = [dict(asset, browser_download_url=asset["browser_download_url"]
                                  .replace("{PORT}", str(port)))
                             for asset in listing["assets"]]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def start(self):
        self.thread.start()
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def stop(self):
        self.server.shutdown()

    @staticmethod
    def respond(self, listing, assets):
        if self.path.endswith("/releases/latest"):
            body = json.dumps(listing).encode()
        elif self.path.startswith("/asset/"):
            body = assets[self.path[len("/asset/"):]]
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="devos-smoke-") as tmp:
        base = Path(tmp)
        root = base / "store"
        xdg = base / "xdg"
        env = dict(os.environ, DEV_ROOT=str(root), XDG_DATA_HOME=str(xdg))
        install_env = dict(env, DEV_BIN_DIR=str(base / "installed-bin"))
        subprocess.run(["sh", str(Path(__file__).with_name("install.sh")), str(binary)],
                       env=install_env, check=True, capture_output=True, text=True, timeout=30)
        subprocess.run([str(base / "installed-bin" / "dev"), "--version"],
                       env=env, check=True, capture_output=True, timeout=30)

        def dev(*args, code=0, cwd=None, extra_env=None):
            result = subprocess.run([str(binary), *map(str, args)], env=dict(env, **(extra_env or {})),
                                    cwd=cwd, text=True, capture_output=True, timeout=60)
            assert result.returncode == code, (args, result.returncode, result.stdout, result.stderr)
            return result

        def packages():
            return json.loads(dev("list", "--json").stdout)["packages"]

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

        # Launcher entry appears in the XDG applications directory and follows the package.
        desktop = xdg / "applications" / "dev-hello.desktop"
        assert desktop.is_file(), "install must write a launcher .desktop entry"
        entry = desktop.read_text()
        assert "Type=Application" in entry and " run hello" in entry and "X-DevOS-Package=hello" in entry
        dev("install", first, "--no-desktop", "--sha256", digest, code=1)  # already installed

        assert "spaces and --flags" in dev("run", "hello", "--", "spaces and --flags").stdout
        dev("run", "hello", "--", "exit", code=7)
        state = json.loads(dev("list", "--json").stdout)
        installed = root / "packages" / state["packages"]["hello"]["active"]["directory"]

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
        assert packages()["hello"]["active"]["manifest"]["version"] == "2"
        dev("rollback", "hello")
        assert packages()["hello"]["active"]["manifest"]["version"] == "1"
        dev("verify", "hello")
        executable_installed = installed / "bin" / "hello"
        executable_installed.write_text("#!/bin/sh\necho tampered\n")
        dev("verify", "hello", code=1)
        dev("run", "hello", code=1)
        dev("remove", "hello")
        assert packages() == {}
        assert not desktop.exists(), "remove must delete the launcher entry"
        assert list((root / "packages").iterdir()) == []
        assert list((root / "staging").iterdir()) == []

        # Dev OS extension packages land in the desktop Extension Store layout.
        ext_id = "org.smoke.widget"
        widget_v1 = base / "org.smoke.widget-1.0.0.dpkx"
        build_extension(widget_v1, ext_id, "1.0.0")
        dev("install", widget_v1)
        listing = json.loads(dev("list", "--json").stdout)["extensions"]
        assert ext_id in listing and listing[ext_id]["version"] == "1.0.0"
        link = xdg / "dev-os" / "extensions" / ext_id
        assert link.is_symlink(), "extension symlink must exist"
        record = json.loads((link / "store-install.json").read_text())
        assert record["id"] == ext_id and record["repository"] == "local/cli"
        assert record["key"].startswith("1.0.0-") and len(record["key"].split("-")[-1]) == 12
        assert (link / "dist" / f"{ext_id}.js").is_file()
        assert '"id": "org.smoke.widget"' in dev("info", ext_id).stdout
        dev("install", widget_v1, code=1)  # reinstalling needs --replace
        widget_v2 = base / "widget-v2.tar.gz"
        build_extension(widget_v2, ext_id, "2.0.0")
        dev("install", widget_v2, "--replace")
        assert json.loads(dev("list", "--json").stdout)["extensions"][ext_id]["version"] == "2.0.0"
        assert "1.0.0" in dev("rollback", ext_id).stdout
        dev("verify", ext_id)
        dev("remove", ext_id, "--purge")
        assert not link.exists()
        assert not (xdg / "dev-os" / "extension-store" / "releases" / ext_id).exists()
        assert json.loads(dev("list", "--json").stdout)["extensions"] == {}

        # dev update: query a GitHub-shaped API mirror and install the newer release.
        updog = base / "updog-source"
        (updog / "bin").mkdir(parents=True)
        (updog / "bin" / "updog").write_text('#!/bin/sh\necho updog "$1"\n')
        (updog / "bin" / "updog").chmod(0o755)
        (updog / "package.json").write_text(json.dumps({
            "name": "updog", "version": "1.0.0", "bin": {"updog": "bin/updog"},
            "repository": {"type": "git", "url": "https://github.com/example/tools",
                           "release": {"type": "dpk", "filename": "updog-{version}.dpk"}},
        }))
        updog_v1 = base / "updog-1.0.0.dpk"
        dev("pack", updog, "--output", updog_v1, cwd=base)
        dev("install", updog_v1)
        updog_v3 = base / "updog-3.0.0.dpk"
        (updog / "package.json").write_text(json.dumps({
            "name": "updog", "version": "3.0.0", "bin": {"updog": "bin/updog"},
            "repository": {"type": "git", "url": "https://github.com/example/tools",
                           "release": {"type": "dpk", "filename": "updog-{version}.dpk"}},
        }))
        dev("pack", updog, "--output", updog_v3, cwd=base)
        build_extension(base / "dev-os-extension.tar.gz", "org.smoke.widget", "3.0.0")
        (base / "extension.json").write_text(json.dumps({
            "id": "org.smoke.widget", "name": "Smoke Widget", "description": "smoke",
            "version": "3.0.0", "apiVersion": 1, "kind": "ui", "system": False,
            "entry": "org.smoke.widget.js", "order": 100, "settingsVersion": 1,
            "enabledByDefault": True, "settings": [],
        }))
        server = ReleaseServer(base)
        api = server.start()
        try:
            dev("update", extra_env={"DEV_UPDATE_API": api})
            assert packages()["updog"]["active"]["manifest"]["version"] == "3.0.0"
            assert "updog" in dev("run", "updog", "--", "check").stdout
            # A second run reports everything up to date and changes nothing.
            assert "up to date" in dev("update", extra_env={"DEV_UPDATE_API": api}).stdout
            dev("remove", "updog")
            # dev self-update swaps the running binary with the newer release.
            tool = base / "devtool"
            shutil.copy(binary, tool)
            tool.chmod(0o755)
            result = subprocess.run([str(tool), "self-update"], env=dict(env, DEV_UPDATE_API=api),
                                    text=True, capture_output=True, timeout=60)
            assert result.returncode == 0, (result.stdout, result.stderr)
            assert "Updated dev to 3.0.0" in result.stdout
            replaced = subprocess.run([str(tool), "--version"], capture_output=True, text=True, timeout=30)
            assert replaced.stdout.strip() == "dev 3.0.0"
        finally:
            server.stop()
        print("Linux CLI smoke test passed: binary installer, default dpk pack, tar.gz compatibility, "
              "corrupt dpk, checksum, install, run, exit codes, concurrent locks, upgrade, rollback, "
              "tampering, remove, launcher entries, Dev OS extensions, dev update, dev self-update")


if __name__ == "__main__":
    main()

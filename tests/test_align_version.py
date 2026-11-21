"""在临时工作区以另一版本解析完整依赖图，不修改发布输入，也不编译二进制。"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


class ReleaseAlignment(unittest.TestCase):
    def test_desktop_resolves_the_retagged_ipc(self):
        root = Path(__file__).resolve().parents[1]
        version = "9.8.7-rc.654"
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            shutil.copy2(root / "Cargo.lock", work)
            for manifest in [root / "Cargo.toml", *sorted(root.glob("aproxy-*/Cargo.toml"))]:
                directory = work / manifest.parent.relative_to(root)
                (directory / "src").mkdir(parents=True)
                shutil.copy2(manifest, directory)
                for target in ("lib.rs", "main.rs"):
                    if (manifest.parent / "src" / target).is_file():
                        (directory / "src" / target).touch()
            env = {**os.environ, "GITHUB_EVENT_NAME": "workflow_dispatch", "DRY_RUN_VERSION": version}
            env.pop("GITHUB_ENV", None)
            subprocess.run([sys.executable, root / ".github/scripts/align-version.py"], cwd=work, env=env, check=True)
            host = next(line.split(": ")[1] for line in subprocess.check_output(["rustc", "-vV"], text=True).splitlines() if line.startswith("host:"))
            command = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", host]
            graph = json.loads(subprocess.check_output(command, cwd=work, env=env))
            packages = {p["name"]: p for p in graph["packages"] if p["id"] in graph["workspace_members"]}
            nodes = {n["id"]: n for n in graph["resolve"]["nodes"]}
            self.assertEqual(packages["aproxy-ipc"]["version"], version)
            self.assertIn(packages["aproxy-ipc"]["id"], nodes[packages["aproxy-desktop"]["id"]]["dependencies"])
            print(f"cargo metadata --locked: aproxy-desktop -> aproxy-ipc {version}")


if __name__ == "__main__":
    unittest.main()

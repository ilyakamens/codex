import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib


repo = Path(__file__).resolve().parent.parent
installed_cli = shutil.which("codex")
if installed_cli is None:
    raise SystemExit("Install the matching Codex release first; its voice runtime is required.")

source = Path(installed_cli).resolve().parent.parent
manifest = source / "codex-package.json"
metadata = json.loads(manifest.read_text())
workspace = tomllib.loads((repo / "codex-rs/Cargo.toml").read_text())
version = workspace["workspace"]["package"]["version"]
if metadata["version"] != version:
    raise SystemExit(f"Expected the installed Codex {version} voice runtime, got {metadata['version']}.")

voice_host = source / "codex-resources/voice/bin/codex-voice-host"
build_commit = subprocess.check_output([voice_host, "--build-commit"], text=True).strip()
# The voice helper requires the matching release's protocol build stamp.
subprocess.run(
    ["cargo", "build", "-p", "codex-cli", "--bin", "codex"],
    cwd=repo / "codex-rs",
    env={
        **os.environ,
        "CARGO_PROFILE_DEV_DEBUG": "0",
        "CARGO_INCREMENTAL": "0",
        "STABLE_GIT_COMMIT": build_commit,
    },
    check=True,
)

destination = repo / "codex-rs/target/voice-text-output"
(destination / "bin").mkdir(parents=True, exist_ok=True)
shutil.copy2(repo / "codex-rs/target/debug/codex", destination / "bin/codex")
shutil.copy2(manifest, destination / manifest.name)
resources = destination / "codex-resources"
if resources.is_symlink():
    resources.unlink()
shutil.copytree(source / "codex-resources", resources, dirs_exist_ok=True)
for name in ("codex-path", "bin/codex-code-mode-host"):
    target = source / name
    target.resolve(strict=True)
    link = destination / name
    if link.is_symlink():
        link.unlink()
    link.symlink_to(target, target_is_directory=target.is_dir())

print(f"Ready: {destination / 'bin/codex'} --no-daemon --enable voice_text_output")

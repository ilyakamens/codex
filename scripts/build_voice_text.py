import json
import os
import shutil
import subprocess
import tomllib
from pathlib import Path

repo = Path(__file__).resolve().parent.parent
installed_cli = shutil.which("codex")
if installed_cli is None:
    raise SystemExit(
        "Install the matching Codex release first; its voice runtime is required."
    )

source = Path(installed_cli).resolve().parent.parent
manifest = source / "codex-package.json"
metadata = json.loads(manifest.read_text())
workspace = tomllib.loads((repo / "codex-rs/Cargo.toml").read_text())
version = workspace["workspace"]["package"]["version"]
# Upstream main uses 0.0.0; only release builds have a matching package version.
if version != "0.0.0" and metadata["version"] != version:
    raise SystemExit(
        f"Expected the installed Codex {version} voice runtime, got {metadata['version']}."
    )

voice_host = source / "codex-resources/voice/bin/codex-voice-host"
build_commit = subprocess.check_output(
    [voice_host, "--build-commit"], text=True
).strip()
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
capture = destination / "bin/codex-voice-capture"
capture.unlink(missing_ok=True)
subprocess.run(
    [
        "xcrun",
        "swiftc",
        "-O",
        str(repo / "scripts/voice_capture.swift"),
        "-o",
        str(capture),
    ],
    check=True,
)
binary = destination / "bin/codex"
# Fresh files avoid stale macOS code signatures when rebuilding executables.
binary.unlink(missing_ok=True)
shutil.copy2(repo / "codex-rs/target/debug/codex", binary)
shutil.copy2(manifest, destination / manifest.name)
resources = destination / "codex-resources"
if resources.is_symlink():
    resources.unlink()
elif resources.exists():
    shutil.rmtree(resources)
shutil.copytree(source / "codex-resources", resources)
for name in ("codex-path", "bin/codex-code-mode-host"):
    target = source / name
    target.resolve(strict=True)
    link = destination / name
    if link.is_symlink():
        link.unlink()
    link.symlink_to(target, target_is_directory=target.is_dir())

print(f"Ready: {destination / 'bin/codex'} --no-daemon --enable voice_text_output")

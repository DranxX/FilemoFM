import json
import os
import platform
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COMMAND_TIMEOUT_SECONDS = 30


def command(arguments):
    try:
        result = subprocess.run(arguments, cwd=ROOT, capture_output=True, text=True, timeout=COMMAND_TIMEOUT_SECONDS, check=False)
        return {"ok": result.returncode == 0, "output": result.stdout.strip() if result.returncode == 0 else result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired):
        return {"ok": False, "output": "Command unavailable or timed out"}


def main():
    report = {
        "os": platform.platform(),
        "machine": platform.machine(),
        "rust": command(["rustc", "--version", "--verbose"]),
        "cargo": command(["cargo", "--version"]),
        "toolchain": command(["rustup", "show", "active-toolchain"]),
        "targets": command(["rustup", "target", "list", "--installed"]),
        "tools": {name: bool(shutil.which(name)) for name in ["cc", "c++", "cmake", "ninja", "pkg-config", "cl", "clang-cl", "lld-link", "qdrant", "llama-server", "xdg-desktop-portal"]},
        "windows_sdk_configured": bool(os.environ.get("WindowsSdkDir")),
        "display_configured": bool(os.environ.get("DISPLAY")),
        "wayland_configured": bool(os.environ.get("WAYLAND_DISPLAY")),
        "compilation_run": False,
    }
    metadata = command(["cargo", "metadata", "--locked", "--offline", "--format-version", "1"])
    if metadata["ok"]:
        data = json.loads(metadata["output"])
        packages = {item["id"]: item for item in data["packages"]}
        package = next(item for item in data["packages"] if Path(item["manifest_path"]).resolve() == ROOT / "Cargo.toml")
        node = next(item for item in data["resolve"]["nodes"] if item["id"] == package["id"])
        report["manifest"] = {"name": package["name"], "edition": package["edition"], "rust_version": package["rust_version"]}
        report["crates"] = {packages[item["pkg"]]["name"]: {"version": packages[item["pkg"]]["version"], "rust_version": packages[item["pkg"]]["rust_version"]} for item in node["deps"]}
        report["resolved_packages"] = len(packages)
        report["highest_declared_dependency_msrv"] = max((item["rust_version"] for item in packages.values() if item["rust_version"]), key=lambda version: tuple(int(part) for part in version.split(".")))
        report["duplicate_gui_versions"] = {name: sorted({item["version"] for item in packages.values() if item["name"] == name}) for name in ["egui", "eframe", "epaint"]}
        report["direct_features"] = {packages[item["pkg"]]["name"]: next(entry["features"] for entry in data["resolve"]["nodes"] if entry["id"] == item["pkg"]) for item in node["deps"]}
        report["target_dependency_resolution"] = {target: command(["cargo", "tree", "--locked", "--offline", "--target", target, "--depth", "1", "--prefix", "none"]) for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]}
    else:
        report["metadata_error"] = metadata["output"]
    if platform.system() == "Linux":
        report["pkg_config"] = {name: command(["pkg-config", "--modversion", name]) for name in ["x11", "xcb", "xkbcommon", "wayland-client", "glib-2.0", "gtk+-3.0", "dbus-1"]}
        libraries = command(["ldconfig", "-p"])
        report["runtime_libraries"] = {name: name in libraries["output"] for name in ["libxkbcommon.so.0", "libX11.so.6", "libwayland-client.so.0", "libEGL.so.1", "libGL.so.1"]}
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

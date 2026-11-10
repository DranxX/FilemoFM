import argparse
import hashlib
import json
import shutil
import struct
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUPPORTED_TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description="Package an existing FilemoFM binary; never builds or downloads.")
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--target", required=True, choices=SUPPORTED_TARGETS)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    source = arguments.binary.resolve(strict=True)
    destination = arguments.output.absolute()
    if not source.is_file() or arguments.binary.is_symlink() or destination.exists():
        parser.error("Use a regular binary and a new output directory")
    suffix = ".exe" if arguments.target.endswith("windows-msvc") else ""
    with source.open("rb") as binary:
        header = binary.read(64)
        if suffix:
            if len(header) < 64 or header[:2] != b"MZ":
                parser.error("Expected a Windows PE x64 binary")
            binary.seek(struct.unpack_from("<I", header, 60)[0])
            signature = binary.read(6)
            if len(signature) != 6 or signature[:4] != b"PE\0\0" or struct.unpack_from("<H", signature, 4)[0] != 0x8664:
                parser.error("Expected a Windows PE x64 binary")
        elif len(header) < 64 or header[:6] != b"\x7fELF\x02\x01" or struct.unpack_from("<H", header, 18)[0] != 62:
            parser.error("Expected a Linux ELF x64 binary")
    destination.mkdir(parents=True, exist_ok=False)
    try:
        binary = destination / ("filemofm" + suffix)
        shutil.copy2(source, binary)
        for name in ["data", "logs", "models", "runtime", "trash"]:
            (destination / name).mkdir()
        shutil.copy2(ROOT / "README.md", destination / "README.md")
        shutil.copy2(ROOT / "docs/PORTABLE.md", destination / "PORTABLE.md")
        shutil.copy2(ROOT / "Cargo.lock", destination / "Cargo.lock")
        for name in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE", "NOTICE"]:
            if (ROOT / name).is_file():
                shutil.copy2(ROOT / name, destination / name)
        with (ROOT / "Cargo.toml").open("rb") as manifest:
            version = tomllib.load(manifest)["package"]["version"]
        record = {"application": "FilemoFM", "version": version, "target": arguments.target, "binary": binary.name, "sha256": digest(binary), "build_run_by_packager": False, "native_runtimes_included": False, "models_included": False}
        (destination / "package.json").write_text(json.dumps(record, indent=2) + "\n")
    except Exception:
        shutil.rmtree(destination)
        raise
    print(destination)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"Packaging failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)

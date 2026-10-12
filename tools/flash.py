#!/usr/bin/env python3
"""Validate this badge/backup, then optionally write ONLY its selected app slot."""
import argparse
import hashlib
from pathlib import Path
import struct
import zlib


def active_slot(ota):
    entries = []
    for offset in (0, 0x1000):
        entry = ota[offset:offset + 32]
        if len(entry) != 32:
            raise ValueError("Incomplete OTA metadata")
        seq = struct.unpack_from("<I", entry)[0]
        state, crc = struct.unpack_from("<II", entry, 24)
        if 0 < seq < 0xffffffff and state in (0, 1, 2, 0xffffffff) and zlib.crc32(entry[:4], 0xffffffff) == crc:
            entries.append(seq)
    if not entries:
        raise ValueError("No valid OTA selection; refusing to guess a write offset")
    return (max(entries) - 1) % 2


def check_layout(table):
    entries = {}
    for offset in range(0, len(table) - 31, 32):
        magic, kind, subkind, start, size, name, flags = struct.unpack_from("<HBBII16sI", table, offset)
        if magic != 0x50aa:
            break
        entries[name.rstrip(b"\0")] = (kind, subkind, start, size, flags)
    required = {
        b"app0": (0, 0x10, 0x10000, 0x300000, 0),
        b"app1": (0, 0x11, 0x310000, 0x300000, 0),
        b"ffat": (1, 0x81, 0x610000, 0x9e0000, 0),
    }
    if any(entries.get(name) != value for name, value in required.items()):
        raise ValueError("Not the supported conference app3M_fat9M_16MB layout")


def check_image(image):
    if len(image) < 32 or image[0] != 0xe9 or struct.unpack_from("<H", image, 12)[0] != 9:
        raise ValueError("Expected ESP32-S3 application image")
    if len(image) > 0x300000:
        raise ValueError("Application exceeds the preserved 3 MiB partition")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("image", type=Path)
    parser.add_argument("--backup", type=Path, required=True)
    parser.add_argument("--port", required=True)
    parser.add_argument("--mac", required=True, help="Expected physical badge MAC")
    parser.add_argument("--write", action="store_true", help="Without this flag, read-only preflight")
    args = parser.parse_args()
    original = args.backup.read_bytes()
    manifest = (args.backup.parent / "SHA256SUMS").read_text().split()[0]
    if len(original) != 0x1000000 or hashlib.sha256(original).hexdigest() != manifest:
        raise ValueError("Full backup size/hash does not match its saved checksum")
    image = args.image.read_bytes()
    check_image(image)
    try:
        import esptool
        from esptool import cmds
    except ImportError:
        raise SystemExit("Run with: uv run --with esptool==5.4.0 python tools/flash.py ...")
    # Keep one connection/stub for all checks and the write. Repeated app resets
    # between CLI invocations can wedge this badge's native USB peripheral.
    with esptool.detect_chip(args.port, 115200) as loader:
        esp = loader
        success = False
        try:
            if esp.CHIP_NAME != "ESP32-S3":
                raise ValueError("Wrong chip")
            mac = ":".join(f"{byte:02x}" for byte in esp.read_mac())
            if mac != args.mac.lower():
                raise ValueError("Connected device MAC does not match expected badge")
            if esp.get_secure_boot_enabled() or esp.get_flash_encryption_enabled():
                raise ValueError("Refusing a secure-boot/encrypted device")
            esp = esp.run_stub()
            table = cmds.read_flash(esp, 0x8000, 0x1000, no_progress=True)
            ota = cmds.read_flash(esp, 0xe000, 0x2000, no_progress=True)
            if table != original[0x8000:0x9000]:
                raise ValueError("Partition table differs from verified conference backup")
            check_layout(table)
            slot = active_slot(ota)
            offset = (0x10000, 0x310000)[slot]
            print(f"Verified backup/device/layout. Selected app{slot} at {offset:#x}; other partitions untouched.")
            if args.write:
                cmds.write_flash(esp, [(offset, image)])
                cmds.verify_flash(esp, [(offset, image)])
                print("Application written and verified. No bootloader/table/NVS/profile image flashed.")
            else:
                print("Read-only preflight complete. Add --write to replace the application.")
            success = True
        finally:
            if success:
                esp.hard_reset()



if __name__ == "__main__":
    main()

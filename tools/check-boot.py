#!/usr/bin/env python3
"""uv run --with pyserial python tools/check-boot.py --port PORT [--reset].
Checks actual startup markers without printing or storing enrollment URLs.
"""
import argparse
import subprocess
import time
import serial

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--port", required=True)
p.add_argument("--reset", action="store_true")
args = p.parse_args()
if args.reset:
    subprocess.run(["uvx", "--from", "esptool==5.4.0", "esptool", "--chip", "esp32s3", "--port", args.port,
                    "--after", "hard-reset", "chip-id"], check=True, stdout=subprocess.DEVNULL)
s = serial.Serial(port=None, baudrate=115200, timeout=0.2)
s.dtr = False
s.rts = False
s.port = args.port
s.open()
end = time.monotonic() + 8
pending = b""
ready = False
mic_idle = False
failed = False
while time.monotonic() < end:
    pending += s.read(4096)
    if len(pending) > 32768:
        pending = pending[-32768:]
    while b"\n" in pending:
        raw, pending = pending.split(b"\n", 1)
        line = raw.decode(errors="replace").strip()
        if any(text in line for text in ("BADGE_PANEL", "BADGE_RENDER_READY", "BADGE_MIC_IDLE", "MIC_TEST_READY", "RIKER_BADGE_SETUP", "WIFI_SSID>")):
            print(line)
        ready |= "BADGE_RENDER_READY" in line
        mic_idle |= "BADGE_MIC_IDLE configured=1 running=0" in line
        failed |= any(text in line for text in ("Guru Meditation", "abort()", "panicked at", "Error: StopWatch", "badge startup stopped safely"))
s.close()
assert ready and mic_idle and not failed, "Startup check failed: renderer/mic-idle marker missing or runtime error (no secrets printed)"
print("PASS: renderer started, microphone idle at boot, device stayed up during the 8-second check.")

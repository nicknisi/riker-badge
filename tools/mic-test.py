#!/usr/bin/env python3
"""Explicit 30s USB microphone quality test. Never uploads audio.
uv run --with pyserial python tools/mic-test.py [--port PORT]
python3 tools/mic-test.py --self-test  # host-only protocol check, no serial dependency
"""
import argparse
from array import array
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import wave

RATE, SAMPLES = 16000, 480000


def decode_sample(lines, report=print):
    data = bytearray()
    checksum = None
    elapsed = 0
    expected = 0
    for line in lines:
        if line.startswith("MIC_ERROR"):
            raise RuntimeError("Badge stopped the test: " + line[10:])
        if line.startswith("MIC_ARMED"):
            report("Badge armed. Press A on the physical badge; B/tap cancels. Nothing records until A.")
        elif line == "MIC_RECORDING":
            report("Recording now. Read the test sentences clearly, including some speech from across the table.")
        elif line.startswith("MIC_BEGIN "):
            fields = line.split()
            if checksum is not None or len(fields) != 5:
                raise ValueError("Unexpected audio header")
            rate, count, elapsed = map(int, fields[1:4])
            checksum = fields[4]
            if rate != RATE or count != SAMPLES or not 27000 <= elapsed <= 34000:
                raise ValueError("Unexpected sample format or capture-clock duration")
            if len(checksum) != 64 or any(c not in "0123456789abcdef" for c in checksum):
                raise ValueError("Invalid sample checksum")
            report(f"Captured {elapsed/1000:.2f}s. Microphone is off; receiving the sample over USB...")
        elif line.startswith("MIC_DATA "):
            if checksum is None:
                raise ValueError("Audio arrived before its header")
            fields = line.split()
            if len(fields) != 3 or int(fields[1]) != expected:
                raise ValueError("Missing/out-of-order audio frame; refusing a damaged recording")
            chunk = bytes.fromhex(fields[2])
            expected_size = min(2048, SAMPLES * 2 - len(data))
            if not expected_size or len(chunk) != expected_size:
                raise ValueError("Incorrect audio-frame length")
            data.extend(chunk)
            expected += 1
        elif line == "MIC_END":
            if len(data) != SAMPLES * 2 or hashlib.sha256(data).hexdigest() != checksum:
                raise ValueError("Incomplete or corrupted sample; refusing playback")
            return data, elapsed
    raise TimeoutError("Test did not finish. No partial recording will be saved.")


def self_test():
    pcm = b"\x01\x00" * SAMPLES
    frames = [f"MIC_BEGIN {RATE} {SAMPLES} 30000 {hashlib.sha256(pcm).hexdigest()}"]
    frames += [f"MIC_DATA {i} {pcm[start:start+2048].hex()}" for i, start in enumerate(range(0, len(pcm), 2048))]
    frames += ["MIC_END"]
    assert decode_sample(iter(frames), lambda _: None)[0] == pcm
    for broken in [frames[:3] + frames[4:], [frames[0]] + frames[2:], frames[:-2] + ["MIC_END"],
                   [frames[0].replace("30000", "1000")] + frames[1:]]:
        try:
            decode_sample(iter(broken), lambda _: None)
        except (ValueError, TimeoutError):
            pass
        else:
            raise AssertionError("Damaged sample accepted")
    damaged = frames.copy()
    damaged[1] = damaged[1][:-2] + "ff"
    try:
        decode_sample(iter(damaged), lambda _: None)
    except ValueError:
        pass
    else:
        raise AssertionError("Checksum mismatch accepted")
    print("PASS: sample parser checks format, clock, order, size, completeness, and SHA256")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", default="/dev/cu.usbmodem1101")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    import serial
    print("MICROPHONE QUALITY TEST — 30 seconds, local playback only; no cloud upload.")
    print("Make sure everyone nearby agrees. Place the badge on a table, not against clothing.")
    print("The recording is temporary and will be deleted when this window finishes.")
    input("Press Enter to arm the test, or Ctrl-C to cancel: ")
    subprocess.run(["uvx", "--from", "esptool==5.4.0", "esptool", "--chip", "esp32s3", "--port", args.port,
                    "--after", "hard-reset", "chip-id"], check=True, stdout=subprocess.DEVNULL)
    port = serial.Serial(port=None, baudrate=115200, timeout=.2)
    port.dtr = False
    port.rts = False
    port.port = args.port
    port.open()
    pending = bytearray()

    def lines(seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            pending.extend(port.read(8192))
            if len(pending) > 16384:
                raise ValueError("Oversized serial frame")
            while b"\n" in pending:
                raw, _, tail = pending.partition(b"\n")
                pending[:] = tail
                # Ignore normal firmware logs, including any enrollment URLs.
                text = raw.rstrip(b"\r").decode("ascii", errors="replace")
                if text.startswith("MIC_"):
                    yield text

    requested = False
    received = False
    try:
        ready = any(line == "MIC_TEST_READY" for line in lines(20))
        if not ready:
            raise RuntimeError("Mic console unavailable; existing Wi-Fi provisioning must finish first")
        print("\nSuggested sentences, some nearby and some from about one metre away:")
        print('  "This is the badge microphone test. Nick is waiting for Riker."')
        print('  "The deadline is Friday at three. We will review the notes tomorrow."')
        print('  "Decision: ship the small version. Nick will send the summary."\n')
        requested = True
        port.write(b"MIC_TEST\n")
        pcm, elapsed = decode_sample(lines(160))
        received = True
    finally:
        port.close()
        if requested and not received:
            # A host abort must stop capture too, not just abandon its output.
            try:
                subprocess.run(["uvx", "--from", "esptool==5.4.0", "esptool", "--chip", "esp32s3", "--port", args.port,
                                "--after", "hard-reset", "chip-id"], check=True, timeout=15,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except (subprocess.SubprocessError, OSError):
                print("Could not reset the badge. Press B/tap to cancel; recording is time-limited.")
    values = array("h", pcm)
    if sys.byteorder != "little":
        values.byteswap()
    peak = max(map(abs, values))
    rms = math.sqrt(sum(int(value) * value for value in values) / len(values))
    clipped = sum(abs(value) >= 32760 for value in values) / len(values)
    if not peak:
        raise RuntimeError("The entire sample is zero: microphone capture needs diagnosis")
    print(f"Verified {len(values)} samples / {elapsed/1000:.2f}s; RMS {20*math.log10(max(rms,1)/32768):.1f} dBFS; clipping {clipped:.2%}.")
    played = False
    with tempfile.TemporaryDirectory(prefix="riker-mic-test-") as directory:
        path = Path(directory) / "sample.wav"
        with wave.open(str(path), "wb") as output:
            output.setnchannels(1)
            output.setsampwidth(2)
            output.setframerate(RATE)
            output.writeframes(pcm)
        path.chmod(0o600)
        if input("Play the sample locally now? [y/N] ").lower() == "y":
            subprocess.run(["afplay", str(path)], check=True)
            played = True
        input("Press Enter to delete the temporary audio, then tell the assistant what you heard: ")
    receipt = Path(__file__).resolve().parents[1] / ".tools/mic-test-result.json"
    receipt.write_text(json.dumps({"completed_at": time.time(), "samples": len(values),
        "capture_ms": elapsed, "rms_dbfs": 20*math.log10(max(rms,1)/32768),
        "clipped_fraction": clipped, "sha256": hashlib.sha256(pcm).hexdigest(),
        "played_locally": played, "audio_deleted": True, "uploaded": False}, indent=2))
    receipt.chmod(0o600)
    pcm[:] = b"\0" * len(pcm)
    print("Temporary audio deleted. Nothing was uploaded. Tell the assistant whether speech was clear across the table.")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nHost test stopped. If the badge is recording, press B/tap to cancel; its capture is bounded to 30 seconds.")
    except (ValueError, RuntimeError, TimeoutError, OSError, subprocess.SubprocessError) as error:
        print(f"Test stopped: {error}", file=sys.stderr)
        sys.exit(1)

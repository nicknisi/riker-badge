#!/usr/bin/env python3
"""Run locally: uv run --with pyserial python tools/provision.py [--port PORT]."""
import argparse
import getpass
import subprocess
import sys
import webbrowser
from urllib.parse import urlsplit
import serial
from serial.tools import list_ports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port")
    parser.add_argument("--reset", action="store_true", help="Restart the badge first so setup prompts are captured")
    args = parser.parse_args()
    ports = [p.device for p in list_ports.comports() if p.vid == 0x303A and p.pid == 0x1001]
    if not args.port and len(ports) != 1:
        parser.error("Specify --port: expected exactly one Espressif USB Serial/JTAG device")
    chosen_port = args.port or ports[0]
    if args.reset:
        subprocess.run(["uvx", "--from", "esptool==5.4.0", "esptool", "--chip", "esp32s3", "--port", chosen_port,
                        "--after", "hard-reset", "chip-id"], check=True, stdout=subprocess.DEVNULL)
    port = serial.Serial(port=None, baudrate=115200, timeout=1)
    port.dtr = False
    port.rts = False
    port.port = chosen_port
    port.open()
    print(f"USB console open on {port.port} — Wi-Fi and Tailscale still need setup.")
    print("Continue until TAILSCALE CONNECTED appears. If quiet, press the badge's reset button.")
    print("Passwords stay in this terminal and on the badge; nothing is written to a config file.")
    pending = b""
    seen_urls = set()
    try:
        while True:
            pending += port.read(1024)
            # Bound malformed serial output; never accumulate an unbounded line.
            if len(pending) > 8192:
                pending = pending[-8192:]
            while b"\n" in pending:
                raw, pending = pending.split(b"\n", 1)
                line = raw.decode("utf-8", errors="replace").strip()
                if "WIFI_SSID>" in line:
                    value = input("2.4 GHz Wi-Fi name: ")
                    if not 1 <= len(value.encode()) <= 32 or any(c in value for c in "\r\n\0"):
                        raise ValueError("Wi-Fi name must contain 1-32 bytes, no control separators")
                    port.write(value.encode() + b"\n")
                elif "WIFI_PASSWORD>" in line:
                    value = getpass.getpass("Wi-Fi password (hidden): ")
                    if not 8 <= len(value.encode()) <= 63 or any(c in value for c in "\r\n\0"):
                        raise ValueError("WPA2 password must contain 8-63 bytes, no control separators")
                    port.write(value.encode() + b"\n")
                    value = ""
                elif "TAILSCALE_PEER>" in line:
                    try:
                        default = subprocess.check_output(["tailscale", "ip", "-4"], text=True, timeout=5).strip()
                    except (OSError, subprocess.SubprocessError):
                        default = ""
                    value = input(f"Permitted sender's Tailscale IPv4 [{default}]: ").strip() or default
                    import ipaddress
                    if ipaddress.ip_address(value) not in ipaddress.ip_network("100.64.0.0/10"):
                        raise ValueError("Expected a Tailscale IPv4 address")
                    port.write(value.encode() + b"\n")
                else:
                    print(line)
                    if line == "WIFI_READY":
                        print("Wi-Fi connected. Next: Tailscale browser approval.")
                    if "Tailscale control ready: addresses=" in line and "addresses=unknown" not in line:
                        print("\nTAILSCALE CONNECTED — enrollment completed. Ready to test status messages.")
                        return
                    marker = "TAILSCALE APPROVAL REQUIRED: "
                    if marker in line:
                        url = line.split(marker, 1)[1].split("\x1b", 1)[0].strip()
                        parsed = urlsplit(url)
                        if parsed.scheme != "https" or parsed.hostname not in {"login.tailscale.com", "controlplane.tailscale.com"}:
                            raise ValueError("Rejected unexpected approval URL")
                        if url not in seen_urls:
                            seen_urls.add(url)
                            if input("Open this Tailscale approval link? [y/N] ").lower() == "y":
                                webbrowser.open(url)
    except KeyboardInterrupt:
        print("\nDisconnected; badge continues running.")
    finally:
        port.close()


if __name__ == "__main__":
    try:
        main()
    except (ValueError, serial.SerialException) as error:
        print(f"Setup stopped: {error}. Reboot the badge to retry.", file=sys.stderr)
        sys.exit(1)

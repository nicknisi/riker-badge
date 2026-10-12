#!/usr/bin/env python3
"""Send only a public Riker state over the host's Tailscale connection."""
import argparse
import json
from pathlib import Path
import socket
import subprocess
import time

STATES = {"idle", "working", "needs-you", "done"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("badge", help="Badge Tailscale IPv4 or MagicDNS name")
    parser.add_argument("state", nargs="?", choices=sorted(STATES))
    parser.add_argument("--riker", action="store_true", help="Poll the local Riker /status endpoint until Ctrl-C")
    parser.add_argument("--seconds", type=int, default=30, help="Manual-state heartbeat duration")
    args = parser.parse_args()
    if args.riker == bool(args.state):
        parser.error("Choose a state OR --riker")
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    address = socket.getaddrinfo(args.badge, 9010, socket.AF_INET, socket.SOCK_DGRAM)[0][4]
    import ipaddress
    if ipaddress.ip_address(address[0]) not in ipaddress.ip_network("100.64.0.0/10"):
        parser.error("Destination must resolve to a Tailscale IPv4 address")
    end = time.monotonic() + args.seconds
    last = None
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as transport:
        while args.riker or time.monotonic() < end:
            try:
                if args.riker:
                    response = subprocess.check_output([
                        "curl", "--silent", "--show-error", "--fail", "--max-time", "3",
                        "--unix-socket", str(Path.home() / ".riker/supervisor.sock"),
                        "http://localhost/status"], timeout=5, stderr=subprocess.DEVNULL)
                    state = json.loads(response)["state"]
                else:
                    state = args.state
                if state not in STATES:
                    raise ValueError("Unrecognized Riker status")
                transport.sendto(state.encode(), address)
                if state != last:
                    print(f"Sent {state} to {address[0]}:{address[1]} (delivery not acknowledged)")
                    last = state
            except (subprocess.SubprocessError, OSError, ValueError, KeyError):
                print("No valid Riker state; withholding heartbeat so the badge goes offline.")
            time.sleep(5)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\nStopped; badge will show offline within 15 seconds.")

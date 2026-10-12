#!/usr/bin/env python3
"""Store a short-lived, account-scoped Access setup token locally; never print it."""
import getpass
import json
import os
import sys
import warnings
from pathlib import Path
import urllib.request
import urllib.error

root = Path(__file__).resolve().parents[1] / ".tools"
context_path = root / "cloudflare-context.json"
context = json.loads(context_path.read_text())
account = context["account_id"]
print("Cloudflare Access setup for transcribe.nisi.sh")
print("Use a token limited to the Nick Nisi account:")
print("  Access: Apps and Policies — Edit")
print("  Access: Service Tokens — Edit")
print("  Access: Organizations — Read")
print("A one-day expiry is recommended. The token is for setup, not badge firmware.")
if not sys.stdin.isatty():
    raise SystemExit("Run this in an interactive Terminal so the token can stay hidden.")
warnings.simplefilter("error", getpass.GetPassWarning)
token = getpass.getpass("Paste the scoped API token (hidden): ").strip()
if len(token) < 20 or any(c.isspace() for c in token):
    raise SystemExit("Invalid token format; nothing saved.")
headers = {"Authorization": "Bearer " + token, "User-Agent": "riker-badge-setup/0.1"}
request = urllib.request.Request(f"https://api.cloudflare.com/client/v4/accounts/{account}/access/apps", headers=headers)
try:
    with urllib.request.urlopen(request, timeout=20) as response:
        result = json.load(response)
        if not result.get("success"):
            raise SystemExit("Cloudflare did not authorize Access; nothing saved.")
except urllib.error.HTTPError as error:
    raise SystemExit(f"Cloudflare returned HTTP {error.code}. Check account scope and permissions; nothing saved.")
path = root / "cloudflare-access-token"
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
with os.fdopen(fd, "w") as output:
    output.write(token)
path.chmod(0o600)
print("Token saved privately under the project's ignored .tools directory. No token value was logged.")
print("Tell the assistant 'token saved'. You can revoke this setup token after configuration finishes.")

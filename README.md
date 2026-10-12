# Riker badge

Nick’s M5Stack StopWatch: animated name badge, LinkedIn QR, live Riker pet, and an explicit-start private meeting recorder.

**Current field gate:** the saved hotspot is rejecting the badge (also reproduced with the older known-good firmware). Phone Wi-Fi save/rejoin and real badge-to-cloud recording still need a live-network check. Do not mistake synthetic soak results for a four-hour microphone test. See [VALIDATION.md](VALIDATION.md).

## Buttons

**Orange/yellow = A. Blue = B.**

| Where | Action |
|---|---|
| Name / pet | Orange switches views; Blue boops; hold Blue for calm mode |
| Name / pet | Tap for LinkedIn QR; tap again to return |
| Normal display | Press both buttons for Settings |
| Settings | Orange moves; Blue chooses; touch also selects |
| Ready to record | Orange explicitly starts; Blue backs out |
| Recording | Blue stops the microphone; uploads finish afterward |
| Stopped with buffered audio | Both opens Settings for Wi-Fi; Orange retries a blocked upload |
| Stopped with buffered audio | Hold **Blue alone** for two seconds to discard unsaved audio; cloud-saved audio remains |
| Saved / test complete | Blue returns |

The LinkedIn destination is exactly `https://linkedin.com/in/nicknisi`. Full-body waiting Nick, peeker, hop, tilt and quiet mode remain; no sound effects.

## Wi-Fi from your phone

1. Both buttons → **Wi-Fi setup** → Blue.
2. Scan the Wi-Fi QR to join the badge’s temporary password-protected network.
3. Open `http://192.168.4.1` if the phone does not open the form. Orange switches to a URL QR; Blue cancels setup.
4. Enter a 2.4 GHz WPA2 network/hotspot. Settings are saved **only after connection succeeds**; failed attempts retain the previous settings.

The setup AP expires after ten minutes. Its form uses a random nonce and rejects access through the normal station interface. Setup and microphone capture cannot own the hardware at the same time. There is no always-on public configuration server. USB provisioning remains available:

```sh
uv run --with pyserial python tools/provision.py --port /dev/cu.usbmodem1101
```

## Record, read, draft

1. Connect Wi-Fi and use USB power for now; battery endurance has not been measured.
2. Both → **Record meeting** or **Record a take** → Blue.
3. The readiness screen warns if Wi-Fi is disconnected or time has not synchronized. An unavailable network shows **Not started**, not a misleading recording-stop message; Both opens settings, Blue goes back, Orange retries. Capture is online-only.
4. Get everyone’s consent, then press Orange. A three-second countdown precedes microphone warm-up. Wait for **RECORDING** before speaking; the first second warms the mic/filter and is discarded. No recording starts on boot, reconnection, or recovery.
5. The red indicator follows the actual microphone state. The display separates captured, cloud-saved and buffered time. Blue stops capture independently of the HTTPS uploader.
6. Open **https://transcribe.nisi.sh** on your phone and sign in as the owner (`nick@nisi.org`). Transcripts, notes and action items are private.
7. For tweets, choose/edit the exact source passage, optionally add **your own take**, mark that material shareable, and generate three options. Review facts and voice, edit, then Copy or Open in X. **You still press Post in X. Nothing posts automatically.**

Draft edits save privately. History changes wait for saving; revision checks prevent another tab from silently overwriting edits. Generated drafts omit unverified links/handles/quotes; you can add verified ones yourself. Known credential patterns are blocked, but that is not a substitute for reviewing private information. The voice profile is a starting point based on public short-form posts and essays, not a trained replica or a full X archive.

### Recording limits and interruptions

- Online-first: 16 kHz mono PCM16LE, immutable numbered ten-second chunks, SHA-256 receipts.
- Four preallocated buffers (~1.28 MB), including any in-flight chunk: **roughly forty seconds of unsaved audio**, not hours of offline storage.
- If buffers fill, capture stops visibly rather than overwriting audio. Unsaved chunks remain in RAM for retry while power remains on. Fix Wi-Fi or explicitly discard them.
- Restart/power loss loses the unsaved RAM tail. An NVS session journal closes the interrupted cloud session without restarting the microphone. Already acknowledged audio stays in the cloud.
- Maximum eight hours per recording and per rolling day; twelve session creations per rolling day. Twenty draft generations per day, at most two model attempts each.
- Cloudflare’s own plan/model quotas can also delay processing. **Verify the account’s Workers AI plan before relying on a long continuous-speech meeting.** No subscription was upgraded; account subscription lookup was not authorized. Failed/partial processing is visible and can be retried while original audio remains.
- Current published Whisper Turbo pricing is $0.000513/audio minute. Four hours of continuous speech with two-second context on both sides of ten-second chunks is roughly 336 processed minutes, or **$0.17 of ASR usage before free allowance**, plus notes/drafts/storage/requests. The free Workers AI allowance is 10,000 neurons/day and stops processing when exhausted; the synthetic soak’s sparse speech does **not** prove a four-hour real-speech quota. Sources: [model pricing](https://developers.cloudflare.com/workers-ai/models/whisper-large-v3-turbo/) and [Workers AI limits](https://developers.cloudflare.com/workers-ai/platform/pricing/).
- Raw audio access ends **seven days from session creation**. Cleanup and R2 lifecycle remove objects; lifecycle is a backup, not an instantaneous deletion guarantee. Transcripts, notes and drafts remain until you delete the recording.
- Speaker diarization and speaker attribution are not guaranteed. Summaries/actions are AI outputs: verify important details.

Audio goes directly to the private HTTPS service; it does **not** depend on an awake Mac. The Riker status bridge does depend on this Mac: sleep makes the pet go offline after fifteen seconds, but that is separate from recording.

## Security and recovery

Cloudflare Access protects the owner viewer. The badge has a separate write-only service credential; it cannot read transcripts, replay audio, delete recordings or generate tweets. TLS server certificates are verified and redirects are refused. No reusable Tailscale auth key, owner login token or Cloudflare administration token is compiled into firmware.

Badge flash/NVS are **not encrypted**. Treat the physical badge and backups as credentials. Use a guest/hotspot network; revoke its Tailscale node and dedicated Access service token if lost. Do not upload `.tools/`, recordings, or flash backups.

Original full-flash backup (device-verified):
`~/Documents/badge-backups/stopwatch-20261006-133026/original-flash.bin`

SHA-256: `f5b87556ff743f85523b62beeb1cbdc9770ca32960d265b54865cbf4fd178cc8`

`tools/flash.py` checks backup checksum, MAC/security state, exact partition table, OTA selector and 3 MiB app limit. A single esptool connection writes/verifies **only the selected app**. It never erases the whole device or writes bootloader, table, NVS or conference-profile filesystem. Do not use a generic upstream flash runner or burn security eFuses.

## Build and checks

Apple Silicon macOS: `uv`, host Rust/cargo, git and several GB free. Toolchains/dependencies are pinned; setup does not edit shell startup files.

```sh
bash tools/setup.sh
cargo test --locked --lib
python3 tools/test_flash.py
uv run --python 3.12 --with pillow --with zxing-cpp python tools/test_badge_qr.py
c++ -std=c++17 tools/test-recorder-controls.cpp -o .tools/test-recorder-controls
.tools/test-recorder-controls
(cd cloud && pnpm test && pnpm check)
bash tools/build.sh
uvx --from esptool==5.4.0 esptool --chip esp32s3 elf2image \
  --flash-size 16MB --flash-mode qio --flash-freq 80m \
  -o .tools/riker-badge.bin target/xtensa-esp32s3-espidf/release/riker-badge
uv run --with esptool==5.4.0 python tools/flash.py .tools/riker-badge.bin \
  --backup ~/Documents/badge-backups/stopwatch-20261006-133026/original-flash.bin \
  --port /dev/cu.usbmodem1101 --mac 28:84:85:44:6a:d0
# Add --write after reviewing preflight. Never flash during an active recording/test.
```

`tools/build.sh` invalidates native C++ inputs when they change. Its file glob is restricted to a generated partition input directory, not package-manager symlink graphs. Editing vendored M5 sources requires `touch sdkconfig.defaults` before building. Avatar conversion: `python3 tools/convert-avatar.py`.

### Safe test/provisioning tools

- `tools/mic-test.py`: explicitly armed thirty-second quality test; physical Orange starts, Blue/touch cancels. Local verified transfer/playback/deletion; no cloud upload.
- `tools/device-soak.py --seconds 7200`: real-time **generated-data** hardware producer/buffer test. No microphone, no cloud upload. `--overflow` deliberately stalls consumption and verifies capture stops at four full retained buffers.
- `tools/cloud-soak.py --hours 4`: real-time synthetic HTTPS/storage/processing test, including a deliberate outage. No microphone; not an ASR-quality test of four hours of continuous speech.
- `tools/check-wifi.py --no-reset`: temporary AP/form checks; `--require-station` also requires working station connectivity.
- `tools/configure-cloud.py`: privately sends only the dedicated service credential over local USB. Never send the administration setup token to the badge.
- USB diagnostics `HEAP` and `CLOUD_CHECK` do not start capture.
- `cloud/tools/preview.ts`: loopback-only UI fixture, never deployed or used as an authentication bypass.
- `cloud/tools/voice-smoke.ts`: real model inference against synthetic inputs/local storage, never posts.

Private local credentials live under ignored `.tools/` with mode 0600. Provisioning is already performed for this badge; do not regenerate tokens casually.

**Security cleanup still needs the owner:** the temporary account-owned Cloudflare setup API token is active with no expiry. Its narrow permissions refused self-revocation (403); its local credential file has been deleted. Revoke that setup token in the account API Tokens page (identifier in `.tools/setup-token-status.json`). Keep the separate badge Access service token: it is what authenticated uploads use.

## Riker

The badge is `100.79.56.98`; the allowed sender is this Mac (`100.91.56.5`). Native Tailscale validates DERP TLS, WireGuard source binding and packet filters. Only state words cross the bridge; private task text never does.

```sh
python3 tools/send-status.py riker-badge working
python3 tools/send-status.py riker-badge --riker
```

The current bridge is a session process, not a login service. Its PID/log are `.tools/riker-bridge.pid` and `.tools/riker-bridge.log`; confirm the process command before stopping it because PIDs can be reused. Source review is bounded, not a full security audit of the community Tailscale implementation.

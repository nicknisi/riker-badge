# Riker badge

Nick’s M5Stack StopWatch: animated name badge, LinkedIn QR, live Riker pet, and an explicit-start private meeting recorder.

**Current field gate:** the saved hotspot is rejecting the badge (also reproduced with the older known-good firmware). Phone Wi-Fi save/rejoin and real badge-to-cloud recording still need a live-network check. Do not mistake synthetic soak results for a four-hour microphone test. See [VALIDATION.md](VALIDATION.md).

## Buttons

**Orange/yellow = A. Blue = B.**

| Where | Action |
|---|---|
| Name / pet | Orange switches views; Blue boops; hold Blue to toggle quiet mode |
| Name / pet | Tap the screen for the LinkedIn QR; tap or press either button to return |
| Normal display | Press both buttons for Settings; press both again to close |
| Settings | Orange moves; Blue chooses; touch also selects |
| Ready to record | Orange explicitly starts; Blue backs out |
| Recording | Blue stops the microphone; uploads finish afterward |
| Stopped with buffered audio | Press both for Settings (Wi-Fi); Orange retries uploads |
| Stopped with buffered audio | Hold **Blue alone** for two seconds to discard unsaved audio; cloud-saved audio remains |
| Not started | Orange retries; Blue goes back; press both for Settings |
| Saved / test complete | Blue returns |

The LinkedIn destination is exactly `https://linkedin.com/in/nicknisi`. The pet view has full-body waiting Nick, a peeker, hop and tilt reactions, and a dimmed, still quiet mode. No sound effects.

## Wi-Fi from your phone

1. Both buttons → **Wi-Fi setup** → Blue.
2. Scan the Wi-Fi QR to join the badge’s temporary password-protected network.
3. Open `http://192.168.4.1` if the phone does not open the form. Orange switches to a URL QR; Blue cancels setup.
4. Enter a 2.4 GHz WPA2 (or open) network/hotspot. Settings are saved **only after connection succeeds**; failed attempts retain the previous settings.

The setup AP expires after ten minutes. Its form uses a random nonce and rejects access through the normal station interface. Setup and microphone capture cannot own the hardware at the same time. There is no always-on public configuration server.

Wi-Fi setup is phone-only on current firmware. `tools/provision.py` still works as a USB serial console and Tailscale-approval helper, but its Wi-Fi and sender prompts only appear with older firmware:

```sh
uv run --with pyserial python tools/provision.py --port /dev/cu.usbmodem1101
```

## Record, read, draft

1. Connect Wi-Fi and use USB power for now; battery endurance has not been measured.
2. Both → **Record meeting** or **Record a take** → Blue.
3. The readiness screen warns if Wi-Fi is disconnected or time has not synchronized. An unavailable network shows **Not started**, not a misleading recording-stop message; Both opens settings, Blue goes back, Orange retries. Capture is online-only.
4. Get everyone’s consent, then press Orange. A three-second countdown precedes microphone warm-up. Wait for **RECORDING** (after **WARMING UP**) before speaking; the first second of mic audio is discarded while the codec warms up. No recording starts on boot, reconnection, or recovery.
5. The red indicator follows the actual microphone state. The display separates captured, cloud-saved and buffered time. Blue stops capture independently of the HTTPS uploader.
6. Open **https://transcribe.nisi.sh** on your phone and sign in as the owner (`nick@nisi.org`). Transcripts, notes and action items are private.
7. For tweets, choose/edit the exact source passage, optionally add **your own take**, mark that material shareable, and generate three options. Review facts and voice, edit, then Copy or Open in X. **You still press Post in X. Nothing posts automatically.**

Draft edits save privately. Switching history entries waits for pending saves; revision checks prevent another tab from silently overwriting edits. Generated drafts are rejected if they contain any link, @handle or double quote; you can add verified ones yourself while editing. Known credential patterns are blocked, but that is not a substitute for reviewing private information. The voice profile is a starting point based on public short-form posts and essays, not a trained replica or a full X archive.

The repo also contains unfinished automatic tweet-suggestion code (`cloud/migrations/0004.sql`, `suggestion_windows`) that is **not deployed**; production runs the manual draft flow above. See [VALIDATION.md](VALIDATION.md).

### Recording limits and interruptions

- Online-first: 16 kHz mono PCM16LE, immutable numbered ten-second chunks, SHA-256 receipts.
- Four preallocated buffers (~1.28 MB), including any in-flight chunk: **roughly forty seconds of unsaved audio**, not hours of offline storage.
- If buffers fill, capture stops visibly rather than overwriting audio. Unsaved chunks remain in RAM for retry while power remains on. Fix Wi-Fi or explicitly discard them.
- Restart/power loss loses the unsaved RAM tail. An NVS session journal closes the interrupted cloud session without restarting the microphone. Already acknowledged audio stays in the cloud.
- Maximum eight hours per recording and eight hours total per rolling day; twelve session creations per rolling day (deleted recordings still count). Twenty manual draft generations per day, at most two model attempts each.
- Cloudflare’s own plan/model quotas can also delay processing. **Verify the account’s Workers AI plan before relying on a long continuous-speech meeting.** No subscription was upgraded; account subscription lookup was not authorized. Failed/partial processing is visible and can be retried while original audio remains.
- Current published Whisper Turbo pricing is $0.000513/audio minute. Four hours of continuous speech with two-second context on both sides of ten-second chunks is roughly 336 processed minutes, or **$0.17 of ASR usage before free allowance**, plus notes/drafts/storage/requests. The free Workers AI allowance is 10,000 neurons/day and stops processing when exhausted; the synthetic soak’s sparse speech does **not** prove a four-hour real-speech quota. Sources: [model pricing](https://developers.cloudflare.com/workers-ai/models/whisper-large-v3-turbo/) and [Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/).
- Raw audio access ends **seven days from session creation**. A five-minute cleanup cron and an R2 lifecycle rule (configured on the bucket, not in this repo) remove objects; lifecycle is a backup, not an instantaneous deletion guarantee. Transcripts, notes and drafts remain until you delete the recording.
- Speaker diarization and speaker attribution are not guaranteed. Summaries/actions are AI outputs: verify important details.

Audio goes directly to the private HTTPS service; it does **not** depend on an awake Mac. The Riker status bridge does depend on this Mac: sleep makes the pet go offline after fifteen seconds, but that is separate from recording.

## Security and recovery

Cloudflare Access protects the owner viewer. The badge has a separate upload-only service credential; besides uploading, it can read only its config and recording status. It cannot read transcripts, replay audio, delete recordings or generate tweets. TLS server certificates are verified and redirects are refused. No reusable Tailscale auth key, owner login token or Cloudflare administration token is compiled into firmware.

Badge flash/NVS are **not encrypted**. Treat the physical badge and backups as credentials. Use a guest/hotspot network; revoke its Tailscale node and dedicated Access service token if lost. Do not upload `.tools/`, recordings, or flash backups.

Original full-flash backup (device-verified):
`~/Documents/badge-backups/stopwatch-20261006-133026/original-flash.bin`

SHA-256: `f5b87556ff743f85523b62beeb1cbdc9770ca32960d265b54865cbf4fd178cc8`

`tools/flash.py` checks backup checksum, MAC/security state, exact partition table, OTA selector and 3 MiB app limit. A single esptool connection writes/verifies **only the selected app**. It never erases the whole device or writes bootloader, table, NVS or conference-profile filesystem. Do not use a generic upstream flash runner or burn security eFuses.

## Build and checks

Apple Silicon macOS with `uv`, host Rust/cargo, git, curl, a C++ compiler (Xcode Command Line Tools) and several GB of free disk. The cloud tests need Node 22.18+ (native TypeScript) and pnpm. The ESP toolchain, ESP-IDF and M5 sources are pinned; setup does not edit shell startup files.

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

`tools/build.sh` forces a native rebuild when `components/badge_display` changes. Its native-input glob covers only a generated `.tools/native-input/` directory. The M5 sources that setup fetches into `components/` are not tracked for changes: run `touch sdkconfig.defaults` after editing them. Avatar conversion: `uv run --with pillow python tools/convert-avatar.py`.

### Safe test/provisioning tools

The Python serial tools need pyserial (`uv run --with pyserial python tools/<tool>.py`); several default to `/dev/cu.usbmodem1101`.

- `tools/mic-test.py`: explicitly armed thirty-second quality test; physical Orange starts, Blue/touch cancels. Local verified transfer/playback/deletion; no cloud upload.
- `tools/device-soak.py --seconds 7200`: real-time **generated-data** hardware producer/buffer test. No microphone, no cloud upload. `--overflow` deliberately stalls consumption and verifies capture stops at four full retained buffers.
- `tools/cloud-soak.py --hours 4`: real-time synthetic HTTPS/storage/processing test, including a deliberate outage. No microphone; not an ASR-quality test of four hours of continuous speech.
- `tools/check-wifi.py --no-reset`: temporary AP/form checks; `--require-station` also requires working station connectivity.
- `tools/check-boot.py`: checks startup markers without printing enrollment URLs. `tools/check-recording-start.py`: verifies offline start rejection shows **Not started**, using generated silence only.
- `tools/configure-cloud.py`: privately sends only the dedicated service credential over local USB. Never send the administration setup token to the badge.
- USB diagnostics `HEAP` and `CLOUD_CHECK` do not start capture.
- `cloud/tools/preview.ts`: loopback-only UI fixture, never deployed or used as an authentication bypass.
- `cloud/tools/voice-smoke.ts`: real model inference against synthetic inputs/local storage, never posts.
- `tools/save-access-token.py`: stores an account-scoped Access setup token in `.tools/` (mode 0600) without printing it.
- `connect.command`, `mic-test.command` and `access-setup.command` are double-click wrappers for `provision.py --reset`, `mic-test.py` and `save-access-token.py`.

Private local credentials live under ignored `.tools/` with mode 0600. Provisioning is already performed for this badge; do not regenerate tokens casually.

**Security cleanup still needs the owner:** the temporary account-owned Cloudflare setup API token is active with no expiry. Its narrow permissions refused self-revocation (403); its local credential file has been deleted. Revoke that setup token in the account API Tokens page (identifier in `.tools/setup-token-status.json`). Keep the separate badge Access service token: it is what authenticated uploads use.

## Riker

Riker is Nick’s local agent supervisor; the pet mirrors its state. The badge is `100.79.56.98`; the allowed sender is this Mac (`100.91.56.5`), read from the NVS `peer` key. That key was written by earlier firmware's USB prompt; current firmware has no way to set it, and a badge without it ignores all senders. Native Tailscale ([tailscale-esp32](https://github.com/0xdilo/tailscale-esp32), pinned in `Cargo.toml`; `src/main.rs` adapts its example under [LICENSE.tailscale-esp32](LICENSE.tailscale-esp32)) validates DERP TLS, WireGuard source binding and packet filters. Only state words cross the bridge; private task text never does.

```sh
python3 tools/send-status.py riker-badge working
python3 tools/send-status.py riker-badge --riker
```

The first command sends one state for 30 seconds. The second is the bridge: it polls `~/.riker/supervisor.sock` until Ctrl-C. It runs in a terminal session, not as a login service, so it is not running unless started. When started as recorded in VALIDATION.md, its PID/log are `.tools/riker-bridge.pid` and `.tools/riker-bridge.log`; confirm the process command before stopping it because PIDs can be reused. Source review is bounded, not a full security audit of the community Tailscale implementation.

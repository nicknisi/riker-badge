# Prototype validation — 2026-10-06/07

## Passed

- Rust host tests: 2 passed (`cargo test --locked --lib`): exact status vocabulary, invalid-packet rejection, 15-second staleness, approval-URL allowlist.
- Python flash guards: passed (`python3 tools/test_flash.py`): chip/size boundary, OTA state/CRC, exact supported app/profile layout. OTA CRC also checked against the real original device metadata (seq 1, CRC 0x4743989a).
- Python tools compile (`python3 -m py_compile ...`).
- Local Riker `/status` endpoint responds with a supported state; no private goal/question data was printed or transmitted.
- ESP32-S3 release build succeeded with ESP-IDF 5.4.3, M5Unified + M5GFX, and native Rust Tailscale client. Application approximately 1.7 MiB, below preserved 3 MiB slot capacity.
- Physical device preflight: MAC 28:84:85:44:6a:d0, unencrypted development configuration, exact original partition table, app0 active.
- App-only flash at 0x10000 succeeded and esptool read-back verification matched.
- Bootloader + partition-table region (0x0–0x9000) and complete conference-profile filesystem (0x610000, size 0x9e0000) separately verified byte-digest-identical to the original backup after flashing.
- Original ESP-IDF 5.5.4 bootloader successfully booted the new ESP-IDF 5.4.3 application.
- Physical boot: 8 MB PSRAM detected; memory test OK.
- Physical runtime startup check passed for 8 seconds (`tools/check-boot.py --reset`):

```text
BADGE_PANEL board=30 width=468 height=468
BADGE_RENDER_READY
RIKER_BADGE_SETUP
WIFI_SSID>
PASS: physical device initialized the renderer and stayed up during the 8-second check.
```

Initial prototype firmware SHA256: `41b7595918b0daa6f5729d038c5f47951a2b28bf4507dee4206b58e68c24ce34`.
Logs are local/ignored under `.tools/` (`firmware-build.log`, `flash.log`, `preservation-check.log`, `boot-check.log`).

## Issue reproduced and fixed

The first hardware boot safely stopped at an overly strict 466×466 display-size assertion. The pinned M5GFX CO5300 driver exposes 468×468 logical dimensions for the advertised 466×466 visible panel. The guard now checks the identified StopWatch and sufficient drawing area instead; the subsequent hardware startup check passed. No driver pins, partition sizes, eFuses or security settings were changed to work around it.

## End-to-end hardware results

- User confirmed Nick is visibly animating.
- USB provisioning input was checked with a deliberately invalid password: the password prompt advanced and validation rejected it before Wi-Fi connection or credential storage.
- User entered real Wi-Fi credentials locally and completed browser approval. No credentials were requested in chat or compiled into firmware.
- `tailscale status --json` reports `riker-badge` online at `100.79.56.98`.
- Hardware logs show network time sync, certificate validation, persistent identity reload, authenticated DERP connection, WireGuard session establishment, then three `accepted authenticated badge status` entries while the permitted Mac sent `working`.
- Later `tailscale ping --icmp --c 1 --timeout 8s 100.79.56.98` succeeded in 1.435 seconds. An earlier default discovery ping timed out during bring-up; full stock-client discovery-ping parity has not been established.
- Physical sequence sent: `needs-you` for 10 seconds, `done` for 10 seconds, then invalid `not-a-state` packets every 3 seconds for 21 seconds. **User explicitly confirmed all three captions:** RIKER NEEDS YOU → MISSION COMPLETE → SUBSPACE LINK LOST. This verifies visible state changes and stale-state fallback despite invalid traffic.
- Live bridge launched on this Mac: `python3 -u tools/send-status.py 100.79.56.98 --riker`, PID recorded in `.tools/riker-bridge.pid`; first logged state was `done`. Only the state word is sent, never task details. This is a session process, not an installed startup service.

## Name/QR + whimsy revision — 2026-10-06

- Added the user's exact destination `https://linkedin.com/in/nicknisi` via native M5GFX QR encoding. Static QR with quiet zone fits inside the visible circle.
- Added name/pet views, full-body waiting GIF for active Riker states, idle peeker, completion hop, boop reactions, tilt nudge, and optional dim/static calm mode. No sounds.
- Asset conversion preserves source timing and checks deduplication round trips: PEEK 35 frames/17 unique, BLINK 8/2, WAIT 20/4. Total pixel data 156,672 bytes.
- `tools/test_badge_qr.py` compiled the actual pinned M5GFX QR encoder, reproduced its quiet-zone geometry and independently decoded its pixels with ZXing: exact URL matched; circular-screen bounds passed.
- Rust host tests (2) and Python flash guards passed; native release build succeeded.
- Reviewed overlay behavior: expiry events cannot repaint QR; hold-to-calm is ignored during scanning; calm mode avoids repeated static redraws. Native RGB565 blit semantics checked against M5GFX `create_pc` implementation.
- Application image: **1,890,096 bytes**. SHA256: `cb69805ea46cf6f31615f08c5afe8f04725188699dfa68484e222051fd6154e6`.
- App-only flash and read-back verification passed; subsequent physical boot/renderer check passed. Existing Wi-Fi and Tailscale identity were retained. ICMP ping succeeded after reboot (498 ms).
- **Human verification:** user answered “Yes, correct profile” after scanning the physical badge QR with a phone; user answered “Yes, much better” for the waiting animation and A/B view/boop interactions.
- Temporary working-state fixture was stopped. Live read-only Riker bridge restored (current PID in `.tools/riker-bridge.pid`); first state after restoration was `working`.

Automated desktop/browser UI verification is not available for the physical ESP32 screen. The above visible-flow evidence is explicitly the user's physical confirmation, not invented computer-use state refs or screenshots.

## Remaining field checks

- Calm/tilt/haptic details have not all been independently measured/confirmed.
- Rejection of a second real tailnet sender has source-review coverage, not a physical second-device test.
- Wi-Fi outage recovery, battery life, prolonged uptime, and memory stability need longer field tests. Wi-Fi/identity persistence across USB-triggered reboots was observed.

Do not describe these unperformed field tests as complete.

## Microphone quality — human-confirmed

- Explicit USB-arm + physical Orange start, countdown, Blue/touch cancellation, microphone off at boot. Thirty-second local transfer checks sequence and SHA; buffers are zeroized.
- Receipt `.tools/mic-test-result.json`: **480,000 samples / 30,002 ms**, RMS **−21.17 dBFS**, no clipped samples. Local playback completed and the temporary audio was deleted; not uploaded.
- User said speech sounded great even across the room. This validates that short input-quality test, not a hours-long recording session.

## Phone Wi-Fi setup

- Random WPA2 AP, Wi-Fi-join QR and local-form QR, nonce, AP destination/peer guards, bounded inputs, ten-minute timeout, save-after-connect and rollback, atomic `wifi_v1` NVS record, microphone/setup arbiter.
- Native QR encoder roundtrips passed for LinkedIn, setup-network and form URLs. Offline on-device form delivery, invalid-nonce rejection and shutdown passed.
- Corrected the ESP-IDF AP default to `192.168.4.1` for the netif and DNS. Recovered a wedged USB interface with a targeted reset of the sole `303a:1001` device; flash tooling now uses one esptool connection/stub.
- **Not passed:** real phone save/rejoin and live-station interface rejection. The saved hotspot rejects the badge with disconnect reason 3; the old known-good QR firmware also fails DHCP. No original-profile fallback network was available. Mac networking was not changed to work around it.

## Deployed cloud pipeline

- Private custom domain `transcribe.nisi.sh`; Access owner login separated from a dedicated service-token JWT identity. This is not client-certificate/mTLS authentication. Public R2 access disabled; native seven-day audio lifecycle configured; D1 ledger, queue, DLQ and reconciliation deployed.
- Anonymous access redirects to Access. Badge credentials are rejected on owner routes. Owner/JWT/audience/CSRF validation is covered by real signed-fixture tests; interactive owner email login remains unperformed.
- A real 9.5-second synthetic speech upload was transcribed accurately in one ASR attempt and summarized. It remains as the clearly titled **Synthetic pipeline check — not a real meeting** for owner inspection.
- **Four-hour real-time synthetic cloud soak PASSED:** 1,440/1,440 ten-second chunks, **230,400,000 samples**, all 1,440 processed, final status `complete`, zero HTTP errors. Elapsed **14,411.6 seconds**, including final processing. A deliberate 35-second outage raised the six-slot simulated buffer high-water to four and recovered without loss.
- This soak uses generated speech across a chunk boundary every five minutes, with silence between. It proves real elapsed-time HTTPS/storage/processing recovery, **not four hours of live microphone audio, continuous-speech ASR quality/quota, or badge Wi-Fi/TLS transport**.
- Receipts: `.tools/cloud-soak-progress.json`, `.tools/cloud-soak.log`, `.tools/cloud-soak-db-proof.json`. The large synthetic fixture was removed after preserving proof: the real Worker cleanup marked `raw_cleaned=1`, then its synthetic ledger rows were purged to release the application's rolling-day quota. `.tools/cloud-soak-cleanup-proof.json` and `.tools/cloud-soak-purge-proof.json` record this. The ten-second synthetic demo remains; Cloudflare's own inference usage is not reset by fixture cleanup.

## Continuous recording implementation and checks

- Four preallocated PCM slots (1.28 MB), bounded non-overwriting queue, independent capture/stop supervisor, actual microphone indicator, explicit countdown/warm-up, physical-only real start, HTTPS receipt verification, retained unsaved buffers, retry/discard, eight-hour limit and NVS interruption journal. Recovery never restarts capture.
- Read-only safety review followed by fixes: a scope-bound stream guard stops/joins callbacks before releasing the ring; permanently rejected uploads pause with retained data rather than retrying forever or silently discarding; credentials can be repaired while capture is stopped.
- Both-button settings intent is separate from overlay visibility, and precedes individual-button gestures. Holding both cannot become the destructive Blue-only discard gesture. Portable C++ assertions cover the chord/discard policy.
- Seven Rust host tests pass, including accelerated four-hour buffer recycling and full-buffer/no-overwrite checks. These are not elapsed-time hardware evidence.
- Initial physical generated-data smoke: **60 seconds / 960,320 samples**, minimum free internal heap **176,091 bytes**, `mic_used=false`, no cancellation. `.tools/device-soak-result.json` / duration-specific receipts identify later runs.
- **Two-hour real-time on-device generated-data test PASSED:** 7,200 seconds, **115,200,960 samples**, minimum free internal heap **176,075 bytes**, no cancellation, `mic_used=false`. Receipt: `.tools/device-soak-7200s-result.json`. That image is preserved as `.tools/riker-badge-soaked.bin`, SHA-256 `2f07c5518c635d434176fa4e66d1dd8a956b3e66962cee2c1d6c0768a47c87f8`.
- Final refinements (settings/discard chord, honest warm-up label, explicit HTTP user-agent and overflow diagnostic) were followed by a fresh final-image test: **60 seconds / 960,320 samples**, minimum internal heap **176,091 bytes**, microphone off. `.tools/device-soak-60s-result.json`.
- **Final-image buffer exhaustion PASSED:** deliberately no consumption; capture stopped at exactly **640,000 accepted samples / four full buffers**, all retained, microphone off. `.tools/device-overflow-result.json`. This is real hardware callback/queue-stop evidence, not a live-microphone or HTTPS outage test.
- Final app-only flash/read-back verified **2,027,776 bytes**, SHA-256 **`0af9f313546aa9edc74dcd210a416d35a5de1337165490be29521c8b0993baf9`**. Final reset/startup check passed: renderer ready, microphone idle, device up for eight seconds. Badge left in normal boot state, not setup/capture/test mode. `.tools/final-boot-check.log`.
- Final public-HTTPS checks with the exact firmware user-agent: dedicated badge credentials receive 200 on config, 403 on owner recordings and tweet routes. Native ESP32 HTTPS remains unverified until Wi-Fi is restored.

## Review-only tweet drafting

- Deployed owner-only draft route/editor. The model receives only the explicitly approved selection and supplied personal take, not the rest of the recording. Public Bluesky/essay evidence supplies a limited voice guide.
- Compared real Workers AI outputs; current draft model is Qwen3-30B. Live synthetic-input inference produced three length/format/reference-validated candidates. Voice remains a human review/calibration task; do not claim a perfect replica or mechanically guaranteed factuality.
- Native browser fixture flows verified generation and private edit saving with checked postconditions: `d85db8d5-40ac-40e9-9d09-a3e6f1d99cf2`, `35644a6a-550c-4688-8b0d-6e16fefd6eec`. Earlier reload restored the edit (`b9894fce-71d3-4991-a783-fe973816fd3b`). Fixture evidence is not a production owner-login claim.
- Fourteen Node tests + TypeScript check pass: auth, receipts/races/gaps/deletion/expiry, drafts/secret guards, serialized autosave, revision conflicts and history changes. Edits may add human-verified links/mentions/quotes; model-generated references are more restricted.
- No clipboard changes, X-composer opening or posting occurred during unattended verification. No posting credentials exist. `.tools/draft-ui-receipt.json`, `.tools/voice-smoke-result.json`.

## Remaining owner gates and operational risks

- Restore a reachable hotspot/network, complete phone Wi-Fi provisioning, then explicitly capture/review a short consented badge-to-cloud clip. Full long live-mic, real network outage recovery, battery endurance and second-tailnet-sender checks remain unperformed.
- Check the actual Workers AI plan/allowance before long continuous speech. Free allowance is 10,000 neurons/day; published Whisper Turbo pricing implies roughly $0.17 ASR usage for four hours with the current context overlap, before free allocation and other costs. Plan lookup was unauthorized; no upgrade was made. Sparse synthetic speech does not prove this quota.
- Revoke the temporary **account-owned setup administration API token**. It remains active without expiry, scoped self-revocation returned 403, and no signed-in dashboard was available. Its local secret file was deleted; its identifier/status remain privately in `.tools/setup-token-status.json`. The badge's separate Access service credential is still needed.
- Seventy-three non-ignored project files were checked for the dedicated credential value: no match. Credentials, binaries, dependencies and build directories are ignored. No source was pushed publicly.

## Follow-up: setup exit and rejected recording start

- User's physical log showed Wi-Fi ready followed by optional Riker startup failing with OS error12. That returned from the function owning the Wi-Fi manager, leaving a visible setup screen with no controller to process Exit.
- The exact failure was reproduced with `tools/check-wifi.py --no-reset --via-display --start-failure`: before the fix, `WIFI_AP_CLOSED` timed out. After the fix, GUI-task open/exit, actual sprite QR finder-pattern checks, AP form and CSRF checks pass across repeated cycles after the same injected failure. Receipts `.tools/wifi-failure-before.log` / `wifi-failure-after.log`.
- Wi-Fi now stays in its own persistent control loop through optional clock/Riker errors. Riker startup waits until setup closes; its three stacks are reserved before network work starts, and partial startup is cancelled/joined on allocation failure. Recorder-worker startup failure also does not disable Wi-Fi setup. A host regression verifies partial workers never run and a later allocation can succeed.
- User then pressed Orange to record. Physical trace confirmed **A → action3 (Start)**, followed by `connect Wi-Fi and synchronize time first`, before microphone capture. This was rejected admission, not a button mapping or microphone failure.
- Updated firmware distinguishes disconnected Wi-Fi, pending time sync, and an open setup session. It shows **Not started** with the cause and Settings/Back/Retry controls instead of implying recorded audio stopped. Current on-device diagnostic specifically reports **No Wi-Fi**.
- `tools/check-recording-start.py` passed against the real recorder path with generated-data mode: no microphone requested, admission rejected, specific error plus `RECORDER_PHASE 10` reached. Nine Rust tests and C++ control-policy assertions pass. Temporary button/action trace logging removed; phase/error diagnostics retained for regression checks.
- Latest app-only flashed/read-back image SHA-256: `945453681ac4a611c0158cc5b67afc419a66249a049226c260d956894c41d6c4`. This is newer than the earlier soak images. Internet-connected native capture remains unverified; hotel captive login is not implemented.
- Automatic ongoing tweet suggestions/voice-editor work is **partial and not deployed** after a builder timeout. Production still runs the previously verified manual draft flow. Do not treat local unfinished cloud changes as released.



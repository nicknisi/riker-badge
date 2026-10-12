# Riker badge

## Surfaces and purpose

- **Badge:** ESP32-S3 M5Stack StopWatch C152, advertised 466×466 round AMOLED (driver exposes 468×468). Whimsical personal name/LinkedIn badge, read-only Riker pet, explicit-start online recorder.
- **Phone web reader:** private `https://transcribe.nisi.sh`, for transcripts, notes/actions, deletion/retry and review-only tweet drafting. Native HTML controls; no posting account integration.

## Confirmed preferences

Nick’s existing pixel-avatar assets: folded-arms full-body waiting GIF, idle peeker, completion hop, boop/haptic, gentle tilt, calm mode. LinkedIn: `https://linkedin.com/in/nicknisi`. Orange/yellow is A; Blue is B. Keep explanations in those colors.

Hours-long capture may depend on Wi-Fi/hotspot and USB power, but not an awake Mac. Audio retained seven days; text retained until deleted. Record only after consent and an explicit physical start. No room recording during unattended tests. Tweet drafts must be editable, grounded in explicitly shareable material and Nick’s supplied take, never automatically posted. Public writing is limited voice evidence, not permission to invent opinions, experiences or quotes.

## Architecture boundaries

Native Tailscale transports only authenticated Riker state words. Direct certificate-verified HTTPS transports immutable numbered audio chunks to Cloudflare Access/Worker/R2/D1/Queues/AI. A dedicated badge service token writes audio/status only; owner sign-in reads/manages content. Four audio buffers bound RAM and stop capture visibly when full. Recovery closes interrupted sessions without restarting capture. Phone Wi-Fi setup is temporary, AP-scoped, nonce-protected and mutually exclusive with microphone ownership.

## Preserve

Original flash layout, bootloader, conference-profile filesystem and recovery. Verified original backup: `~/Documents/badge-backups/stopwatch-20261006-133026/`. No erase-all, security-eFuse changes, secrets in tracked source, private Riker details on the badge, automatic social posting, or unapproved subscription upgrades.

## Evidence and limits

Human-confirmed: animated avatar, live state/timeout, LinkedIn phone scan, physical view/boop interactions, and across-room thirty-second microphone quality. Automated checks and synthetic real-time tests cover separate firmware/cloud paths; they do not prove hours-long live microphone capture. Current saved hotspot rejects the badge, including known-good firmware; physical phone Wi-Fi save/rejoin and actual badge-to-cloud capture are still field gates. Owner email sign-in, final voice calibration, battery endurance, a second real tailnet sender and full continuous-speech model quotas remain unverified. See `VALIDATION.md` for exact receipts and distinctions.

Replication-game functionality is deferred. The Riker bridge is a Mac session process, not a startup service; recording is independent of it.

export const RATE = 16_000;
export const CHUNK_SAMPLES = RATE * 10;
export const CHUNKS_PER_MINUTE = 6; // 60s / 10s per chunk
export const MAX_AUTO_WINDOWS = 96; // bounded automatic suggestion windows per rolling day (≈8h at 5 min)
export const CONTEXT_SAMPLES = RATE * 2;
export const MAX_CHUNKS = 8 * 60 * 6; // bounded v1: eight hours per recording/day
export const MAX_SAMPLES = CHUNK_SAMPLES * MAX_CHUNKS;
export const AUDIO_DAYS = 7;
export const GRACE_MS = 10 * 60_000;
export const LEASE_MS = 20 * 60_000; // exceeds the queue invocation wall-clock budget
export const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
export const SHA = /^[0-9a-f]{64}$/;
export type Recording = {
  id: string;
  device_id: string;
  title: string;
  created_at: number;
  updated_at: number;
  expires_at: number;
  expected_chunks: number | null;
  total_samples: number | null;
  reason: string | null;
  sealed_at: number | null;
  status: string;
  notes: string | null;
  deleted_at: number | null;
};
export type Chunk = {
  recording_id: string;
  seq: number;
  object_key: string;
  sha256: string;
  samples: number;
  created_at: number;
  status: string;
  attempts: number;
  claim_token: string | null;
  lease_until: number;
  words: string | null;
  error: string | null;
};
export type SuggestionWindow = {
  recording_id: string;
  window_index: number;
  start_seq: number;
  end_seq: number;
  status: string;
  attempts: number;
  claim_token: string | null;
  lease_until: number;
  draft_id: string | null;
  created_at: number;
};
export type Word = { word: string; start: number; end: number };
export interface Env {
  DB: D1Database;
  AUDIO: R2Bucket;
  JOBS: Queue;
  AI: Ai;
  ACCESS_TEAM_DOMAIN: string;
  ACCESS_AUD: string;
  DEVICE_CLIENT_ID: string;
  OWNER_EMAIL: string;
  PUBLIC_ORIGIN: string;
  BUCKET_NAME: string;
}
export class HttpError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}
export function requireValue(
  condition: unknown,
  status = 400,
  message = "Invalid request",
): asserts condition {
  if (!condition) throw new HttpError(status, message);
}
export function validSequence(seq: number) {
  return Number.isInteger(seq) && seq >= 0 && seq < MAX_CHUNKS;
}
export function objectKey(id: string, seq: number) {
  return `audio/${id}/${String(seq).padStart(6, "0")}.pcm`;
}
export function parseKey(key: string): { id: string; seq: number } | null {
  const m = /^audio\/([^/]+)\/(\d{6})\.pcm$/.exec(key);
  return m && UUID.test(m[1]) && validSequence(Number(m[2]))
    ? { id: m[1], seq: Number(m[2]) }
    : null;
}
export async function digest(bytes: Uint8Array): Promise<string> {
  const result = await crypto.subtle.digest(
    "SHA-256",
    bytes as Uint8Array<ArrayBuffer>,
  );
  return Array.from(new Uint8Array(result), (b) =>
    b.toString(16).padStart(2, "0"),
  ).join("");
}
export async function readBounded(
  request: Request,
  limit: number,
): Promise<Uint8Array> {
  const length = request.headers.get("content-length");
  requireValue(
    length === null || (/^\d+$/.test(length) && Number(length) <= limit),
    413,
    "Request too large",
  );
  const reader = request.body?.getReader();
  if (!reader) return new Uint8Array();
  const parts: Uint8Array[] = [];
  let total = 0;
  let timedOut = false;
  const deadline = setTimeout(() => {
    timedOut = true;
    void reader.cancel("Upload timed out");
  }, 30_000);
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      total += next.value.byteLength;
      if (total > limit) {
        await reader.cancel();
        throw new HttpError(413, "Request too large");
      }
      parts.push(next.value);
    }
    requireValue(!timedOut, 408, "Upload timed out");
    if (length !== null)
      requireValue(total === Number(length), 400, "Incomplete upload");
    const output = new Uint8Array(total);
    let offset = 0;
    for (const part of parts) {
      output.set(part, offset);
      offset += part.length;
    }
    return output;
  } finally {
    clearTimeout(deadline);
    reader.releaseLock();
  }
}
export async function jsonBody(
  request: Request,
  limit = 4096,
): Promise<Record<string, unknown>> {
  requireValue(
    request.headers.get("content-type")?.split(";")[0] === "application/json",
    415,
    "JSON required",
  );
  try {
    const parsed = JSON.parse(
      new TextDecoder().decode(await readBounded(request, limit)),
    );
    requireValue(
      parsed && typeof parsed === "object" && !Array.isArray(parsed),
    );
    return parsed;
  } catch (error) {
    if (error instanceof HttpError) throw error;
    throw new HttpError(400, "Invalid JSON");
  }
}
export function pcmWav(pcm: Uint8Array): Uint8Array {
  const out = new Uint8Array(44 + pcm.length);
  const view = new DataView(out.buffer);
  for (const [offset, text] of [
    [0, "RIFF"],
    [8, "WAVE"],
    [12, "fmt "],
    [36, "data"],
  ] as const)
    out.set(new TextEncoder().encode(text), offset);
  view.setUint32(4, 36 + pcm.length, true);
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, RATE, true);
  view.setUint32(28, RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  view.setUint32(40, pcm.length, true);
  out.set(pcm, 44);
  return out;
}
export function selectWords(
  output: unknown,
  offset: number,
  start: number,
  end: number,
  windowSeconds = end - offset + 2,
): Word[] {
  const result = output as { text?: unknown; segments?: { words?: Word[] }[] };
  if (typeof result.text !== "string") throw new Error("ASR_INVALID_RESULT");
  const words = (result.segments ?? []).flatMap((s) => s.words ?? []);
  if (result.text.trim() && !words.length)
    throw new Error("ASR_WORD_TIMESTAMPS_MISSING");
  if (
    words.some(
      (w) =>
        typeof w.word !== "string" ||
        w.word.length > 200 ||
        !Number.isFinite(w.start) ||
        !Number.isFinite(w.end) ||
        w.start < 0 ||
        w.end < w.start ||
        w.end > windowSeconds + 0.25,
    )
  )
    throw new Error("ASR_INVALID_RESULT");
  return words
    .map((w) => ({
      word: w.word,
      start: w.start + offset,
      end: w.end + offset,
    }))
    .filter(
      (w) =>
        (w.start + w.end) / 2 >= start - 0.25 &&
        (w.start + w.end) / 2 < end + 0.25,
    );
}
export function orderedText(
  chunks: Chunk[],
  expected: number,
): { text: string; missing: number[] } {
  const rows = new Map(chunks.map((c) => [c.seq, c]));
  const missing: number[] = [];
  const lines: string[] = [];
  let previous: (Word & { seq: number }) | undefined;
  for (let seq = 0; seq < expected; seq++) {
    const chunk = rows.get(seq);
    if (!chunk || chunk.status !== "done") {
      missing.push(seq);
      lines.push(
        `[TRANSCRIPT GAP: ${seq * 10}s–${(seq + 1) * 10}s; ${chunk?.status ?? "not received"}]`,
      );
      previous = undefined;
      continue;
    }
    const kept: string[] = [];
    for (const word of JSON.parse(chunk.words ?? "[]") as Word[]) {
      const normalized = word.word.trim().toLowerCase();
      if (
        previous &&
        previous.seq !== seq &&
        normalized === previous.word.trim().toLowerCase() &&
        word.start < previous.end &&
        Math.abs(word.start - previous.start) < 0.3
      )
        continue;
      kept.push(word.word.trim());
      previous = { ...word, seq };
    }
    if (kept.length)
      lines.push(
        `[${Math.floor(seq / 6)}:${String((seq % 6) * 10).padStart(2, "0")}] ${kept.join(" ")}`,
      );
  }
  return { text: lines.join("\n"), missing };
}
export function textParts(text: string, size = 12_000): string[] {
  const parts: string[] = [];
  let current = "";
  for (const line of text.split("\n")) {
    if (current.length + line.length + 1 > size && current) {
      parts.push(current);
      current = "";
    }
    for (let offset = 0; offset < line.length; offset += size) {
      const piece = line.slice(offset, offset + size);
      if (current.length + piece.length + 1 > size) {
        parts.push(current);
        current = "";
      }
      current += piece + "\n";
    }
  }
  if (current.trim()) parts.push(current);
  return parts;
}

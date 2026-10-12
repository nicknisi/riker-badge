import { identity, type Identity } from "./auth.ts";
import {
  AUDIO_DAYS,
  CHUNK_SAMPLES,
  GRACE_MS,
  MAX_CHUNKS,
  MAX_SAMPLES,
  RATE,
  SHA,
  UUID,
  HttpError,
  digest,
  jsonBody,
  objectKey,
  orderedText,
  pcmWav,
  readBounded,
  requireValue,
  type Chunk,
  type Env,
  type Recording,
} from "./model.ts";
import { runQueue, reconcile } from "./jobs.ts";
import { PAGE, SCRIPT } from "./view.ts";
import { draftRoute, voiceRoute } from "./drafts.ts";

export async function recording(env: Env, id: string): Promise<Recording> {
  requireValue(UUID.test(id));
  const row = await env.DB.prepare("SELECT * FROM recordings WHERE id=?")
    .bind(id)
    .first<Recording>();
  requireValue(row && !row.deleted_at, 404, "Recording not found");
  return row;
}
export async function chunks(env: Env, id: string): Promise<Chunk[]> {
  return (
    await env.DB.prepare(
      "SELECT * FROM chunks WHERE recording_id=? ORDER BY seq",
    )
      .bind(id)
      .all<Chunk>()
  ).results;
}
function requireDevice(owner: Identity, row?: Recording) {
  requireValue(
    owner.role === "device" && (!row || row.device_id === owner.id),
    403,
    "Badge ingestion only",
  );
}
function liveAudio(row: Recording) {
  requireValue(row.expires_at > Date.now(), 410, "Original audio has expired");
}
async function requestJob(env: Env, id: string) {
  // D1 is the durable source of pending work. Cron recovers a failed notification send.
  try {
    await env.JOBS.send({ kind: "recording", id });
  } catch {
    console.warn("job_notification_failed");
  }
}
async function create(request: Request, env: Env, actor: Identity) {
  requireDevice(actor);
  const body = await jsonBody(request);
  requireValue(typeof body.id === "string" && UUID.test(body.id));
  const title = body.title === undefined ? "Meeting" : body.title;
  requireValue(
    typeof title === "string" &&
      title.trim().length > 0 &&
      title.length <= 120 &&
      !/[\u0000-\u001f\u007f]/.test(title),
  );
  const prior = await env.DB.prepare("SELECT * FROM recordings WHERE id=?")
    .bind(body.id)
    .first<Recording>();
  if (prior) {
    requireValue(
      prior.device_id === actor.id && !prior.deleted_at,
      409,
      "Recording ID unavailable",
    );
    return Response.json({
      id: prior.id,
      status: prior.status,
      expires_at: prior.expires_at,
    });
  }
  const now = Date.now();
  const result = await env.DB.prepare(
    `INSERT INTO recordings(id,device_id,title,created_at,updated_at,expires_at)
    SELECT ?,?,?,?,?,? WHERE (SELECT COUNT(*) FROM recordings WHERE created_at>?)<12`,
  )
    .bind(
      body.id,
      actor.id,
      title.trim(),
      now,
      now,
      now + AUDIO_DAYS * 86400_000,
      now - 86400_000,
    )
    .run();
  requireValue(result.meta.changes, 429, "Daily recording limit reached");
  return Response.json(
    { id: body.id, status: "open", expires_at: now + AUDIO_DAYS * 86400_000 },
    { status: 201 },
  );
}
async function putChunk(
  env: Env,
  key: string,
  pcm: Uint8Array,
  hash: string,
  id: string,
  seq: number,
) {
  const stored = await env.AUDIO.put(key, pcm, {
    onlyIf: { etagDoesNotMatch: "*" },
    sha256: hash,
    httpMetadata: { contentType: "application/octet-stream" },
    customMetadata: {
      sha256: hash,
      samples: String(pcm.length / 2),
      recording: id,
      seq: String(seq),
    },
  });
  if (!stored) {
    const existing = await env.AUDIO.head(key);
    requireValue(
      existing?.customMetadata?.sha256 === hash && existing.size === pcm.length,
      409,
      "Stored chunk conflict",
    );
  }
}
async function upload(
  request: Request,
  env: Env,
  actor: Identity,
  row: Recording,
  seq: number,
) {
  requireDevice(actor, row);
  liveAudio(row);
  requireValue(Number.isInteger(seq) && seq >= 0 && seq < MAX_CHUNKS);
  requireValue(
    request.headers.get("content-type")?.split(";")[0] ===
      "application/octet-stream",
    415,
    "16 kHz mono PCM16LE required",
  );
  const hash = request.headers.get("x-audio-sha256") ?? "";
  requireValue(SHA.test(hash));
  const pcm = await readBounded(request, CHUNK_SAMPLES * 2);
  requireValue(pcm.length > 0 && pcm.length % 2 === 0);
  const samples = pcm.length / 2;
  requireValue((await digest(pcm)) === hash, 422, "Checksum mismatch");
  if (row.expected_chunks !== null) {
    requireValue(
      seq < row.expected_chunks &&
        samples ===
          (seq === row.expected_chunks - 1
            ? row.total_samples! - seq * CHUNK_SAMPLES
            : CHUNK_SAMPLES),
      409,
      "Does not match sealed manifest",
    );
  }
  const key = objectKey(row.id, seq);
  let entry = await env.DB.prepare(
    "SELECT * FROM chunks WHERE recording_id=? AND seq=?",
  )
    .bind(row.id, seq)
    .first<Chunk>();
  if (entry) {
    requireValue(
      entry.sha256 === hash && entry.samples === samples,
      409,
      "Chunk identity conflict",
    );
    if (["pending", "processing", "done", "failed"].includes(entry.status)) {
      let existing = await env.AUDIO.head(key);
      const restored = !existing;
      if (restored) {
        await putChunk(env, key, pcm, hash, row.id, seq);
        existing = await env.AUDIO.head(key);
      }
      try {
        const current = await recording(env, row.id);
        liveAudio(current);
      } catch (error) {
        if (restored) await env.AUDIO.delete(key);
        throw error;
      }
      requireValue(
        existing?.customMetadata?.sha256 === hash &&
          existing.size === pcm.length,
        409,
        "Stored chunk conflict",
      );
      return Response.json({ saved: true, seq, sha256: hash });
    }
  }
  requireValue(
    row.status === "open" || row.status === "sealed",
    409,
    "Recording is closed",
  );
  requireValue(
    row.sealed_at === null || Date.now() < row.sealed_at + GRACE_MS,
    409,
    "Late-upload window closed",
  );
  if (!entry) {
    // Reserve identity BEFORE object creation, making every possible orphan discoverable in D1.
    // Reservations count toward the budget so concurrent failing PUTs cannot bypass it; same-seq retries do not recount.
    const result = await env.DB.prepare(
      `INSERT OR IGNORE INTO chunks(recording_id,seq,object_key,sha256,samples,created_at)
      SELECT ?,?,?,?,?,? WHERE EXISTS(SELECT 1 FROM recordings WHERE id=? AND deleted_at IS NULL AND status IN ('open','sealed') AND expires_at>?
        AND (expected_chunks IS NULL OR (?<expected_chunks AND ?=CASE WHEN ?=expected_chunks-1 THEN total_samples-?*? ELSE ? END))
        AND (sealed_at IS NULL OR sealed_at+?>?))
      AND COALESCE((SELECT SUM(samples) FROM chunks WHERE created_at>?),0)+?<=?`,
    )
      .bind(
        row.id,
        seq,
        key,
        hash,
        samples,
        Date.now(),
        row.id,
        Date.now(),
        seq,
        samples,
        seq,
        seq,
        CHUNK_SAMPLES,
        CHUNK_SAMPLES,
        GRACE_MS,
        Date.now(),
        Date.now() - 86400_000,
        samples,
        MAX_SAMPLES,
      )
      .run();
    entry = await env.DB.prepare(
      "SELECT * FROM chunks WHERE recording_id=? AND seq=?",
    )
      .bind(row.id, seq)
      .first<Chunk>();
    requireValue(entry, 429, "Daily audio limit reached or recording closed");
    requireValue(
      entry.sha256 === hash && entry.samples === samples,
      409,
      "Chunk identity conflict",
    );
    void result;
  }
  await putChunk(env, key, pcm, hash, row.id, seq);
  const result = await env.DB.prepare(
    `UPDATE chunks SET status='pending' WHERE recording_id=? AND seq=? AND status='receiving'
    AND EXISTS(SELECT 1 FROM recordings WHERE id=? AND deleted_at IS NULL AND status IN ('open','sealed') AND expires_at>?)`,
  )
    .bind(row.id, seq, row.id, Date.now())
    .run();
  if (!result.meta.changes) {
    const fresh = await env.DB.prepare(
      "SELECT r.deleted_at,r.status AS recording_status,c.status AS chunk_status FROM recordings r JOIN chunks c ON c.recording_id=r.id WHERE r.id=? AND c.seq=?",
    )
      .bind(row.id, seq)
      .first<{
        deleted_at: number | null;
        recording_status: string;
        chunk_status: string;
      }>();
    if (
      !fresh ||
      fresh.deleted_at ||
      fresh.chunk_status === "receiving" ||
      fresh.chunk_status === "deleted"
    ) {
      await env.AUDIO.delete(key);
      throw new HttpError(410, "Recording closed during upload");
    }
  }
  await env.DB.prepare(
    "UPDATE recordings SET updated_at=? WHERE id=? AND deleted_at IS NULL",
  )
    .bind(Date.now(), row.id)
    .run();
  await requestJob(env, row.id);
  // Only storage + durable receipt are acknowledged. ASR completion is a separate status.
  return Response.json({ saved: true, seq, sha256: hash });
}
async function seal(env: Env, row: Recording, body: Record<string, unknown>) {
  const n = body.expected_chunks,
    total = body.total_samples,
    reason = body.reason;
  requireValue(
    Number.isInteger(n) &&
      Number.isInteger(total) &&
      Number(n) >= 0 &&
      Number(n) <= MAX_CHUNKS &&
      Number(total) >= 0 &&
      Number(total) <= MAX_SAMPLES,
  );
  requireValue(
    Number(n) === Math.ceil(Number(total) / CHUNK_SAMPLES) &&
      (reason === "stopped" || reason === "interrupted"),
  );
  if (row.expected_chunks !== null) {
    requireValue(
      row.expected_chunks === n &&
        row.total_samples === total &&
        row.reason === reason,
      409,
      "Manifest conflict",
    );
    return Response.json({ status: row.status });
  }
  requireValue(row.status === "open", 409, "Recording is closed");
  const invalid = await env.DB.prepare(
    `SELECT COUNT(*) AS n FROM chunks WHERE recording_id=? AND
    (seq>=? OR (seq<? AND samples<>?) OR (seq=? AND samples<>?))`,
  )
    .bind(
      row.id,
      n,
      Number(n) - 1,
      CHUNK_SAMPLES,
      Number(n) - 1,
      Number(total) - (Number(n) - 1) * CHUNK_SAMPLES,
    )
    .first<{ n: number }>();
  requireValue(invalid?.n === 0, 409, "Existing chunks conflict with manifest");
  const result = await env.DB.prepare(
    `UPDATE recordings SET expected_chunks=?,total_samples=?,reason=?,sealed_at=?,status='sealed',updated_at=?
    WHERE id=? AND expected_chunks IS NULL AND deleted_at IS NULL AND status='open'
    AND NOT EXISTS(SELECT 1 FROM chunks WHERE recording_id=? AND (seq>=? OR (seq<? AND samples<>?) OR (seq=? AND samples<>?)))`,
  )
    .bind(
      n,
      total,
      reason,
      Date.now(),
      Date.now(),
      row.id,
      row.id,
      n,
      Number(n) - 1,
      CHUNK_SAMPLES,
      Number(n) - 1,
      Number(total) - (Number(n) - 1) * CHUNK_SAMPLES,
    )
    .run();
  if (!result.meta.changes)
    return seal(env, await recording(env, row.id), body);
  await requestJob(env, row.id);
  return Response.json({ status: "sealed", processing: true }, { status: 202 });
}
async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  requireValue(url.origin === env.PUBLIC_ORIGIN, 404, "Not found");
  const actor = await identity(request, env);
  if (actor.role === "owner" && !["GET", "HEAD"].includes(request.method))
    requireValue(
      request.headers.get("origin") === env.PUBLIC_ORIGIN,
      403,
      "Same-origin action required",
    );
  if (url.pathname.startsWith("/api/device/")) {
    requireDevice(actor);
    if (url.pathname === "/api/device/config" && request.method === "GET")
      return Response.json({
        sample_rate: RATE,
        channels: 1,
        format: "s16le",
        chunk_samples: CHUNK_SAMPLES,
        max_samples: MAX_SAMPLES,
        audio_retention_days: AUDIO_DAYS,
      });
    if (url.pathname === "/api/device/recordings" && request.method === "POST")
      return create(request, env, actor);
    const match =
      /^\/api\/device\/recordings\/([^/]+)(?:\/(chunks)\/(\d+)|\/(seal))?$/.exec(
        url.pathname,
      );
    if (match) {
      const row = await recording(env, match[1]);
      requireDevice(actor, row);
      if (match[2] === "chunks" && request.method === "PUT")
        return upload(request, env, actor, row, Number(match[3]));
      if (match[4] === "seal" && request.method === "POST") {
        liveAudio(row);
        return seal(env, row, await jsonBody(request));
      }
      if (!match[2] && !match[4] && request.method === "GET") {
        const counts = await env.DB.prepare(
          "SELECT status,COUNT(*) AS count FROM chunks WHERE recording_id=? GROUP BY status",
        )
          .bind(row.id)
          .all();
        const last = await env.DB.prepare(
          "SELECT seq,samples FROM chunks WHERE recording_id=? ORDER BY seq DESC LIMIT 1",
        )
          .bind(row.id)
          .first<{ seq: number; samples: number }>();
        return Response.json({
          id: row.id,
          status: row.status,
          expires_at: row.expires_at,
          expected_chunks: row.expected_chunks,
          last_chunk: last,
          counts: counts.results,
        });
      }
    }
    throw new HttpError(404, "Not found");
  }
  requireValue(actor.role === "owner", 403, "Owner access required");
  if (url.pathname === "/api/voice") return voiceRoute(request, env);
  const draftMatch = /^\/api\/recordings\/([^/]+)\/drafts(?:\/([^/]+))?$/.exec(
    url.pathname,
  );
  if (draftMatch) {
    requireValue(!draftMatch[2] || UUID.test(draftMatch[2]));
    return draftRoute(
      request,
      env,
      await recording(env, draftMatch[1]),
      draftMatch[2],
    );
  }
  if (url.pathname === "/" && request.method === "GET")
    return new Response(PAGE, {
      headers: { "content-type": "text/html; charset=utf-8" },
    });
  if (url.pathname === "/app.js" && request.method === "GET")
    return new Response(SCRIPT, {
      headers: { "content-type": "text/javascript; charset=utf-8" },
    });
  if (url.pathname === "/api/recordings" && request.method === "GET") {
    const rows = await env.DB.prepare(
      "SELECT id,title,created_at,status,total_samples,expires_at FROM recordings WHERE deleted_at IS NULL ORDER BY created_at DESC LIMIT 100",
    ).all();
    return Response.json(rows.results);
  }
  const match =
    /^\/api\/recordings\/([^/]+)(?:\/(notes|audio|close-partial|retry)(?:\/(\d+))?)?$/.exec(
      url.pathname,
    );
  if (!match) throw new HttpError(404, "Not found");
  const row = await recording(env, match[1]);
  if (!match[2] && request.method === "DELETE") {
    await env.DB.batch([
      env.DB.prepare(
        "UPDATE recordings SET deleted_at=?,title='',notes=NULL,status='deleted' WHERE id=?",
      ).bind(Date.now(), row.id),
      env.DB.prepare(
        "UPDATE chunks SET words=NULL,status='deleted',claim_token=NULL WHERE recording_id=?",
      ).bind(row.id),
      env.DB.prepare("DELETE FROM summaries WHERE recording_id=?").bind(row.id),
      env.DB.prepare("DELETE FROM tweet_drafts WHERE recording_id=?").bind(
        row.id,
      ),
      env.DB.prepare(
        "DELETE FROM suggestion_windows WHERE recording_id=?",
      ).bind(row.id),
    ]);
    await env.JOBS.send({ kind: "delete", id: row.id });
    return Response.json({ deleted: true }, { status: 202 });
  }
  if (match[2] === "retry" && request.method === "POST") {
    liveAudio(row);
    requireValue(
      ["partial", "notes-failed"].includes(row.status),
      409,
      "Only failed/partial results can be retried",
    );
    const failed = await env.DB.prepare(
      "SELECT COUNT(*) AS n FROM chunks WHERE recording_id=? AND status='failed'",
    )
      .bind(row.id)
      .first<{ n: number }>();
    const guard =
      "EXISTS(SELECT 1 FROM recordings WHERE id=? AND deleted_at IS NULL AND status IN ('partial','notes-failed'))";
    // One transaction: a concurrent retry sees the new sealed status and changes nothing.
    // Keep the previous notes visible until a replacement is successfully committed.
    const results = await env.DB.batch([
      env.DB.prepare(
        `UPDATE chunks SET status='pending',attempts=0,error=NULL,claim_token=NULL,lease_until=0 WHERE recording_id=? AND status='failed' AND ${guard}`,
      ).bind(row.id, row.id),
      failed?.n
        ? env.DB.prepare(
            `DELETE FROM summaries WHERE recording_id=? AND ${guard}`,
          ).bind(row.id, row.id)
        : env.DB.prepare(
            `UPDATE summaries SET attempts=0,error=NULL,claim_token=NULL,lease_until=0 WHERE recording_id=? AND output IS NULL AND ${guard}`,
          ).bind(row.id, row.id),
      env.DB.prepare(
        "UPDATE recordings SET status='sealed',sealed_at=?,updated_at=? WHERE id=? AND deleted_at IS NULL AND status IN ('partial','notes-failed')",
      ).bind(Date.now(), Date.now(), row.id),
    ]);
    requireValue(
      results.at(-1)?.meta.changes,
      409,
      "Already retrying or deleted",
    );
    await requestJob(env, row.id);
    return Response.json({ queued: true }, { status: 202 });
  }
  const entries = await chunks(env, row.id);
  if (match[2] === "close-partial" && request.method === "POST") {
    requireValue(row.expected_chunks === null, 409, "Already sealed");
    const last = entries.at(-1);
    return seal(env, row, {
      expected_chunks: last ? last.seq + 1 : 0,
      total_samples: last ? last.seq * CHUNK_SAMPLES + last.samples : 0,
      reason: "interrupted",
    });
  }
  if (match[2] === "audio" && request.method === "GET") {
    liveAudio(row);
    const seq = Number(match[3]);
    requireValue(Number.isInteger(seq));
    const entry = entries.find((c) => c.seq === seq);
    requireValue(entry, 404, "Audio not found");
    const object = await env.AUDIO.get(entry.object_key);
    requireValue(object, 410, "Audio expired");
    return new Response(
      pcmWav(
        new Uint8Array(await object.arrayBuffer()),
      ) as Uint8Array<ArrayBuffer>,
      { headers: { "content-type": "audio/wav" } },
    );
  }
  const extent = row.expected_chunks ?? (entries.at(-1)?.seq ?? -1) + 1;
  const transcript = orderedText(entries, extent);
  if (match[2] === "notes" && request.method === "GET") {
    const text = `# ${row.title}\n\nStatus: ${row.status}. AI notes are drafts; check against the transcript.\n\n${row.notes ?? "Notes are not ready."}\n\n## Transcript\n\n${transcript.text}`;
    return new Response(text, {
      headers: {
        "content-type": "text/plain; charset=utf-8",
        "content-disposition": `attachment; filename="${row.id}.md"`,
      },
    });
  }
  if (!match[2] && request.method === "GET")
    return Response.json({
      ...row,
      transcript: transcript.text,
      gaps: transcript.missing,
      chunks: entries.map((c) => ({
        seq: c.seq,
        samples: c.samples,
        status: c.status,
        error: c.error,
      })),
    });
  throw new HttpError(404, "Not found");
}
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    let response: Response;
    try {
      response = await route(request, env);
    } catch (error) {
      if (!(error instanceof HttpError)) console.error("request_failed");
      response = Response.json(
        {
          error:
            error instanceof HttpError
              ? error.message
              : "Request failed; safe to retry with the same identity",
        },
        { status: error instanceof HttpError ? error.status : 503 },
      );
    }
    const headers = new Headers(response.headers);
    headers.set("cache-control", "no-store");
    headers.set("x-content-type-options", "nosniff");
    headers.set("referrer-policy", "no-referrer");
    headers.set(
      "content-security-policy",
      "default-src 'self'; script-src 'self'; style-src 'unsafe-inline'; media-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    );
    return new Response(response.body, { status: response.status, headers });
  },
  queue: runQueue,
  async scheduled(_event: ScheduledController, env: Env) {
    await reconcile(env);
  },
} satisfies ExportedHandler<Env>;

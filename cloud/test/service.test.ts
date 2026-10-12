import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { readFileSync, readdirSync } from "node:fs";
import { generateKeyPair, exportJWK, SignJWT } from "jose";
import worker from "../src/index.ts";
import {
  CHUNK_SAMPLES,
  GRACE_MS,
  RATE,
  digest,
  orderedText,
  pcmWav,
  selectWords,
  type Env,
} from "../src/model.ts";
import { reconcile } from "../src/jobs.ts";

const { privateKey, publicKey } = await generateKeyPair("RS256");
const jwk = {
  ...(await exportJWK(publicKey)),
  kid: "fixture",
  alg: "RS256",
  use: "sig",
};
const originalFetch = globalThis.fetch;
globalThis.fetch = (async (input: string | URL | Request) => {
  const url =
    typeof input === "string"
      ? input
      : input instanceof URL
        ? input.href
        : input.url;
  assert.equal(
    url,
    "https://fixture.cloudflareaccess.com/cdn-cgi/access/certs",
  );
  return Response.json({ keys: [jwk] });
}) as typeof fetch;
test.after(() => {
  globalThis.fetch = originalFetch;
});
const origin = "https://transcribe.nisi.sh";
async function token(role: "owner" | "device", extra = {}) {
  return new SignJWT({
    type: "app",
    ...(role === "owner"
      ? { email: "owner@example.com", sub: "owner" }
      : { common_name: "badge.access", sub: "" }),
    ...extra,
  })
    .setProtectedHeader({ alg: "RS256", kid: "fixture" })
    .setIssuer("https://fixture.cloudflareaccess.com")
    .setAudience("fixture-aud")
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(privateKey);
}
const owner = await token("owner"),
  device = await token("device");
function fixture() {
  const db = new DatabaseSync(":memory:");
  for (const file of readdirSync(
    new URL("../migrations/", import.meta.url),
  ).sort())
    db.exec(
      readFileSync(new URL("../migrations/" + file, import.meta.url), "utf8"),
    );
  const objects = new Map<
    string,
    { bytes: Uint8Array; metadata: Record<string, string> }
  >();
  const pending: unknown[] = [];
  let calls = 0,
    losePut = false;
  let sqlHook: ((sql: string) => Promise<void>) | undefined;
  let putHook: (() => Promise<void>) | undefined;
  function statement(sql: string, args: unknown[] = []): any {
    const prepared = db.prepare(sql);
    return {
      bind: (...a: unknown[]) => statement(sql, a),
      first: async () => prepared.get(...(args as any[])) ?? null,
      all: async () => ({
        results: prepared.all(...(args as any[])),
        success: true,
        meta: {},
      }),
      run: async () => {
        if (sqlHook) await sqlHook(sql);
        const r = prepared.run(...(args as any[]));
        return {
          success: true,
          results: [],
          meta: { changes: Number(r.changes) },
        };
      },
    };
  }
  const env = {
    PUBLIC_ORIGIN: origin,
    ACCESS_TEAM_DOMAIN: "https://fixture.cloudflareaccess.com",
    ACCESS_AUD: "fixture-aud",
    OWNER_EMAIL: "owner@example.com",
    DEVICE_CLIENT_ID: "badge.access",
    BUCKET_NAME: "fixture-audio",
    DB: {
      prepare: (sql: string) => statement(sql),
      batch: async (items: any[]) => {
        db.exec("BEGIN");
        try {
          const values = [];
          for (const item of items) values.push(await item.run());
          db.exec("COMMIT");
          return values;
        } catch (e) {
          db.exec("ROLLBACK");
          throw e;
        }
      },
    },
    AUDIO: {
      put: async (key: string, bytes: Uint8Array, options: any) => {
        if (putHook) await putHook();
        if (objects.has(key) && options.onlyIf) return null;
        objects.set(key, {
          bytes: bytes.slice(),
          metadata: options.customMetadata,
        });
        if (losePut) {
          losePut = false;
          throw new Error("injected response loss after storage");
        }
        return { key };
      },
      head: async (key: string) => {
        const o = objects.get(key);
        return o
          ? { key, size: o.bytes.length, customMetadata: o.metadata }
          : null;
      },
      get: async (key: string) => {
        const o = objects.get(key);
        return o ? { arrayBuffer: async () => o.bytes.slice().buffer } : null;
      },
      delete: async (keys: string | string[]) => {
        for (const key of typeof keys === "string" ? [keys] : keys)
          objects.delete(key);
      },
      list: async (options: any) => ({
        objects: [...objects.keys()]
          .filter((k) => k.startsWith(options.prefix))
          .map((key) => ({ key })),
        truncated: false,
      }),
    },
    JOBS: {
      send: async (body: unknown) => {
        pending.push(body);
      },
    },
    AI: {
      run: async (model: string, input: any) => {
        calls++;
        if (input.response_format)
          return {
            response: JSON.stringify({
              drafts: [
                {
                  angle: "Observation",
                  text: "The useful part is finding what breaks outside the demo.",
                },
                {
                  angle: "Takeaway",
                  text: "A passing check is not the same thing as a useful tool.",
                },
                {
                  angle: "Question",
                  text: "What happens when the meeting recorder loses its connection halfway through?",
                },
              ],
            }),
          };
        return model.includes("whisper")
          ? {
              text: "synthetic",
              segments: [
                { words: [{ word: "synthetic", start: 2.1, end: 2.4 }] },
              ],
            }
          : {
              response:
                "## Summary\nA synthetic fixture.\n\n## Action items\nNone specified.",
            };
      },
    },
  } as unknown as Env;
  async function request(
    path: string,
    method = "GET",
    body?: unknown,
    role: "owner" | "device" = "device",
    headers: Record<string, string> = {},
  ) {
    const h = new Headers({
      "cf-access-jwt-assertion": role === "owner" ? owner : device,
      ...headers,
    });
    if (role === "owner" && method !== "GET") h.set("origin", origin);
    let data: any;
    if (body instanceof Uint8Array) {
      data = body;
      h.set("content-type", "application/octet-stream");
      h.set("x-audio-sha256", await digest(body));
    } else if (body !== undefined) {
      data = JSON.stringify(body);
      h.set("content-type", "application/json");
    }
    return worker.fetch(
      new Request(origin + path, { method, headers: h, body: data }) as any,
      env,
    );
  }
  async function drain() {
    let iterations = 0;
    while (pending.length) {
      assert.ok(iterations++ < 100, "queue should converge");
      const body = pending.shift();
      await worker.queue(
        {
          messages: [
            {
              body,
              ack() {},
              retry() {
                throw Error("unexpected retry");
              },
            },
          ],
        } as any,
        env,
      );
    }
  }
  return {
    env,
    db,
    objects,
    request,
    drain,
    pending,
    calls: () => calls,
    loseNextPut: () => {
      losePut = true;
    },
    beforeSQL: (hook: typeof sqlHook) => {
      sqlHook = hook;
    },
    beforePut: (hook: typeof putHook) => {
      putHook = hook;
    },
  };
}
async function create(f: ReturnType<typeof fixture>) {
  const id = crypto.randomUUID();
  assert.equal(
    (
      await f.request("/api/device/recordings", "POST", {
        id,
        title: "Synthetic test",
      })
    ).status,
    201,
  );
  return id;
}

test("Access signatures, audiences and roles fail closed", async () => {
  const f = fixture();
  assert.equal(
    (await worker.fetch(new Request(origin + "/") as any, f.env)).status,
    401,
  );
  assert.equal((await f.request("/api/recordings")).status, 403);
  assert.equal(
    (await f.request("/api/device/config", "GET", undefined, "owner")).status,
    403,
  );
  const wrong = await new SignJWT({
    type: "app",
    email: "owner@example.com",
    sub: "owner",
  })
    .setProtectedHeader({ alg: "RS256", kid: "fixture" })
    .setIssuer("https://fixture.cloudflareaccess.com")
    .setAudience("wrong")
    .setIssuedAt()
    .setExpirationTime("1h")
    .sign(privateKey);
  assert.equal(
    (
      await worker.fetch(
        new Request(origin + "/", {
          headers: { "cf-access-jwt-assertion": wrong },
        }) as any,
        f.env,
      )
    ).status,
    403,
  );
  assert.equal((await f.request("/", "GET", undefined, "owner")).status, 200);
});

test("durable receipt, retry conflict and sealed processing converge without duplicate text", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  pcm[0] = 1;
  f.loseNextPut();
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm))
      .status,
    503,
  );
  assert.equal(
    f.db.prepare("SELECT status FROM chunks").get()!.status,
    "receiving",
  );
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm))
      .status,
    200,
  );
  assert.equal(f.objects.size, 1);
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm))
      .status,
    200,
  );
  const changed = pcm.slice();
  changed[1] = 3;
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", changed))
      .status,
    409,
  );
  assert.equal(
    (
      await f.request(`/api/device/recordings/${id}/seal`, "POST", {
        expected_chunks: 1,
        total_samples: CHUNK_SAMPLES,
        reason: "stopped",
      })
    ).status,
    202,
  );
  await f.drain();
  const r = (await (
    await f.request(`/api/recordings/${id}`, "GET", undefined, "owner")
  ).json()) as any;
  assert.equal(r.status, "complete");
  assert.match(r.transcript, /synthetic/);
  assert.match(r.notes, /Summary/);
  const before = f.calls();
  f.pending.push({ kind: "recording", id });
  await f.drain();
  assert.equal(f.calls(), before);
  assert.equal(
    (
      await f.request(`/api/device/recordings/${id}/seal`, "POST", {
        expected_chunks: 2,
        total_samples: CHUNK_SAMPLES * 2,
        reason: "stopped",
      })
    ).status,
    409,
  );
});

test("missing sealed audio is partial, never complete; deletion cannot resurrect output", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm);
  await f.request(`/api/device/recordings/${id}/seal`, "POST", {
    expected_chunks: 2,
    total_samples: CHUNK_SAMPLES * 2,
    reason: "stopped",
  });
  await f.drain();
  assert.equal(
    f.db.prepare("SELECT status FROM recordings").get()!.status,
    "sealed",
  );
  f.db
    .prepare("UPDATE recordings SET sealed_at=?")
    .run(Date.now() - GRACE_MS - 1);
  f.pending.push({ kind: "recording", id });
  await f.drain();
  assert.equal(
    f.db.prepare("SELECT status FROM recordings").get()!.status,
    "partial",
  );
  assert.equal(f.calls(), 0, "digital silence must not hallucinate notes");
  assert.equal(
    (await f.request(`/api/recordings/${id}`, "DELETE", undefined, "owner"))
      .status,
    202,
  );
  await f.drain();
  assert.equal(f.objects.size, 0);
  f.pending.push({
    bucket: "fixture-audio",
    object: { key: `audio/${id}/000000.pcm` },
  });
  await f.drain();
  assert.equal(
    (await f.request(`/api/recordings/${id}`, "GET", undefined, "owner"))
      .status,
    404,
  );
  assert.equal(
    (await f.request("/api/device/recordings", "POST", { id })).status,
    409,
  );
  assert.equal(f.db.prepare("SELECT notes FROM recordings").get()!.notes, null);
});

test("expiry denies replay/read and reconciliation removes raw audio but preserves text", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm);
  await f.request(`/api/device/recordings/${id}/seal`, "POST", {
    expected_chunks: 1,
    total_samples: CHUNK_SAMPLES,
    reason: "stopped",
  });
  await f.drain();
  f.db.prepare("UPDATE recordings SET expires_at=?").run(Date.now() - 1);
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm))
      .status,
    410,
  );
  assert.equal(
    (
      await f.request(
        `/api/recordings/${id}/audio/0`,
        "GET",
        undefined,
        "owner",
      )
    ).status,
    410,
  );
  await reconcile(f.env);
  await f.drain();
  assert.equal(f.objects.size, 0);
  assert.ok(f.db.prepare("SELECT notes FROM recordings").get()!.notes);
});

test("CSRF, identity ambiguity and input limits are enforced", async () => {
  const f = fixture(),
    id = await create(f);
  for (const badOrigin of [null, "https://attacker.example"]) {
    const headers = new Headers({ "cf-access-jwt-assertion": owner });
    if (badOrigin) headers.set("origin", badOrigin);
    assert.equal(
      (
        await worker.fetch(
          new Request(origin + "/api/recordings/" + id, {
            method: "DELETE",
            headers,
          }) as any,
          f.env,
        )
      ).status,
      403,
    );
  }
  for (const badToken of [
    await token("device", { email: "owner@example.com" }),
    await token("owner", { email: "someone@example.com" }),
    await token("owner", { type: "org" }),
    await new SignJWT({ type: "app", email: "owner@example.com", sub: "owner" })
      .setProtectedHeader({ alg: "RS256", kid: "fixture" })
      .setIssuer("https://fixture.cloudflareaccess.com")
      .setAudience("fixture-aud")
      .setIssuedAt(1)
      .setExpirationTime(2)
      .sign(privateKey),
  ]) {
    assert.equal(
      (
        await worker.fetch(
          new Request(origin + "/", {
            headers: { "cf-access-jwt-assertion": badToken },
          }) as any,
          f.env,
        )
      ).status,
      403,
    );
  }
  for (const title of ["", "   ", "a".repeat(121), "line\nline"])
    assert.equal(
      (
        await f.request("/api/device/recordings", "POST", {
          id: crypto.randomUUID(),
          title,
        })
      ).status,
      400,
    );
  assert.equal(
    (await f.request("/api/device/recordings", "POST", { id: "../bad" }))
      .status,
    400,
  );
  const path = origin + "/api/device/recordings/" + id + "/chunks/0";
  async function raw(body: Uint8Array, extra: Record<string, string>) {
    return worker.fetch(
      new Request(path, {
        method: "PUT",
        body: body as any,
        headers: { "cf-access-jwt-assertion": device, ...extra },
      }) as any,
      f.env,
    );
  }
  assert.equal(
    (await raw(new Uint8Array(2), { "content-type": "text/plain" })).status,
    415,
  );
  assert.equal(
    (
      await raw(new Uint8Array(2), {
        "content-type": "application/octet-stream",
      })
    ).status,
    400,
  );
  assert.equal(
    (
      await raw(new Uint8Array(2), {
        "content-type": "application/octet-stream",
        "x-audio-sha256": "0".repeat(64),
      })
    ).status,
    422,
  );
  assert.equal(
    (
      await raw(new Uint8Array(CHUNK_SAMPLES * 2 + 2), {
        "content-type": "application/octet-stream",
        "x-audio-sha256": "0".repeat(64),
      })
    ).status,
    413,
  );
});

function gate() {
  let reached!: () => void, release!: () => void;
  const waiting = new Promise<void>((r) => {
    reached = r;
  });
  const blocked = new Promise<void>((r) => {
    release = r;
  });
  return { waiting, blocked, reached, release };
}
test("upload racing seal or delete cannot escape the manifest or resurrect audio", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm);
  const g = gate();
  f.beforeSQL(async (sql) => {
    if (sql.includes("INSERT OR IGNORE INTO chunks")) {
      f.beforeSQL(undefined);
      g.reached();
      await g.blocked;
    }
  });
  const uploading = f.request(
    `/api/device/recordings/${id}/chunks/1`,
    "PUT",
    pcm,
  );
  await g.waiting;
  assert.equal(
    (
      await f.request(`/api/device/recordings/${id}/seal`, "POST", {
        expected_chunks: 1,
        total_samples: CHUNK_SAMPLES,
        reason: "stopped",
      })
    ).status,
    202,
  );
  g.release();
  assert.notEqual((await uploading).status, 200);
  assert.equal(
    f.db.prepare("SELECT COUNT(*) AS n FROM chunks WHERE seq=1").get()!.n,
    0,
  );
  const second = await create(f),
    d = gate();
  f.beforePut(async () => {
    f.beforePut(undefined);
    d.reached();
    await d.blocked;
  });
  const late = f.request(
    `/api/device/recordings/${second}/chunks/0`,
    "PUT",
    pcm,
  );
  await d.waiting;
  await f.request(`/api/recordings/${second}`, "DELETE", undefined, "owner");
  d.release();
  assert.equal((await late).status, 410);
  assert.ok(![...f.objects.keys()].some((k) => k.includes(second)));
});

test("seal racing a reserved chunk rejects the now-incomplete manifest", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm);
  const g = gate();
  f.beforeSQL(async (sql) => {
    if (sql.startsWith("UPDATE recordings SET expected_chunks")) {
      f.beforeSQL(undefined);
      g.reached();
      await g.blocked;
    }
  });
  const sealing = f.request(`/api/device/recordings/${id}/seal`, "POST", {
    expected_chunks: 1,
    total_samples: CHUNK_SAMPLES,
    reason: "stopped",
  });
  await g.waiting;
  await f.request(`/api/device/recordings/${id}/chunks/1`, "PUT", pcm);
  g.release();
  assert.equal((await sealing).status, 409);
  assert.equal(
    f.db.prepare("SELECT expected_chunks FROM recordings WHERE id=?").get(id)!
      .expected_chunks,
    null,
  );
});

test("complete notes cannot be reset, and matching lost raw objects can be repaired", async () => {
  const f = fixture(),
    id = await create(f),
    pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm);
  await f.request(`/api/device/recordings/${id}/seal`, "POST", {
    expected_chunks: 1,
    total_samples: CHUNK_SAMPLES,
    reason: "stopped",
  });
  await f.drain();
  const notes = f.db.prepare("SELECT notes FROM recordings").get()!.notes;
  assert.equal(
    (await f.request(`/api/recordings/${id}/retry`, "POST", {}, "owner"))
      .status,
    409,
  );
  assert.equal(
    f.db.prepare("SELECT notes FROM recordings").get()!.notes,
    notes,
  );
  f.objects.clear();
  assert.equal(
    (await f.request(`/api/device/recordings/${id}/chunks/0`, "PUT", pcm))
      .status,
    200,
  );
  assert.equal(f.objects.size, 1);
});

test("drafts require owner approval, preserve edits, and disappear on recording deletion", async () => {
  const f = fixture(),
    id = await create(f),
    path = `/api/recordings/${id}/drafts`;
  const source =
    "A synthetic discussion about evaluating developer tools and handling recorder outages.";
  assert.equal(
    (await f.request(path, "POST", { source, shareable: true })).status,
    403,
  );
  assert.equal(
    (await f.request(path, "POST", { source, shareable: false }, "owner"))
      .status,
    400,
  );
  assert.equal(f.calls(), 0);
  const response = await f.request(
    path,
    "POST",
    { source, shareable: true, take: "" },
    "owner",
  );
  assert.equal(response.status, 200);
  const result = (await response.json()) as any;
  assert.equal(result.posted, false);
  assert.equal(result.drafts.length, 3);
  const edited = ["My own edited draft.", "Another draft.", "One more draft."];
  assert.equal(
    (
      await f.request(
        path + "/" + result.id,
        "PATCH",
        { texts: edited, revision: 0 },
        "owner",
      )
    ).status,
    200,
  );
  const saved = (await (
    await f.request(path, "GET", undefined, "owner")
  ).json()) as any;
  assert.equal(JSON.parse(saved.generations[0].payload)[0].text, edited[0]);
  assert.equal(
    (
      await f.request(
        path + "/" + result.id,
        "PATCH",
        { texts: ["stale", "stale", "stale"], revision: 0 },
        "owner",
      )
    ).status,
    409,
  );
  assert.equal(
    (
      await f.request(
        path + "/" + result.id,
        "PATCH",
        { texts: ["cfast_fake-credential", "ok", "ok"], revision: 1 },
        "owner",
      )
    ).status,
    400,
  );
  assert.equal(
    (
      await f.request(
        path + "/" + result.id,
        "PATCH",
        {
          texts: [
            "Verified link https://example.com",
            "A verified @handle",
            'A verified "quote"',
          ],
          revision: 1,
        },
        "owner",
      )
    ).status,
    200,
  );
  await f.request("/api/recordings/" + id, "DELETE", undefined, "owner");
  assert.equal(
    f.db.prepare("SELECT COUNT(*) AS n FROM tweet_drafts").get()!.n,
    0,
  );
});

async function fillChunks(
  f: ReturnType<typeof fixture>,
  id: string,
  count: number,
) {
  const pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  pcm[0] = 1; // non-silent so the synthetic ASR returns a word per chunk
  for (let seq = 0; seq < count; seq++)
    assert.equal(
      (await f.request(`/api/device/recordings/${id}/chunks/${seq}`, "PUT", pcm))
        .status,
      200,
    );
  assert.equal(
    (
      await f.request(`/api/device/recordings/${id}/seal`, "POST", {
        expected_chunks: count,
        total_samples: CHUNK_SAMPLES * count,
        reason: "stopped",
      })
    ).status,
    202,
  );
  await f.drain();
}

test("automatic suggestions claim windows once, dedupe repeats, and never block notes", async () => {
  const f = fixture();
  // Shrink the window so a short synthetic recording spans several windows + a tail.
  f.db.prepare("UPDATE voice_settings SET cadence_minutes=1").run();
  const id = await create(f);
  await fillChunks(f, id, 13); // windows [0,6) [6,12) full + tail [12,13)
  const recording = (await (
    await f.request(`/api/recordings/${id}`, "GET", undefined, "owner")
  ).json()) as any;
  assert.equal(recording.status, "complete");
  assert.match(recording.notes, /Summary/);
  const windows = f.db
    .prepare(
      "SELECT status,COUNT(*) AS n FROM suggestion_windows WHERE recording_id=? GROUP BY status",
    )
    .all(id) as any[];
  const byStatus = Object.fromEntries(windows.map((w) => [w.status, w.n]));
  assert.equal(byStatus.done, 1, "one window yields a suggestion");
  assert.equal(byStatus.skipped, 2, "duplicate + tiny-tail windows are skipped");
  const autos = f.db
    .prepare(
      "SELECT payload,approved,window_start,window_end,kind FROM tweet_drafts WHERE kind='auto'",
    )
    .all() as any[];
  assert.equal(autos.length, 1, "exact duplicate ideas are not re-stored");
  assert.equal(autos[0].approved, 0, "automatic suggestions are private, not approved");
  assert.equal(autos[0].window_start, 0);
  assert.equal(autos[0].window_end, 60);
  assert.equal(JSON.parse(autos[0].payload).length, 1);
  // History list is metadata-only: no giant source snapshot per row.
  const list = (await (
    await f.request(`/api/recordings/${id}/drafts`, "GET", undefined, "owner")
  ).json()) as any;
  const autoRow = list.generations.find((g: any) => g.kind === "auto");
  assert.ok(autoRow && !("source_text" in autoRow));
  // Source fetched on demand for the selected draft only.
  const source = (await (
    await f.request(
      `/api/recordings/${id}/drafts/${autoRow.id}`,
      "GET",
      undefined,
      "owner",
    )
  ).json()) as any;
  assert.match(source.source_text, /synthetic/);
  // Re-processing does not duplicate model calls or windows.
  const windowCount = () =>
    (f.db.prepare("SELECT COUNT(*) AS n FROM suggestion_windows").get() as any).n;
  const before = windowCount();
  f.pending.push({ kind: "recording", id });
  await f.drain();
  assert.equal(windowCount(), before);
});

test("approval gates export for automatic suggestions without touching manual flow", async () => {
  const f = fixture();
  f.db.prepare("UPDATE voice_settings SET cadence_minutes=1").run();
  const id = await create(f);
  await fillChunks(f, id, 6);
  const auto = f.db
    .prepare("SELECT id,revision FROM tweet_drafts WHERE kind='auto'")
    .get() as any;
  assert.ok(auto, "an automatic suggestion was generated");
  // Owner edits and approves via CAS PATCH.
  const approveRes = await f.request(
    `/api/recordings/${id}/drafts/${auto.id}`,
    "PATCH",
    { texts: ["A reviewed automatic suggestion."], revision: auto.revision, approve: true },
    "owner",
  );
  assert.equal(approveRes.status, 200);
  assert.equal((await approveRes.json()).approved, true);
  assert.equal(
    (f.db.prepare("SELECT approved FROM tweet_drafts WHERE id=?").get(auto.id) as any)
      .approved,
    1,
  );
});

test("disabled voice settings suppress automatic suggestions entirely", async () => {
  const f = fixture();
  f.db
    .prepare("UPDATE voice_settings SET auto_enabled=0,cadence_minutes=1")
    .run();
  const id = await create(f);
  await fillChunks(f, id, 6);
  assert.equal(
    (f.db.prepare("SELECT COUNT(*) AS n FROM suggestion_windows").get() as any).n,
    0,
  );
  assert.equal(
    (f.db.prepare("SELECT COUNT(*) AS n FROM tweet_drafts WHERE kind='auto'").get() as any).n,
    0,
  );
});

test("automatic windows are bounded to the daily budget", async () => {
  const f = fixture();
  f.db.prepare("UPDATE voice_settings SET cadence_minutes=1").run();
  // Exhaust the rolling-day budget with unrelated windows.
  for (let i = 0; i < 96; i++)
    f.db
      .prepare(
        "INSERT INTO suggestion_windows(recording_id,window_index,start_seq,end_seq,status,created_at) VALUES('budget',?,?,?,'skipped',?)",
      )
      .run(i, i, i + 1, Date.now());
  const id = await create(f);
  await fillChunks(f, id, 6);
  assert.equal(
    (f.db
      .prepare("SELECT COUNT(*) AS n FROM suggestion_windows WHERE recording_id=?")
      .get(id) as any).n,
    0,
    "no new windows created once the daily budget is spent",
  );
});

test("a concurrent delete cannot resurrect an automatic suggestion", async () => {
  const f = fixture();
  f.db.prepare("UPDATE voice_settings SET cadence_minutes=1").run();
  const id = await create(f);
  const pcm = new Uint8Array(CHUNK_SAMPLES * 2);
  pcm[0] = 1;
  for (let seq = 0; seq < 6; seq++)
    await f.request(`/api/device/recordings/${id}/chunks/${seq}`, "PUT", pcm);
  await f.request(`/api/device/recordings/${id}/seal`, "POST", {
    expected_chunks: 6,
    total_samples: CHUNK_SAMPLES * 6,
    reason: "stopped",
  });
  const g = gate();
  f.beforeSQL(async (sql) => {
    if (sql.includes("INSERT INTO tweet_drafts") && sql.includes("'auto'")) {
      f.beforeSQL(undefined);
      g.reached();
      await g.blocked;
    }
  });
  const draining = f.drain();
  await g.waiting;
  await f.request(`/api/recordings/${id}`, "DELETE", undefined, "owner");
  g.release();
  await draining;
  assert.equal(
    (f.db.prepare("SELECT COUNT(*) AS n FROM tweet_drafts").get() as any).n,
    0,
    "the suggestion insert loses to the delete guard",
  );
  assert.equal(
    (f.db.prepare("SELECT COUNT(*) AS n FROM suggestion_windows").get() as any).n,
    0,
    "windows are removed with the recording",
  );
});

test("voice settings require owner auth, same-origin, and valid values", async () => {
  const f = fixture();
  // Device identity cannot read owner settings.
  assert.equal(
    (await f.request("/api/voice", "GET", undefined, "device")).status,
    403,
  );
  const current = (await (
    await f.request("/api/voice", "GET", undefined, "owner")
  ).json()) as any;
  assert.equal(current.auto_enabled, true);
  assert.equal(current.cadence_minutes, 5);
  assert.match(current.fallback, /Nick writes/);
  // CSRF: owner mutation without a same-origin Origin header is refused.
  assert.equal(
    (
      await worker.fetch(
        new Request(origin + "/api/voice", {
          method: "PUT",
          headers: {
            "cf-access-jwt-assertion": owner,
            "content-type": "application/json",
          },
          body: JSON.stringify({
            style_instructions: "",
            example_posts: "",
            auto_enabled: false,
            cadence_minutes: 5,
            revision: current.revision,
          }),
        }) as any,
        f.env,
      )
    ).status,
    403,
  );
  // Invalid cadence is rejected.
  assert.equal(
    (
      await f.request(
        "/api/voice",
        "PUT",
        {
          style_instructions: "",
          example_posts: "",
          auto_enabled: true,
          cadence_minutes: 7,
          revision: current.revision,
        },
        "owner",
      )
    ).status,
    400,
  );
  // Credential-pattern guard applies to custom style.
  assert.equal(
    (
      await f.request(
        "/api/voice",
        "PUT",
        {
          style_instructions: "cfast_fake-credential",
          example_posts: "",
          auto_enabled: true,
          cadence_minutes: 5,
          revision: current.revision,
        },
        "owner",
      )
    ).status,
    400,
  );
  // A valid update persists and bumps the revision (CAS).
  const ok = await f.request(
    "/api/voice",
    "PUT",
    {
      style_instructions: "Dry and specific.",
      example_posts: "ship small things",
      auto_enabled: false,
      cadence_minutes: 30,
      revision: current.revision,
    },
    "owner",
  );
  assert.equal(ok.status, 200);
  assert.equal((await ok.json()).revision, current.revision + 1);
  const reloaded = (await (
    await f.request("/api/voice", "GET", undefined, "owner")
  ).json()) as any;
  assert.equal(reloaded.auto_enabled, false);
  assert.equal(reloaded.cadence_minutes, 30);
  assert.equal(reloaded.style_instructions, "Dry and specific.");
  // Stale revision is refused.
  assert.equal(
    (
      await f.request(
        "/api/voice",
        "PUT",
        {
          style_instructions: "x",
          example_posts: "",
          auto_enabled: true,
          cadence_minutes: 5,
          revision: current.revision,
        },
        "owner",
      )
    ).status,
    409,
  );
});

test("word ownership, real repetitions and WAV format", () => {
  assert.throws(
    () => selectWords({ text: "un-timestamped" }, 0, 0, 10),
    /TIMESTAMPS/,
  );
  const words = selectWords(
    { text: "a", segments: [{ words: [{ word: "a", start: 2, end: 2.2 }] }] },
    8,
    10,
    20,
  );
  assert.equal(words[0].start, 10);
  const c = (seq: number, words: any[]) =>
    ({ seq, status: "done", words: JSON.stringify(words) }) as any;
  const joined = orderedText(
    [
      c(0, [
        { word: "very", start: 9.7, end: 9.9 },
        { word: "very", start: 9.91, end: 10.1 },
      ]),
      c(1, [
        { word: "very", start: 9.93, end: 10.1 },
        { word: "good", start: 10.2, end: 10.5 },
      ]),
    ],
    2,
  ).text;
  assert.equal(joined.match(/very/g)?.length, 2);
  const wav = pcmWav(new Uint8Array([1, 2, 3, 4]));
  assert.equal(new TextDecoder().decode(wav.slice(0, 4)), "RIFF");
  assert.equal(new DataView(wav.buffer).getUint32(24, true), RATE);
  assert.equal(wav.length, 48);
});

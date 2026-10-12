// Real Workers AI inference through the production draft generator, with local
// synthetic storage. This does not bypass production Access or post anything.
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { draftRoute } from "../src/drafts.ts";
import { weightedLength } from "../src/voice.ts";
import type { Env, Recording } from "../src/model.ts";
const context = JSON.parse(
  readFileSync("../.tools/cloudflare-context.json", "utf8"),
);
const auth = JSON.parse(
  execFileSync("pnpm", ["exec", "wrangler", "auth", "token", "--json"], {
    encoding: "utf8",
    env: { ...process.env, WRANGLER_SEND_METRICS: "false" },
  }),
);
const token = auth.token ?? auth.apiToken ?? auth.oauthToken;
if (!token) throw Error("No AI setup authentication");
const db = new DatabaseSync(":memory:");
for (const file of readdirSync("migrations").sort())
  db.exec(readFileSync("migrations/" + file, "utf8"));
const id = crypto.randomUUID();
db.prepare(
  "INSERT INTO recordings(id,device_id,title,created_at,updated_at,expires_at) VALUES(?,?,?,?,?,?)",
).run(
  id,
  "test",
  "Synthetic drafting fixture",
  Date.now(),
  Date.now(),
  Date.now() + 60000,
);
function prepared(sql: string, args: any[] = []): any {
  const q = db.prepare(sql);
  return {
    bind: (...a: any[]) => prepared(sql, a),
    run: async () => {
      const r = q.run(...args);
      return { meta: { changes: Number(r.changes) } };
    },
    all: async () => ({ results: q.all(...args) }),
    first: async () => q.get(...args) ?? null,
  };
}
const env = {
  DB: { prepare: (sql: string) => prepared(sql) },
  AI: {
    run: async (model: string, input: unknown) => {
      const response = await fetch(
        `https://api.cloudflare.com/client/v4/accounts/${context.account_id}/ai/run/${model}`,
        {
          method: "POST",
          headers: {
            Authorization: "Bearer " + token,
            "Content-Type": "application/json",
          },
          body: JSON.stringify(input),
        },
      );
      if (!response.ok)
        throw Error("Model request failed with HTTP " + response.status);
      const json = (await response.json()) as any;
      if (!json.success) throw Error("Model inference rejected");
      return json.result;
    },
  },
} as unknown as Env;
const request = new Request(
  "https://transcribe.nisi.sh/api/recordings/" + id + "/drafts",
  {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      shareable: true,
      source:
        "Synthetic scenario, not a real conference session: a demo worked only while its network connection was perfect. The useful improvement was persisting each audio chunk before acknowledging it, keeping a bounded retry buffer, and making interrupted recordings visible instead of pretending they were complete.",
      take: "I care a lot more about what happens when the connection drops than another perfect demo. That is the point of this synthetic test, not a claim that I attended a particular talk.",
    }),
  },
);
const result = await draftRoute(
  request as any,
  env,
  db.prepare("SELECT * FROM recordings").get() as unknown as Recording,
).catch((error) => {
  console.log(db.prepare("SELECT error,attempts FROM tweet_drafts").get());
  throw error;
});
const output = (await result.json()) as any;
if (output.posted !== false || output.drafts.length !== 3)
  throw Error("Draft contract failed");
writeFileSync(
  "../.tools/voice-smoke-result.json",
  JSON.stringify(
    {
      synthetic: true,
      posted: false,
      drafts: output.drafts,
      checked_at: new Date().toISOString(),
    },
    null,
    2,
  ),
);
for (const draft of output.drafts)
  console.log(
    draft.angle + " (" + weightedLength(draft.text) + "): " + draft.text,
  );
console.log(
  "PASS: real model returned three validated private drafts; nothing posted.",
);

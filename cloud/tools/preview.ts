// Loopback-only UI fixture. Not imported by/deployed with the Worker; no real data or auth.
import { createServer } from "node:http";
import { PAGE, SCRIPT } from "../src/view.ts";
const id = "12345678-1234-4123-8123-123456789abc";
let exists = true;
const generations: any[] = [];
const recording = {
  id,
  title: "UI fixture — synthetic conference notes",
  status: "complete",
  created_at: Date.now(),
  expires_at: Date.now() + 604800000,
  total_samples: 160000,
  expected_chunks: 1,
  gaps: [],
  chunks: [{ seq: 0, samples: 160000, status: "done" }],
  notes:
    "## Summary\nA synthetic discussion about testing tools and finding failures.\n\n## Decisions\nMeasure behavior instead of trusting a green check.",
  transcript:
    "[0:00] This is synthetic fixture text. A demo passing is not the same as a tool helping someone.\n[0:05] Test what happens when the connection disappears.",
};
createServer(async (req, res) => {
  const url = new URL(req.url!, "http://127.0.0.1:8788");
  let body = "";
  for await (const chunk of req) body += chunk;
  const json = (value: any, status = 200) => {
    res.writeHead(status, {
      "content-type": "application/json",
      "cache-control": "no-store",
    });
    res.end(JSON.stringify(value));
  };
  if (url.pathname === "/") {
    res.writeHead(200, { "content-type": "text/html" });
    res.end(PAGE);
    return;
  }
  if (url.pathname === "/app.js") {
    res.writeHead(200, { "content-type": "text/javascript" });
    res.end(SCRIPT);
    return;
  }
  if (url.pathname === "/api/recordings") {
    json(exists ? [recording] : []);
    return;
  }
  if (url.pathname === `/api/recordings/${id}`) {
    if (req.method === "DELETE") {
      exists = false;
      json({ deleted: true });
    } else json(recording);
    return;
  }
  if (url.pathname === `/api/recordings/${id}/drafts`) {
    if (req.method === "POST") {
      const input = JSON.parse(body);
      if (!input.shareable) {
        json({ error: "Mark the selection shareable" }, 400);
        return;
      }
      const drafts = [
        {
          angle: "Observation",
          text: "A passing demo is not the same as a useful tool.",
        },
        {
          angle: "Personal take",
          text: "I want the failure case in the demo, not hidden in the setup.",
        },
        {
          angle: "Question",
          text: "What happens when the recorder loses its connection halfway through?",
        },
      ];
      const g = {
        id: crypto.randomUUID(),
        revision: 0,
        created_at: Date.now(),
        source_text: input.source,
        personal_take: input.take,
        payload: JSON.stringify(drafts),
        error: null,
      };
      generations.unshift(g);
      json({
        id: g.id,
        revision: 0,
        created_at: g.created_at,
        drafts,
        posted: false,
      });
    } else json({ generations, voice_sources: [] });
    return;
  }
  if (
    url.pathname.startsWith(`/api/recordings/${id}/drafts/`) &&
    req.method === "PATCH"
  ) {
    const g = generations.find((g) => g.id === url.pathname.split("/").at(-1));
    if (!g) {
      json({ error: "missing" }, 404);
      return;
    }
    const input = JSON.parse(body);
    if (input.revision !== g.revision) {
      json({ error: "Draft changed elsewhere" }, 409);
      return;
    }
    const texts = input.texts;
    g.payload = JSON.stringify(
      JSON.parse(g.payload).map((d: any, i: number) => ({
        ...d,
        text: texts[i],
      })),
    );
    g.revision++;
    json({ saved: true, revision: g.revision });
    return;
  }
  json({ error: "Fixture route not implemented" }, 404);
}).listen(8788, "127.0.0.1", () =>
  console.log("UI fixture: http://127.0.0.1:8788"),
);

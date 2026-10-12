import test from "node:test";
import assert from "node:assert/strict";
import {
  parseDrafts,
  parseSuggestion,
  weightedLength,
  effectiveStyle,
  exampleBlock,
  VOICE,
} from "../src/voice.ts";
const values = [
  { angle: "A", text: "A concrete observation." },
  { angle: "B", text: "A different useful angle." },
  { angle: "C", text: "A real question worth asking?" },
];
test("draft boundaries block unverified references, secrets, fabricated first person and oversized posts", () => {
  assert.equal(parseDrafts({ drafts: values }, false).length, 3);
  assert.equal(weightedLength("abc"), 3);
  assert.equal(weightedLength("界"), 2);
  for (const bad of [
    "https://example.com",
    "Ask @someone",
    'A "quote" from a speaker',
    "cfast_this-is-not-a-real-secret",
    "I loved that talk.",
    "a".repeat(281),
    "界".repeat(141),
  ]) {
    assert.throws(() =>
      parseDrafts(
        { drafts: [{ ...values[0], text: bad }, ...values.slice(1)] },
        false,
      ),
    );
  }
  assert.equal(
    parseDrafts(
      {
        drafts: [
          { ...values[0], text: "I changed my mind after testing it." },
          ...values.slice(1),
        ],
      },
      true,
    ).length,
    3,
  );
  assert.throws(() =>
    parseDrafts({ drafts: [values[0], values[0], values[0]] }, false),
  );
});
test("automatic suggestions allow none, block fabricated first person, and filter exact repeats", () => {
  const good = { drafts: [{ angle: "A", text: "A concrete observation about caching." }] };
  assert.equal(parseSuggestion(good, []).text, "A concrete observation about caching.");
  // Nothing worthwhile -> no suggestion.
  assert.equal(parseSuggestion({ drafts: [] }, []), null);
  // Exact duplicate of a recent suggestion -> suppressed.
  assert.equal(
    parseSuggestion(good, ["a concrete observation about caching."]),
    null,
  );
  // Automatic drafts carry no personal take, so first person is rejected.
  assert.throws(() =>
    parseSuggestion({ drafts: [{ angle: "A", text: "I loved this idea." }] }, []),
  );
  assert.throws(() => parseSuggestion({ drafts: {} }, []));
});
test("voice style falls back to the built-in profile and frames examples as tone-only", () => {
  assert.equal(effectiveStyle({ style_instructions: "   " }), VOICE);
  assert.equal(
    effectiveStyle({ style_instructions: "Terse and dry." }),
    "Terse and dry.",
  );
  assert.equal(exampleBlock({ example_posts: "" }), "");
  const block = exampleBlock({ example_posts: "ship it" });
  assert.match(block, /tone/i);
  assert.match(block, /ship it/);
});

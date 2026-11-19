import assert from "node:assert/strict";
import test from "node:test";
import { fromOpenAI, openAISSEToAnthropic, toOpenAI } from "./openai-bridge.mjs";
import { gatewayTarget } from "./runtime.mjs";

test("gatewayTarget never doubles the /v1 segment", () => {
  assert.equal(gatewayTarget({ gateway: "https://integrate.api.nvidia.com/v1" }, "/v1/messages").toString(), "https://integrate.api.nvidia.com/v1/messages");
  assert.equal(gatewayTarget({ gateway: "https://api.example.com" }, "/v1/messages").toString(), "https://api.example.com/v1/messages");
  assert.equal(gatewayTarget({ gateway: "https://x.test/api/v1/" }, "/v1/messages").toString(), "https://x.test/api/v1/messages");
});

test("Anthropic request is translated to OpenAI chat completions", () => {
  const openai = toOpenAI({
    model: "google/gemma-3-12b-it",
    max_tokens: 256,
    temperature: 0.2,
    stop_sequences: ["END"],
    stream: true,
    system: [{ type: "text", text: "Be brief" }],
    messages: [
      { role: "user", content: [{ type: "text", text: "Hi" }] },
      { role: "assistant", content: [{ type: "text", text: "Hello" }, { type: "tool_use", id: "t1", name: "lookup", input: { q: "x" } }] },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: [{ type: "text", text: "42" }] }] },
    ],
    tools: [{ name: "lookup", description: "Find", input_schema: { type: "object" } }],
    tool_choice: { type: "any" },
    diagnostics: { ignored: true },
  });
  assert.equal(openai.stream, true);
  assert.equal(openai.messages[0].role, "system");
  assert.equal(openai.messages[0].content, "Be brief");
  assert.equal(openai.messages[1].content, "Hi");
  assert.equal(openai.messages[2].tool_calls[0].function.name, "lookup");
  assert.equal(openai.messages[3].role, "tool");
  assert.equal(openai.messages[3].content, "42");
  assert.deepEqual(openai.stop, ["END"]);
  assert.equal(openai.tools[0].function.name, "lookup");
  assert.equal(openai.tool_choice, "required");
});

test("OpenAI reply is translated back into an Anthropic message", () => {
  const message = fromOpenAI({
    id: "chatcmpl-1",
    model: "gemma",
    choices: [{ finish_reason: "length", message: { content: "done", tool_calls: [{ id: "t2", function: { name: "f", arguments: '{"a":1}' } }] } }],
    usage: { prompt_tokens: 10, completion_tokens: 3 },
  }, "gemma");
  assert.equal(message.role, "assistant");
  assert.equal(message.stop_reason, "max_tokens");
  assert.deepEqual(message.content[0], { type: "text", text: "done" });
  assert.deepEqual(message.content[1], { type: "tool_use", id: "t2", name: "f", input: { a: 1 } });
  assert.deepEqual(message.usage, { input_tokens: 10, output_tokens: 3 });
});

test("OpenAI SSE becomes Anthropic SSE events Claude Desktop can render", () => {
  const sse = openAISSEToAnthropic(
    [
      'data: {"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}\n\n',
      'data: {"choices":[{"delta":{"content":"lo"}}]}\n\n',
      'data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}\n\n',
      'data: [DONE]\n\n',
    ].join(""),
    "gemma",
  );
  assert.match(sse, /event: message_start/);
  assert.match(sse, /event: content_block_delta\ndata: .*"text":"Hel"/);
  assert.match(sse, /event: content_block_delta\ndata: .*"text":"lo"/);
  assert.match(sse, /event: message_delta/);
  assert.match(sse, /"stop_reason":"end_turn"/);
  assert.match(sse, /event: message_stop/);
  assert.ok(sse.trimEnd().endsWith("data: [DONE]"));
});

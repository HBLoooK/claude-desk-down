// Bridge Claude's Anthropic Messages API to OpenAI-compatible gateways.
// Only used as a fallback when an upstream has no /v1/messages route but does
// expose /v1/chat/completions (e.g. NVIDIA NIM).

const textOf = (content) => {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((block) => block?.type === "text" ? String(block.text ?? "") : "").join("");
};

function toOpenAIContent(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  const parts = [];
  for (const block of content) {
    if (!block || typeof block !== "object") continue;
    if (block.type === "text") parts.push({ type: "text", text: String(block.text ?? "") });
    else if (block.type === "image" && block.source?.type === "base64") {
      parts.push({ type: "image_url", image_url: { url: `data:${block.source.media_type || "image/png"};base64,${block.source.data || ""}` } });
    }
  }
  // A single text part is simpler for strict OpenAI-compatible endpoints.
  if (parts.length === 1 && parts[0].type === "text") return parts[0].text;
  return parts;
}

export function toOpenAI(body) {
  const messages = [];
  // Anthropic carries `system` as a top-level field; OpenAI expects a system message.
  if (body.system !== undefined && body.system !== null) {
    const system = toOpenAIContent(body.system);
    if (typeof system === "string" ? system.trim() : (Array.isArray(system) && system.length)) {
      messages.push({ role: "system", content: system });
    }
  }
  for (const entry of Array.isArray(body.messages) ? body.messages : []) {
    if (!entry || typeof entry !== "object") continue;
    if (entry.role === "system") {
      messages.push({ role: "system", content: toOpenAIContent(entry.content) });
      continue;
    }
    if (entry.role === "user") {
      const content = Array.isArray(entry.content)
        ? entry.content.filter((block) => block?.type !== "tool_result")
        : entry.content;
      if (Array.isArray(content) || typeof content === "string") {
        const converted = toOpenAIContent(content);
        if (typeof converted === "string" ? converted.trim() : (Array.isArray(converted) && converted.length)) {
          messages.push({ role: "user", content: converted });
        }
      }
      for (const block of Array.isArray(entry.content) ? entry.content : []) {
        if (block?.type === "tool_result") {
          messages.push({ role: "tool", tool_call_id: block.tool_use_id, content: textOf(block.content) });
        }
      }
      continue;
    }
    if (entry.role === "assistant") {
      const text = textOf(entry.content);
      const toolCalls = (Array.isArray(entry.content) ? entry.content : [])
        .filter((block) => block?.type === "tool_use")
        .map((block) => ({
          id: block.id,
          type: "function",
          function: { name: block.name, arguments: JSON.stringify(block.input ?? {}) },
        }));
      const message = { role: "assistant", content: text };
      if (toolCalls.length) message.tool_calls = toolCalls;
      messages.push(message);
    }
  }
  const out = {
    model: body.model,
    messages,
    stream: Boolean(body.stream),
  };
  if (body.max_tokens != null) out.max_tokens = body.max_tokens;
  if (body.temperature != null) out.temperature = body.temperature;
  if (body.top_p != null) out.top_p = body.top_p;
  if (Array.isArray(body.stop_sequences) && body.stop_sequences.length) out.stop = body.stop_sequences;
  if (Array.isArray(body.tools) && body.tools.length) {
    out.tools = body.tools.map((tool) => ({
      type: "function",
      function: {
        name: tool.name,
        description: tool.description ?? "",
        parameters: tool.input_schema ?? { type: "object", properties: {} },
      },
    }));
  }
  if (body.tool_choice && typeof body.tool_choice === "object") {
    if (body.tool_choice.type === "any") out.tool_choice = "required";
    else if (body.tool_choice.type === "tool" && body.tool_choice.name) {
      out.tool_choice = { type: "function", function: { name: body.tool_choice.name } };
    } else out.tool_choice = "auto";
  }
  return out;
}

const STOP = { stop: "end_turn", length: "max_tokens", tool_calls: "tool_use", content_filter: "stop_sequence" };
const messageId = () => `msg_${Date.now().toString(16)}${Math.random().toString(16).slice(2, 10)}`;

function usageOf(value) {
  return {
    input_tokens: Number(value?.prompt_tokens) || 0,
    output_tokens: Number(value?.completion_tokens) || 0,
  };
}

function contentOf(message) {
  const blocks = [];
  const text = typeof message?.content === "string" ? message.content : "";
  if (text) blocks.push({ type: "text", text });
  for (const call of Array.isArray(message?.tool_calls) ? message.tool_calls : []) {
    let input = {};
    try { input = JSON.parse(call.function?.arguments || "{}"); } catch { /* keep empty */ }
    blocks.push({ type: "tool_use", id: call.id, name: call.function?.name, input });
  }
  return blocks.length ? blocks : [{ type: "text", text: "" }];
}

export function fromOpenAI(reply, model) {
  const choice = reply?.choices?.[0];
  const stop = STOP[choice?.finish_reason] || "end_turn";
  return {
    id: reply.id || messageId(),
    type: "message",
    role: "assistant",
    model: model || reply.model,
    content: contentOf(choice?.message),
    stop_reason: stop,
    stop_sequence: null,
    usage: usageOf(reply.usage),
  };
}

const sse = (event, data) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;

/** Convert an OpenAI SSE stream into the Anthropic SSE events Claude Desktop expects. */
export function openAISSEToAnthropic(text, model) {
  const id = messageId();
  const out = [
    sse("message_start", {
      type: "message_start",
      message: { id, type: "message", role: "assistant", model, content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 0, output_tokens: 0 } },
    }),
    sse("content_block_start", { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
    sse("ping", { type: "ping" }),
  ];
  let stopReason = "end_turn";
  let outputTokens = 0;
  let inputTokens = 0;
  let started = false;
  for (const line of String(text || "").split(/\r?\n/)) {
    if (!line.startsWith("data:")) continue;
    const data = line.slice(5).trim();
    if (!data || data === "[DONE]") continue;
    let chunk;
    try { chunk = JSON.parse(data); } catch { continue; }
    if (chunk.usage) {
      inputTokens = Number(chunk.usage.prompt_tokens) || inputTokens;
      outputTokens = Number(chunk.usage.completion_tokens) || outputTokens;
    }
    const choice = chunk.choices?.[0];
    if (!choice) continue;
    if (choice.finish_reason) stopReason = STOP[choice.finish_reason] || "end_turn";
    for (const call of choice.delta?.tool_calls || []) {
      let input = "";
      try { input = call.function?.arguments || ""; } catch { /* ignore */ }
      if (input) {
        out.push(sse("content_block_delta", { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: input } }));
      }
    }
    const delta = choice.delta?.content;
    if (typeof delta === "string" && delta) {
      started = true;
      out.push(sse("content_block_delta", { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: delta } }));
    }
  }
  if (!started) out.splice(1, 1); // no text block was ever produced
  out.push(sse("content_block_stop", { type: "content_block_stop", index: 0 }));
  out.push(sse("message_delta", {
    type: "message_delta",
    delta: { stop_reason: stopReason, stop_sequence: null },
    usage: { output_tokens: outputTokens, input_tokens: inputTokens },
  }));
  out.push(sse("message_stop", { type: "message_stop" }));
  return out.join("") + "data: [DONE]\n\n";
}

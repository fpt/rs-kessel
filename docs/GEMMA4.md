# Gemma 4 prompt format, and how kessel implements it

Gemma 4 does not use ChatML. It has its own turn markers and — unusually for an
open model — a **native tool-calling syntax baked into the chat template**, not a
JSON convention layered on top. `configs/gemma4.yaml` runs
`unsloth/gemma-4-26B-A4B-it-qat-GGUF`, so kessel has to speak that format
exactly. The format is shared across the Gemma 4 family, so everything below
applies equally to the smaller `gemma-4-E4B-it-GGUF`.

This documents the wire format and what `crates/lib/src/llm_local.rs` actually
does with it.

**Sources**
- [Gemma 4 prompt formatting](https://ai.google.dev/gemma/docs/core/prompt-formatting-gemma4) (Google)
- [`chat_template.jinja`](https://huggingface.co/google/gemma-4-E4B-it/blob/main/chat_template.jinja) (`google/gemma-4-E4B-it`) — the normative artifact; line numbers below refer to it

---

## 1. Control tokens

| Token(s) | Purpose |
|---|---|
| `<\|turn>` … `<turn\|>` | Open / close a dialogue turn |
| `system` · `user` · `model` | Role names. Note **`model`**, not `assistant` |
| `<\|tool>` … `<tool\|>` | A tool *declaration* (in the system turn) |
| `<\|tool_call>` … `<tool_call\|>` | The model requesting a call |
| `<\|tool_response>` … `<tool_response\|>` | A tool result fed back |
| `<\|"\|>` | **String delimiter** — the model's stand-in for a quote character |
| `<\|think\|>` | Enables thinking mode; emitted at the top of the first system turn |
| `<\|channel>thought` … `<channel\|>` | The model's internal reasoning |
| `<\|image\|>` · `<\|audio\|>` · `<\|video\|>` | Multimodal placeholders (unused by kessel) |

The role name is `model`, not `assistant` — the template maps it
(`{%- set role = 'model' if message['role'] == 'assistant' else message['role'] -%}`,
line 218), so callers still pass `assistant`.

## 2. Conversation layout

```
<bos><|turn>system
You are a helpful assistant.<|tool>declaration:read{...}<tool|><turn|>
<|turn>user
Read a.txt<turn|>
<|turn>model
```

The system turn is emitted (line 179) when **any** of these hold: thinking is
enabled, tools are present, or `messages[0]` is a `system`/`developer` message.
Tool declarations live *inside that same system turn*, after the system text.

The trailing `<|turn>model\n` is the generation prompt, appended when
`add_generation_prompt` is set — but suppressed if the previous message was a
tool call or tool response (lines 356–360), so the model continues its turn
rather than starting a new one.

## 3. Tool calling

**Declaration** (system turn):

```
<|tool>declaration:search-godoc{query:string}<tool|>
```

**Call** (model → us):

```
<|tool_call>call:search-godoc{query:<|"|>mcp-go<|"|>}<tool_call|>
```

**Response** (us → model):

```
<|tool_response>response:search-godoc{result:<|"|>...<|"|>}<tool_response|>
```

Two things to notice, because both bite:

- **`<|"|>` replaces the quote character.** Arguments are not JSON. A string
  value is wrapped in `<|"|>…<|"|>`; numbers are bare (`limit:50`).
- **Tool names may contain `-` and `.`** — they are not restricted to
  identifier characters.

## 4. What kessel does

### Rendering the prompt

`llama-cpp-2` 0.1.150 removed its OAI-compat/jinja chat layer, so kessel renders
the GGUF's **embedded** jinja template itself with **minijinja**
(`jinja_env`). Two shims are needed:

- `minijinja_contrib::pycompat` — templates call Python string methods
  (`.strip()`, `.startswith()`, …).
- A `raise_exception()` function, because chat templates use it to reject inputs
  they don't support.

`build_prompt` then picks a path:

1. **Native tool protocol.** `template_supports_native_tools()` sniffs the
   template source for `<|tool_call>`, `<|tool>`, or `declaration:`. If found —
   as it is for gemma 4 — `render_native()` passes the tools as an OpenAI-shaped
   `tools` array plus full message objects (assistant `tool_calls`, `tool`
   results). The template emits the native tokens itself. **The model sees tools
   in exactly the form it was trained on**, which is the whole point.
2. **JSON-prose fallback.** For templates with no native tool support, tool
   definitions are appended to the system message with a JSON output protocol
   (`tool_instructions`).
3. **System fold.** If the template *rejects* a system role, the system text is
   folded into the first user turn and rendering is retried.
4. **Manual ChatML.** Last resort when there is no embedded template, or it still
   won't render.

### Parsing the reply

`parse_tool_calls` strips `<think>…</think>` first (so a JSON scan can't latch
onto braces inside chain-of-thought), then tries, in order:

1. JSON — a bare object/array, or OpenAI `tool_calls`, including stringified
   `arguments`.
2. Python/Llama style — `[name(arg=val)]`. Gated on the whole reply looking like
   a call list, so prose mentioning `read()` isn't misread as a call.
3. **Gemma native** — `parse_gemma_calls`, as a lenient last resort.

`parse_gemma_calls` normalizes `<|"|>` to `"` and matches
`call:\s*([A-Za-z0-9_.\-]+)\s*\{([^{}]*)\}`. It is deliberately
**delimiter-agnostic**: it keys on `call:NAME{…}` rather than requiring the
surrounding `<|tool_call>` markers, because models truncate and mangle them.
Call ids are assigned positionally (`call_0`, `call_1`, …).

Gemma native is tried *last* on purpose: it is the most permissive matcher, and
running it before the JSON scan would let it swallow well-formed JSON replies.

## 5. Gotchas

**The system role *is* supported.** Gemma 4's template contains **no
`raise_exception`** at all and handles `system`/`developer` at line 186. The
system-fold in `build_prompt` is a generic fallback for templates that *do*
reject a system role (gemma 2/3 did) — **it does not fire for gemma 4**. Earlier
comments in this repo claimed otherwise; they were wrong and have been corrected.

**Only `messages[0]` gets the system turn.** The template's system block reads
`messages[0]` and nothing else (line 179). A system message appearing *later*
falls through to the main loop and renders as a second, mid-conversation
`<|turn>system` block — syntactically fine, but not a shape gemma was trained on.
This matters: kessel appends the skill catalog as a trailing system message
(`lib.rs`), so a gemma turn with skills loaded gets exactly that. It works, but
if gemma ever ignores the catalog, this is the first place to look.

**Thoughts must be stripped between turns.** Google's spec is explicit that the
model's generated thoughts from a previous turn must be removed before
continuing, *except* mid-function-call. kessel strips `<think>` blocks before
parsing.

**The model may ignore the JSON protocol.** Even when asked for JSON, gemma
sometimes emits its native `<|tool_call>` form — which is precisely why
`parse_gemma_calls` exists as a fallback rather than a native-path-only branch.

## 6. Tests

`crates/lib/src/llm_local.rs` (`mod tests`) covers the format directly:

| Test | Property |
|---|---|
| `parses_gemma_native_tool_call` | The full envelope; hyphens in both the tool name and the value |
| `parses_gemma_call_with_mixed_args` | `<\|"\|>`-quoted strings alongside bare numbers |
| `plain_prose_is_not_a_gemma_call` | Prose about calling a tool is not a call |
| `parses_call_after_think_block` | `<think>` stripping before the JSON scan |

Run: `cd crates && cargo test -p kessel-core --lib gemma`

Verified end to end against `gemma-4-E4B`. The default in `configs/gemma4.yaml`
has since moved to `gemma-4-26B-A4B-it-qat`, which shares the prompt format but
has **not** been re-run against these paths — if tool calling misbehaves there,
that gap is the first thing to check (`make run-gemma4`).

# Experiment 0006 — Pi Reference Study

This document records the design study of the current `earendil-works/pi`
repository performed **before** implementing the Experiment 0006 Rust
kernel. It records *principles observed in source*, not code to port.

Sources inspected (current state of the repository, read directly — not
from memory):

| Area | Source |
| --- | --- |
| Product session layer | `packages/coding-agent/src/core/agent-session.ts` (≈3.7k lines) |
| Programmatic session entry point | `packages/coding-agent/src/core/sdk.ts` (`createAgentSession`) |
| Fundamental agent loop + state + events | `packages/agent/src/agent-loop.ts`, `agent.ts`, `types.ts` |
| Built-in tool set | `packages/coding-agent/src/core/tools/index.ts` |
| Tool description/execution split | `packages/coding-agent/src/core/extensions/types.ts` (`ToolDefinition`) |
| Event protocol (headless) | `packages/coding-agent/docs/rpc.md` |
| SDK/event surface | `packages/coding-agent/docs/sdk.md` |

Pi organizes the agent in two layers that the study keeps separate:

1. **Agent core** (`packages/agent`): a small, provider-agnostic loop.
   `runAgentLoop` streams an assistant response, executes its tool calls,
   appends tool results to the context, and repeats until there are no more
   tool calls. State is `AgentState { systemPrompt, model, tools, messages,
   isStreaming, pendingToolCalls, errorMessage }`. Events are a closed typed
   union: agent/turn/message/tool-execution lifecycle.
2. **Session layer** (`packages/coding-agent`): everything product-shaped —
   settings, session persistence, extensions, skills, slash commands,
   compaction, retry, caching, auth, UI, RPC — wrapped *around* the core.
   `AgentSession` subscribes to core events and layers persistence,
   extensions, and compaction on top. Interactive/TUI, RPC, print, and SDK
   modes all consume the same session/events instead of the loop itself.

## Findings by question

### Kernel ownership

What Pi treats as common agent/session core:

- The turn loop: assistant response → tool calls → tool results → next turn.
- Message transcript as the unit of state (`messages: AgentMessage[]`).
- Tool call parsing and dispatch, tool result insertion.
- Turn/agent lifecycle events (start/end), message lifecycle,
  tool execution start/end.
- Termination: the loop stops when an assistant response carries no tool
  calls (plus optional `shouldStopAfterTurn` hooks).

What Pi keeps *outside* the core:

- **Interactive/TUI mode**: rendering, editor, keybindings, themes,
  streaming display.
- **Print mode**: one-shot output formatting.
- **RPC mode**: a JSONL command/event protocol; commands (`prompt`, `steer`,
  `follow_up`, `abort`, …) and `{"type":"response"}` correlation. The
  kernel never knows an RPC layer exists — RPC observes events emitted by the
  session.
- **Settings, session persistence (SessionManager), session switching/
  forking, compaction, model registry/auth, extensions, skills, slash
  commands** — all in the session/product layer.

### Turn loop (semantics only)

Pi's `runAgentLoop`:

```
emit agent_start, turn_start
append prompt messages to context
loop while the assistant response contains tool calls (or queued messages):
    prepare next turn (context rewrite hooks — e.g. compaction)
    stream assistant response (provider)
    if response is error/aborted: emit turn_end + agent_end; stop
    collect tool calls from the response content
    execute tool calls (parallel/sequential per tool policy)
    append tool results to context
    emit turn_end
    if shouldStopAfterTurn: stop
emit agent_end
```

Semantics extracted:

- One turn = one assistant response plus the tool results it triggered.
- The transcript (messages) is the sole conversational state; tool results
  are messages, appended after their call.
- Termination is a *property of the response* (no tool calls), not a timer.
  Pi has no turn/tool-call count limit in the core — a policy hook decides.
- Steering/follow-up message queues are a product feature layered on the
  loop, not part of the loop.

### Tool model

Pi separates **description** from **execution**:

- `ToolDefinition` (description side): `name` (LLM-facing), `label`
  (UI-facing), `description` (LLM-facing), `parameters` (TypeBox JSON
  schema), optional prompt snippets/guidelines.
- `AgentTool` (execution side): the same name plus `execute(toolCallId,
  params, signal, onUpdate)`, validation of params against the schema,
  execution-mode policy (parallel vs sequential), and result shape
  `{ content, details, isError, terminate }`.
- Invocations are represented as `toolCall` content blocks inside the
  assistant message: `{ id, name, arguments }`.
- Results come back as `toolResult` messages keyed by `toolCallId`.
- Built-in tools (`read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`,
  `powershell`) are factories with options; allow/deny lists select which
  definitions are active per session.

### State

Fundamental loop state (from `AgentState` / `AgentContext`):

- transcript (`messages`)
- active tool set
- model identity
- in-flight markers (`isStreaming`, `pendingToolCalls`)
- last error

Everything else in `AgentSession` is product state: settings, scoped model
lists, extension runners, compaction controllers, retry counters, steering
queues, cache warmer, bash abort controllers, cwd, session manager.

### Events / protocol

- Core emits a **closed, typed event union** (discriminated by `type`),
  with distinct lifecycles for agent → turn → message → tool execution.
- The session re-emits (possibly enriched) events to subscribers; RPC mode
  serializes them as JSONL to stdout. Commands (stdin) and events (stdout)
  are **distinct channels**; commands correlate with `{"type":"response"}`
  via an optional `id`.
- The event stream is stream-like: `message_update` deltas during a
  response, `*_start` / `*_end` pairs for turns and tool executions,
  terminal `agent_end` then `agent_settled`.

Properties useful for Mutagen: typed command/event distinction; external
consumers observe rather than drive the loop; structured, lifecycle-ordered
events make a trajectory readable after the fact.

### Minimalism

Features Pi keeps outside the fundamental kernel: persistence, branching/
forking, compaction, thinking-level switching, model cycling, retry, caching,
skills, extensions, commands, all presentation, project trust, telemetry.
The core loop itself is a single `while` over "response has tool calls".

## Design-note table

| Pi observation | Principle | Mutagen 0006 decision | Status |
| --- | --- | --- | --- |
| One `runAgentLoop` shared by interactive, print, RPC, and SDK modes | The execution kernel must not depend on presentation or control mode | `kernel.rs` knows only messages, model-call, tools, limits, and its own event vector; no CLI, TUI, RPC, or filesystem awareness | adopted |
| Transcript is the sole conversational state; tool results are appended as messages | An agent session's fundamental state is its message list plus counters | `EpisodeState { messages, events, model_turn_count, tool_call_count }` | adopted |
| Turn = one assistant response + resulting tool results; loop ends when the response has no tool calls | Termination is a property of the model response, not a timer | Kernel loop ends on a final answer; hard limits (`turn_limit`, `tool_call_limit`) act as explicit, recorded policy — Pi lacks these, we need them for experiment eligibility | adopted |
| `ToolDefinition` (name/description/JSON schema) separated from `AgentTool.execute` | A tool is a model-visible description plus an execution boundary | `state_read`/`state_write` keep exactly that split: JSON-schema `ToolSpec` + executor over `ExternalState` | adopted |
| Closed typed event union with agent/turn/message/tool lifecycles | Structured, lifecycle-ordered events give an authoritative trajectory | Small closed `TrajectoryEvent` enum (9 variants); the kernel itself appends events to `EpisodeState.events` — trajectory is never reconstructed from logs | adopted |
| RPC mode observes events rather than driving the loop | Observers are external to execution | The experiment runner consumes the kernel's `EpisodeOutcome`; it never injects into the loop mid-episode | adopted |
| Tool allow/deny lists select which descriptions the model sees | Capability is granted by construction | The model receives exactly `state_read` + `state_write`; no shell/fs/browser/network tools exist in the kernel at all | adopted |
| Tool calls identified by `toolCallId`, results keyed back to it | Call identity should be explicit and stable | Each record persists `tool_call_id`; canonical identity = tool name + key-sorted JSON arguments | adopted |
| Steering/follow-up queues, mid-run user messages | Mid-run human injection is a product feature | Rejected for 0006: episodes are one-shot task prompts | rejected |
| Session persistence, forking, branching (SessionManager) | Persistence is product state | Rejected: each episode is ephemeral; the *artifact* (written by the runner, not the kernel) is the only durable record | rejected |
| Compaction / context overflow recovery | Context management is a long-session product feature | Rejected: 12-turn episodes cannot approach context limits | rejected |
| Model registry, auth flows, model cycling, thinking levels, retry, cache warming | Multi-provider product machinery | Rejected: one concrete `ModelClient`, one model, no retry, no cycling | rejected |
| Extension system, skills, slash commands, custom tools | Open extension surface | Rejected: the capability surface is a fixed two tools | rejected |
| Parallel tool execution with per-tool execution modes | Concurrency is a throughput feature | Rejected: synchronous single-call execution; a >1-call response is recorded as `parallel_tool_calls_unsupported`, not silently serialized | rejected |
| Streaming deltas (`message_update`), partial tool results | Interactive responsiveness | Rejected: non-streaming `chat/completions`; the 9-event set covers the lifecycle without deltas | rejected |
| `reasoning`/thinking content carried in messages | Provider-specific chain-of-thought | Rejected: reasoning fields are ignored, never stored, never persisted (spec §24) | rejected |
| 3.7k-line `AgentSession` with ~20 subsystems | Mature product sessions accrete subsystems | The kernel module is a single ~400-line file whose core is one loop function; every subsystem above is absent by construction, not by flag (the full crate is ~4k lines only because of tests and the artifact verifier) | adopted (as a design constraint) |

## Adopt / Reject / Defer

### Adopt (conceptually borrowed)

1. Kernel/presentation separation — the kernel is mode-agnostic.
2. Transcript-as-state plus small explicit counters.
3. Tool = model-visible description + execution boundary.
4. Closed typed event set emitted by the kernel itself.
5. Termination as a property of the response; explicit count limits as
   experiment policy layered beside it.
6. `tool_call_id`-keyed result insertion.
7. Capability granted by construction (only the tools the model is given).

### Reject (not appropriate for Mutagen 0006)

1. Mid-run message queues (steering/follow-up).
2. Session persistence/branching/compaction.
3. Multi-provider registry, auth flows, model cycling, retry, caching.
4. Extension/skill/command/plugin surfaces.
5. Parallel tool execution and concurrency.
6. Streaming events.
7. Persistence of reasoning content.
8. Pi's bash/file tool architecture (no shell, no filesystem access in the
   experiment kernel).

### Defer (potentially useful, no current evidence)

1. A general event *bus* / multi-subscriber protocol — 0006 needs one
   consumer (the kernel's own vector); revisit when a second observer
   (UI, RPC) exists.
2. Tool result error semantics beyond `ok` (Pi's `isError`, `details`,
   `terminate`) — revisit when tools can fail in varied ways.
3. Context/compaction management — only relevant once episodes grow long.
4. A `ModelBackend`-style abstraction — 0006 has exactly one provider; the
   kernel takes a closure, and no named trait is introduced (see
   README "Design notes").
5. Durable/replay semantics for tool effects (`replay: "safe"`) — a
   production concern, not an 0006 question.

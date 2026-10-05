//! Transcript: turn an agent CLI's JSON Lines stdout (`--mode json`) into
//! (a) a HUMAN-READABLE rendering for `state/logs/<task>.log` and (b) the
//! final token usage — so `Receipt.tokens` measures real spend instead of
//! staying a dead `None` (the data was already on the wire and thrown
//! away).
//!
//! Event shapes were captured from a real `pi --mode json` run: the CLI
//! writes one JSON object per line — `session`, `message_start`,
//! `message_update` (carrying `usage` and an `assistantMessageEvent` such
//! as `text_delta` or a tool call), `message_end` (the fully formed
//! assistant message, usage included), `turn_end`, `agent_end`,
//! `agent_settled`. The totals appear on several events and are IDENTICAL
//! on the final ones, so the authoritative number is the one on the LAST
//! event that carries a usage object.
//!
//! Parsing is DEFENSIVE by contract: lines that are not valid JSON are
//! ignored, unknown event types are ignored, no input can panic, and a
//! stream that yields no usage at all (a crash, a truncation, a CLI that
//! ignores the flag) yields `usage: None` — missing telemetry must never
//! fail an attempt. The rendering keeps the log a debugging surface for
//! humans — raw JSONL in the log would be a debuggability regression.
//!
//! Rendering rules, all of them learned from that captured run:
//! * assistant/user/tool message text is rendered as prose, labelled by the
//!   message ROLE (the first `message_end` is the USER message carrying the
//!   prompt — labelling it `assistant:` is a lie a developer would act on);
//! * a finished tool call is one compact line naming the tool and a bounded
//!   summary of its arguments, and its output is rendered under it;
//! * STREAMING CHUNKS ARE NOT RENDERED one line each. One single tool call
//!   in the captured run produced 110 `thinking_delta` plus 123
//!   `toolcall_delta` events; a line per chunk buries the signal and scales
//!   with how long the agent thinks. Only completed activity is rendered, and
//!   an unrecognised event is rendered only when the CLI labelled it — so no
//!   future streaming event type can re-introduce the line-per-chunk log.

use serde_json::Value;

/// Token usage reported by the agent CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub total_tokens: u64,
}

/// A parsed transcript: the human-readable rendering plus the final usage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    /// Human-readable rendering for the task log. Empty when the stream
    /// carried nothing renderable (callers fall back to the raw stream so
    /// the log never silently loses output).
    pub rendered: String,
    /// Usage from the LAST event that carried one — the authoritative
    /// final total. `None` when no event did.
    pub usage: Option<Usage>,
}

/// Parse a captured `--mode json` stdout stream.
pub fn parse(stdout: &str) -> Transcript {
    let mut t = Transcript::default();
    // text_delta chunks of the in-flight assistant message: the fallback
    // text when the stream dies before its message_end.
    let mut pending = String::new();
    let mut lines: Vec<String> = Vec::new();

    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // not valid JSON — ignored
        };
        let Some(kind) = v.get("type").and_then(Value::as_str) else {
            continue;
        };
        match kind {
            "message_start" => pending.clear(),
            "message_update" => {
                note_usage(&v, &mut t);
                if let Some(ev) = v.get("assistantMessageEvent") {
                    match ev.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(d) = ev.get("delta").and_then(Value::as_str) {
                                pending.push_str(d);
                            }
                        }
                        // Streaming chunks: deliberately silent. The finished
                        // activity is rendered at its `_end` event.
                        Some(
                            "thinking_start" | "thinking_delta" | "text_start" | "text_end"
                            | "toolcall_start" | "toolcall_delta",
                        ) => {}
                        // The agent reasoned: worth one bounded line, not the
                        // 110 lines its chunks occupied.
                        Some("thinking_end") => {
                            if let Some(l) = thinking_line(ev) {
                                lines.push(l);
                            }
                        }
                        // The call the agent decided to make, once it is whole.
                        Some("toolcall_end") => lines.push(tool_call_line(ev)),
                        // Unknown activity: kept only when the CLI labelled it.
                        Some(_) => {
                            if let Some(l) = labelled_activity(ev) {
                                lines.push(l);
                            }
                        }
                        None => {}
                    }
                }
            }
            "message_end" => {
                note_usage(&v, &mut t);
                if let Some(msg) = v.get("message") {
                    // pi carries the usage inside the message object here.
                    note_usage(msg, &mut t);
                    let final_text = content_text(msg);
                    let text = if final_text.trim().is_empty() {
                        pending.clone()
                    } else {
                        final_text
                    };
                    if !text.trim().is_empty() {
                        // The role is real: the first message_end is the USER
                        // message that carries the task prompt, and a tool's
                        // output comes back as a `toolResult` message. Only the
                        // assistant's own words may be attributed to it.
                        let role = msg
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("assistant");
                        match role {
                            "assistant" | "user" => lines.push(format!("{role}: {text}")),
                            other => lines.push(format!("[{other}]\n{text}")),
                        }
                    }
                }
                pending.clear();
            }
            // The tool's own execution events are CLI internals that duplicate
            // the `toolResult` message rendered above — ignored on purpose, so
            // a tool's output appears exactly once.
            // The final totals ride on these events too — the LAST one
            // seen wins, so a multi-message stream reports its true total.
            "turn_end" | "agent_end" => note_usage(&v, &mut t),
            // session / agent_settled / anything unknown: nothing to render.
            _ => {}
        }
    }
    // A stream truncated mid-message still shows what the agent said.
    if !pending.trim().is_empty() {
        lines.push(format!("assistant (truncated): {pending}"));
    }
    if let Some(u) = t.usage {
        lines.push(format!(
            "usage: {} in, {} out, {} total",
            u.input, u.output, u.total_tokens
        ));
    }
    t.rendered = lines.join("\n");
    t
}

/// Record `obj`'s usage object (if any) as the latest one seen.
fn note_usage(obj: &Value, t: &mut Transcript) {
    if let Some(u) = obj.get("usage").and_then(usage_from) {
        t.usage = Some(u);
    }
}

/// A usage object (`{"input":..,"output":..,"totalTokens":..}`) to a
/// [`Usage`]. Defensive: numbers may arrive as floats; a usage without
/// `totalTokens` falls back to input+output when both exist, else is not
/// a usable usage at all.
fn usage_from(u: &Value) -> Option<Usage> {
    let num = |k: &str| u.get(k).and_then(num_as_u64);
    let input = num("input");
    let output = num("output");
    let total = num("totalTokens").or_else(|| match (input, output) {
        (Some(i), Some(o)) => Some(i.saturating_add(o)),
        _ => None,
    })?;
    Some(Usage {
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        total_tokens: total,
    })
}

/// JSON number → u64 (integers directly, floats truncated at zero).
fn num_as_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
}

/// One compact line for a finished tool call: the tool's name plus a bounded
/// summary of its arguments (`[tool] bash: ls /tmp | head -3`).
fn tool_call_line(ev: &Value) -> String {
    let call = ev.get("toolCall").unwrap_or(ev);
    let name = call
        .get("name")
        .or_else(|| ev.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    match args_summary(call.get("arguments")) {
        Some(args) => format!("[tool] {name}: {args}"),
        None => format!("[tool] {name}"),
    }
}

/// A one-line, length-bounded summary of a tool call's arguments. The
/// informative key wins when there is one (`command`/`filePath`/`path`/
/// `pattern`) — a JSON dump of every argument makes a log unreadable.
fn args_summary(args: Option<&Value>) -> Option<String> {
    let raw = match args? {
        Value::Null => return None,
        Value::String(s) => s.clone(),
        Value::Object(map) => ["command", "filePath", "path", "pattern"]
            .iter()
            .find_map(|k| map.get(*k).and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| Value::Object(map.clone()).to_string()),
        other => other.to_string(),
    };
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return None;
    }
    Some(truncate_chars(&one_line, 160))
}

/// Truncate at a char boundary, marking that something was cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

/// One bounded line for a finished thinking block — the agent reasoned, which
/// is worth knowing, but the reasoning itself is long and redundant with the
/// answer.
fn thinking_line(ev: &Value) -> Option<String> {
    let thought = ev
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if thought.is_empty() {
        return Some("[thinking]".to_string());
    }
    let one_line = thought.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!("[thinking] {}", truncate_chars(&one_line, 160)))
}

/// Compact one-line rendering of a labelled, unrecognised activity event:
/// the event type plus its title/name. Returns `None` when the CLI gave the
/// event no label — an unlabelled event is not worth a log line, and
/// rendering those unconditionally is exactly how a log fills up with
/// meaningless `[some_delta]` noise.
fn labelled_activity(ev: &Value) -> Option<String> {
    let kind = ev.get("type").and_then(Value::as_str)?;
    let label = ev
        .get("title")
        .or_else(|| ev.get("name"))
        .and_then(Value::as_str)?;
    Some(format!("[{kind}] {label}"))
}

/// Concatenated `text` parts of a message/result object shaped
/// `{ "content": [ {"type":"text","text":…}, … ] }` (the fully formed
/// text is authoritative over the streamed deltas). Non-text parts —
/// `thinking`, `toolCall` — contribute nothing.
fn content_text(msg: &Value) -> String {
    let Some(parts) = msg.get("content").and_then(Value::as_array) else {
        return String::new();
    };
    let mut out = String::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcript in the REAL event order of a `pi --mode json` run
    /// (captured from a live run, then trimmed to the informative events):
    /// turn 1 streams a thinking block, a tool call and NO text; the tool's
    /// result comes back as a `toolResult` MESSAGE; turn 2 streams the
    /// assistant's text. PARTIAL usage rides the streaming events, the
    /// full/final usage rides `message_end`/`turn_end`/`agent_end` with the
    /// SAME total — the number a receipt must record.
    const REALISTIC: &str = concat!(
        "{\"type\":\"session\",\"version\":3,\"sessionId\":\"s1\"}\n",
        "{\"type\":\"message_start\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Create hello.txt\"}]}}\n",
        "{\"type\":\"message_end\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Create hello.txt\"}]}}\n",
        "{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_start\",\"contentIndex\":0}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_delta\",\"contentIndex\":0,\"delta\":\"Let\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_delta\",\"contentIndex\":0,\"delta\":\" me check\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"toolcall_start\",\"contentIndex\":1}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"toolcall_delta\",\"contentIndex\":1,\"delta\":\"{\\\"command\\\":\"ls\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"toolcall_delta\",\"contentIndex\":1,\"delta\":\" /tmp\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_end\",\"contentIndex\":0,\"content\":\"The user wants a file; I will write it.\"}}\n",
        "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"toolcall_end\",\"contentIndex\":1,\"toolCall\":{\"type\":\"toolCall\",\"id\":\"c1\",\"name\":\"bash\",\"arguments\":{\"command\":\"ls /tmp | head -3\"}}}}\n",
        "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"The user wants a file; I will write it.\"},{\"type\":\"toolCall\",\"name\":\"bash\"}],\"usage\":{\"input\":120,\"output\":2,\"totalTokens\":122},\"stopReason\":\"toolUse\"}}\n",
        "{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\"toolName\":\"bash\",\"args\":{\"command\":\"ls /tmp | head -3\"}}\n",
        "{\"type\":\"tool_execution_end\",\"toolCallId\":\"c1\",\"toolName\":\"bash\",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"af_dog_run.log\\naf_dog_state\\n\"}]},\"isError\":false}\n",
        "{\"type\":\"message_start\",\"message\":{\"role\":\"toolResult\",\"content\":[]}}\n",
        "{\"type\":\"message_end\",\"message\":{\"role\":\"toolResult\",\"content\":[{\"type\":\"text\",\"text\":\"af_dog_run.log\\naf_dog_state\\n\"}]}}\n",
        "{\"type\":\"turn_end\",\"usage\":{\"input\":120,\"output\":2,\"totalTokens\":122}}\n",
        "{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_start\",\"contentIndex\":0}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"ok\"}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\", done\"}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_end\",\"contentIndex\":0,\"content\":\"ok, done\"}}\n",
        "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok, done\"}],\"provider\":\"zai\",\"model\":\"glm-5.2\",\"usage\":{\"input\":13523,\"output\":2,\"cacheRead\":0,\"cacheWrite\":0,\"reasoning\":0,\"totalTokens\":13525,\"cost\":{\"input\":0,\"output\":0,\"total\":0}},\"stopReason\":\"stop\"}}\n",
        "{\"type\":\"turn_end\",\"usage\":{\"input\":13523,\"output\":2,\"totalTokens\":13525}}\n",
        "{\"type\":\"agent_end\",\"messages\":[],\"usage\":{\"input\":13523,\"output\":2,\"totalTokens\":13525},\"willRetry\":false}\n",
        "{\"type\":\"agent_settled\"}\n",
    );

    #[test]
    fn realistic_transcript_yields_final_usage_and_readable_text() {
        let t = parse(REALISTIC);
        // The LAST usage wins — the final total, not the streaming partial.
        assert_eq!(
            t.usage,
            Some(Usage {
                input: 13523,
                output: 2,
                total_tokens: 13525
            })
        );
        // The final message content is rendered once as prose …
        assert_eq!(t.rendered.matches("assistant: ok, done").count(), 1);
        // … the usage is summarized …
        assert!(t.rendered.contains("usage: 13523 in, 2 out, 13525 total"));
        // … and NO raw JSONL survives into the rendering.
        assert!(!t.rendered.contains("\"type\""));
        assert!(!t.rendered.contains("totalTokens"));
    }

    /// The rendering must survive the shape a REAL run produces: roles kept
    /// honest, tool call + output visible, and the streaming chunks — which
    /// dominated the raw stream — absent.
    #[test]
    fn real_stream_renders_roles_tool_activity_and_no_chunk_spam() {
        let t = parse(REALISTIC);
        let r = &t.rendered;
        // The prompt is the USER's message, not the assistant's words.
        assert!(r.contains("user: Create hello.txt"), "rendered: {r}");
        assert!(
            !r.contains("assistant: Create hello.txt"),
            "the prompt must not be attributed to the assistant: {r}"
        );
        // The tool call: name plus the informative argument.
        assert!(
            r.contains("[tool] bash: ls /tmp | head -3"),
            "rendered: {r}"
        );
        // Its output, rendered exactly once.
        assert_eq!(r.matches("af_dog_run.log").count(), 1, "rendered: {r}");
        // Thinking is summarized as one bounded line, not streamed.
        assert!(
            r.contains("[thinking] The user wants a file; I will write it."),
            "rendered: {r}"
        );
        // No streaming chunk is rendered as a line of its own.
        for chunk in [
            "thinking_delta",
            "thinking_start",
            "toolcall_delta",
            "toolcall_start",
            "text_delta",
            "text_start",
        ] {
            assert!(!r.contains(chunk), "chunk event {chunk} leaked into: {r}");
        }
        // 16 streamed events must not become 16 lines.
        let lines_rendered = r.lines().count();
        assert!(
            lines_rendered <= 12,
            "the rendering must stay compact, got {lines_rendered} lines: {r}"
        );
        assert!(!r.contains("\"type\"") && !r.contains("totalTokens"));
    }

    /// A CLI-internal `tool_execution_*` event must not double-render the
    /// output that its `toolResult` message already carries.
    #[test]
    fn tool_execution_events_do_not_duplicate_the_tool_result() {
        let t = parse(REALISTIC);
        assert_eq!(t.rendered.matches("af_dog_state").count(), 1);
        assert_eq!(t.rendered.matches("[toolResult]").count(), 1);
    }

    #[test]
    fn usage_comes_from_the_last_event_that_carries_one() {
        // Two completed messages: the second (last) usage is the truth.
        let stream = concat!(
            "{\"type\":\"message_end\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"first\"}],\"usage\":{\"input\":10,\"output\":1,\"totalTokens\":11}}}\n",
            "{\"type\":\"message_end\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}],\"usage\":{\"input\":20,\"output\":2,\"totalTokens\":22}}}\n",
        );
        let t = parse(stream);
        assert_eq!(t.usage.map(|u| u.total_tokens), Some(22));
        assert!(t.rendered.contains("assistant: first"));
        assert!(t.rendered.contains("assistant: second"));
    }

    #[test]
    fn non_json_lines_and_unknown_events_are_ignored() {
        let stream = concat!(
            "\n",
            "this is not json at all\n",
            "{\"type\":\"something_new\",\"payload\":\"whatever\"}\n",
            "{\"not\":\"an event object\"}\n",
            "[1,2,3]\n",
        );
        let t = parse(stream);
        assert_eq!(t.usage, None);
        assert_eq!(t.rendered, "", "nothing renderable was observed");
    }

    #[test]
    fn truncated_stream_renders_the_deltas_and_yields_no_usage() {
        // The CLI died mid-message: only deltas arrived, no message_end.
        let stream = concat!(
            "{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
            "{\"type\":\"message_update\",\"usage\":{\"input\":5,\"output\":0,\"totalTokens\":5},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"partial \"}}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"answer\"}}\n",
        );
        let t = parse(stream);
        assert!(t.rendered.contains("assistant (truncated): partial answer"));
        // The streaming usage was superseded by nothing — but a truncated
        // stream's last word is still reported (it is real spend).
        assert_eq!(t.usage.map(|u| u.total_tokens), Some(5));
    }

    #[test]
    fn empty_and_session_only_streams_have_no_usage() {
        assert_eq!(parse("").usage, None);
        let t = parse("{\"type\":\"session\",\"sessionId\":\"x\"}\n");
        assert_eq!(t.usage, None);
        assert_eq!(t.rendered, "");
    }

    #[test]
    fn malformed_event_payloads_never_panic() {
        // Structurally wrong payloads are skipped, not crashed on.
        let stream = concat!(
            "{\"type\":\"message_update\"}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":\"not an object\"}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":42}}\n",
            "{\"type\":\"message_end\"}\n",
            "{\"type\":\"message_end\",\"message\":\"not an object\"}\n",
            "{\"type\":\"message_end\",\"message\":{\"content\":\"not an array\"}}\n",
            "{\"type\":\"message_end\",\"message\":{\"content\":[{\"type\":\"tool_use\"}]}}\n",
            "{\"type\":\"turn_end\",\"usage\":\"not an object\"}\n",
        );
        let t = parse(stream);
        assert_eq!(t.usage, None);
        // The tool_use content part carries no text — nothing rendered.
        assert_eq!(t.rendered, "");
    }

    #[test]
    fn final_message_content_wins_over_the_streamed_deltas() {
        let stream = concat!(
            "{\"type\":\"message_start\",\"message\":{}}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"text_delta\",\"delta\":\"streaming guess\"}}\n",
            "{\"type\":\"message_end\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"the real answer\"}],\"usage\":{\"input\":1,\"output\":1,\"totalTokens\":2}}}\n",
        );
        let t = parse(stream);
        assert!(t.rendered.contains("assistant: the real answer"));
        assert!(!t.rendered.contains("streaming guess"));
    }

    #[test]
    fn multiple_text_parts_are_joined() {
        let stream = concat!(
            "{\"type\":\"message_end\",\"message\":{\"content\":[",
            "{\"type\":\"text\",\"text\":\"part one\"},",
            "{\"type\":\"text\",\"text\":\"part two\"}],",
            "\"usage\":{\"input\":3,\"output\":4,\"totalTokens\":7}}}\n",
        );
        let t = parse(stream);
        assert!(t.rendered.contains("assistant: part one\npart two"));
        assert_eq!(t.usage.map(|u| u.total_tokens), Some(7));
    }

    #[test]
    fn usage_without_total_tokens_falls_back_to_input_plus_output() {
        let t = parse("{\"type\":\"turn_end\",\"usage\":{\"input\":40,\"output\":2}}\n");
        assert_eq!(
            t.usage,
            Some(Usage {
                input: 40,
                output: 2,
                total_tokens: 42
            })
        );
    }

    #[test]
    fn usage_numbers_may_arrive_as_floats() {
        let t = parse(
            "{\"type\":\"turn_end\",\"usage\":{\"input\":10.0,\"output\":2.0,\"totalTokens\":12.0}}\n",
        );
        assert_eq!(t.usage.map(|u| u.total_tokens), Some(12));
    }

    #[test]
    fn unknown_assistant_activity_is_kept_only_when_labelled() {
        let stream = concat!(
            // Unlabelled streaming of an event type the parser does not know:
            // dropped, so it cannot become a line per chunk.
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"future_delta\",\"contentIndex\":0}}\n",
            // Labelled and not a streaming chunk: kept, one compact line.
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"web_search\",\"title\":\"docs.rs\"}}\n",
        );
        let t = parse(stream);
        assert_eq!(
            t.rendered, "[web_search] docs.rs",
            "rendered: {}",
            t.rendered
        );
    }

    #[test]
    fn argument_summaries_handle_every_shape() {
        // A bare string argument (no wrapper object).
        let s = serde_json::json!({"toolCall": {"name": "bash", "arguments": "ls -la"}});
        assert_eq!(tool_call_line(&s), "[tool] bash: ls -la");
        // Explicit null, and no arguments key at all: the bare tool name.
        let null = serde_json::json!({"toolCall": {"name": "bash", "arguments": null}});
        assert_eq!(tool_call_line(&null), "[tool] bash");
        // A whitespace-only argument is not worth printing.
        let blank = serde_json::json!({"toolCall": {"name": "bash", "arguments": "   \n "}});
        assert_eq!(tool_call_line(&blank), "[tool] bash");
        // An argument shape with none of the informative keys: raw JSON, but
        // still a single bounded line.
        let other = serde_json::json!({"toolCall": {"name": "x", "arguments": {"n": 3}}});
        assert_eq!(tool_call_line(&other), "[tool] x: {\"n\":3}");
        // A non-object, non-string argument.
        let num = serde_json::json!({"toolCall": {"name": "x", "arguments": 7}});
        assert_eq!(tool_call_line(&num), "[tool] x: 7");
        // No name and no title: still a usable line rather than a panic.
        assert_eq!(tool_call_line(&serde_json::json!({})), "[tool] tool");
    }

    #[test]
    fn an_empty_thinking_block_still_reports_that_it_happened() {
        let stream = concat!(
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_end\",\"content\":\"   \"}}\n",
            "{\"type\":\"message_update\",\"assistantMessageEvent\":{\"type\":\"thinking_end\"}}\n",
        );
        let t = parse(stream);
        assert_eq!(
            t.rendered, "[thinking]\n[thinking]",
            "rendered: {}",
            t.rendered
        );
    }

    #[test]
    fn labelled_activity_prefers_title_then_name_and_drops_unlabelled() {
        let titled =
            labelled_activity(&serde_json::json!({"type": "tool_call", "title": "read file"}));
        assert_eq!(titled.as_deref(), Some("[tool_call] read file"));
        let named = labelled_activity(&serde_json::json!({"type": "tool_call", "name": "bash"}));
        assert_eq!(named.as_deref(), Some("[tool_call] bash"));
        // An unlabelled event is not worth a line: rendering these
        // unconditionally is how a log fills with `[reasoning_delta]` noise.
        assert_eq!(labelled_activity(&serde_json::json!({"type": "x"})), None);
        assert_eq!(labelled_activity(&serde_json::json!({})), None);
    }

    #[test]
    fn tool_calls_render_the_tool_name_and_the_informative_argument() {
        // The real shape: the finished call under `toolCall`.
        let call = serde_json::json!({
            "type": "toolcall_end",
            "toolCall": {"type": "toolCall", "id": "c1", "name": "bash",
                         "arguments": {"command": "cargo test --all"}}
        });
        assert_eq!(tool_call_line(&call), "[tool] bash: cargo test --all");
        // A file-oriented tool: the path is the informative part.
        let read = serde_json::json!({
            "type": "toolcall_end",
            "toolCall": {"name": "read", "arguments": {"filePath": "/tmp/a.rs"}}
        });
        assert_eq!(tool_call_line(&read), "[tool] read: /tmp/a.rs");
        // A long argument is bounded, at a char boundary.
        let long = serde_json::json!({
            "type": "toolcall_end",
            "toolCall": {"name": "bash", "arguments": {"command": "x".repeat(400)}}
        });
        let line = tool_call_line(&long);
        assert!(line.ends_with('…'), "truncation is marked: {line}");
        let width = line.chars().count();
        assert!(width <= 180, "bounded: {width}");
        // No arguments at all: still a usable line.
        let named_only = serde_json::json!({"toolCall": {"name": "todo"}});
        assert_eq!(tool_call_line(&named_only), "[tool] todo");
    }
}

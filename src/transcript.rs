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
//! humans: the assistant text is rendered as prose (from the final message
//! content, or from the `text_delta` chunks when the stream was truncated
//! mid-message) and every other observed activity (tool calls, …) stays
//! visible as one compact line — raw JSONL in the log would be a
//! debuggability regression.

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
                        // Any other assistant activity (a tool call and
                        // friends) stays visible as one compact line.
                        Some(other) => lines.push(compact_activity(other, ev)),
                        None => {}
                    }
                }
            }
            "message_end" => {
                note_usage(&v, &mut t);
                if let Some(msg) = v.get("message") {
                    // pi carries the usage inside the message object here.
                    note_usage(msg, &mut t);
                    let final_text = message_text(msg);
                    let text = if final_text.trim().is_empty() {
                        pending.clone()
                    } else {
                        final_text
                    };
                    if !text.trim().is_empty() {
                        lines.push(format!("assistant: {text}"));
                    }
                }
                pending.clear();
            }
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

/// Compact one-line rendering of non-text assistant activity: the event
/// type plus its title/name when one is present.
fn compact_activity(kind: &str, ev: &Value) -> String {
    match ev
        .get("title")
        .or_else(|| ev.get("name"))
        .and_then(Value::as_str)
    {
        Some(label) => format!("[{kind}] {label}"),
        None => format!("[{kind}]"),
    }
}

/// Concatenated `text` parts of a final assistant message (the fully
/// formed text is authoritative over the streamed deltas).
fn message_text(msg: &Value) -> String {
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

    /// A realistic transcript, exactly the event order a real
    /// `pi --mode json` run produces: session → message_start → a tool
    /// call → two text deltas (PARTIAL usage) → message_end (full usage)
    /// → turn_end → agent_end → agent_settled. The final three events
    /// carry the SAME total — the number a receipt must record.
    const REALISTIC: &str = concat!(
        "{\"type\":\"session\",\"sessionId\":\"s1\"}\n",
        "{\"type\":\"message_start\",\"message\":{\"role\":\"assistant\",\"content\":[]}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"cacheRead\":0,\"cacheWrite\":0,\"reasoning\":0,\"totalTokens\":101,\"cost\":{\"total\":0}},\"assistantMessageEvent\":{\"type\":\"tool_call\",\"toolCallId\":\"c1\",\"title\":\"inspect worktree\"}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\"ok\"}}\n",
        "{\"type\":\"message_update\",\"usage\":{\"input\":100,\"output\":1,\"totalTokens\":101},\"assistantMessageEvent\":{\"type\":\"text_delta\",\"contentIndex\":0,\"delta\":\", done\"}}\n",
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
        // … the tool call stays visible in compact form …
        assert!(t.rendered.contains("[tool_call] inspect worktree"));
        // … the usage is summarized …
        assert!(t.rendered.contains("usage: 13523 in, 2 out, 13525 total"));
        // … and NO raw JSONL survives into the rendering.
        assert!(!t.rendered.contains("\"type\""));
        assert!(!t.rendered.contains("totalTokens"));
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
    fn compact_activity_prefers_title_then_name() {
        let titled = compact_activity(
            "tool_call",
            &serde_json::json!({"type": "tool_call", "title": "read file"}),
        );
        assert_eq!(titled, "[tool_call] read file");
        let named = compact_activity(
            "tool_call",
            &serde_json::json!({"type": "tool_call", "name": "bash"}),
        );
        assert_eq!(named, "[tool_call] bash");
        let bare = compact_activity("reasoning_delta", &serde_json::json!({"type": "x"}));
        assert_eq!(bare, "[reasoning_delta]");
    }
}

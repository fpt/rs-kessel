//! Goal tracking: a session-scoped completion condition the agent works toward
//! across turns until an evaluator confirms it is met (modelled on Claude Code's
//! `/goal`). The evaluator is a plain, tool-less LLM call that judges the
//! condition against the recent conversation transcript and returns yes/no + a
//! short reason. The Swift/Windows frontends drive the turn loop; this module
//! owns the state and the evaluation prompt/parse.

use crate::llm::{ChatMessage, ChatRole};
use std::time::Instant;

/// How many of the most recent messages the evaluator sees.
pub const EVAL_CONTEXT_MESSAGES: usize = 24;

/// Session-scoped goal state.
pub struct GoalState {
    pub condition: String,
    pub started_at: Instant,
    pub turns_evaluated: u32,
    pub last_reason: Option<String>,
}

impl GoalState {
    pub fn new(condition: String) -> Self {
        Self {
            condition,
            started_at: Instant::now(),
            turns_evaluated: 0,
            last_reason: None,
        }
    }
}

/// Render recent messages as a plain transcript for the evaluator.
pub fn format_transcript(messages: &[ChatMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = match m.role {
            ChatRole::System => continue, // system prompts/skills aren't evidence
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
            ChatRole::Tool => "ToolResult",
        };
        let content = m.content.trim();
        if content.is_empty() {
            continue;
        }
        out.push_str(role);
        out.push_str(": ");
        out.push_str(content);
        out.push_str("\n\n");
    }
    out
}

/// Build the two-message evaluator prompt (no tools, no skills).
pub fn build_eval_messages(condition: &str, transcript: &str) -> Vec<ChatMessage> {
    let system = "You are a strict completion evaluator. Decide whether the GOAL \
        has been FULLY satisfied, judging ONLY from the conversation transcript \
        provided. You cannot run commands or read files — rely solely on what the \
        transcript already demonstrates (e.g. a test run that passed, a clean build, \
        a command's output). When in doubt, answer that it is NOT met. Reply with a \
        single JSON object and nothing else: \
        {\"met\": true|false, \"reason\": \"<one short sentence>\"}."
        .to_string();

    let user = format!(
        "GOAL:\n{}\n\nTRANSCRIPT (oldest first, most recent last):\n{}\n\nIs the goal \
         fully satisfied? Reply with the JSON object only.",
        condition.trim(),
        if transcript.trim().is_empty() {
            "(no conversation yet)"
        } else {
            transcript.trim()
        }
    );

    vec![ChatMessage::system(system), ChatMessage::user(user)]
}

/// Parse an evaluator reply into `(met, reason)`. Lenient: prefers a JSON object,
/// falls back to a leading YES/NO. Ambiguous replies are treated as NOT met so a
/// goal never completes by accident (the turn cap bounds the loop instead).
pub fn parse_evaluation(raw: &str) -> (bool, String) {
    let cleaned = strip_think(raw);
    let trimmed = cleaned.trim();

    if let Some(obj) = extract_json_object(trimmed) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&obj) {
            if let Some(met) = v.get("met").and_then(|m| m.as_bool()) {
                let reason = v
                    .get("reason")
                    .and_then(|r| r.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| default_reason(met));
                return (met, reason);
            }
        }
    }

    // Fallback: scan the leading word for an explicit decision.
    let lower = trimmed.to_lowercase();
    let says_yes = leads_with(&lower, "yes") || leads_with(&lower, "met") || leads_with(&lower, "true");
    let says_no = leads_with(&lower, "no") || leads_with(&lower, "not") || leads_with(&lower, "false");
    let met = says_yes && !says_no;
    let reason = if trimmed.is_empty() {
        default_reason(met)
    } else {
        first_sentence(trimmed)
    };
    (met, reason)
}

fn default_reason(met: bool) -> String {
    if met {
        "Condition satisfied.".to_string()
    } else {
        "Condition not yet satisfied.".to_string()
    }
}

/// True if `s` begins with `word` as a whole word (next char is non-alphanumeric).
fn leads_with(s: &str, word: &str) -> bool {
    s.strip_prefix(word)
        .map(|rest| !rest.chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false))
        .unwrap_or(false)
}

/// First sentence (up to '.', '\n', or 200 chars) for a compact reason.
fn first_sentence(s: &str) -> String {
    let end = s
        .find(['.', '\n'])
        .map(|i| i + 1)
        .unwrap_or(s.len())
        .min(200);
    let end = s.floor_char_boundary(end);
    s[..end].trim().to_string()
}

/// Strip `<think>...</think>` reasoning blocks emitted by some local models.
fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out, // unterminated — drop the rest
        }
    }
    out.push_str(rest);
    out
}

/// Extract the first balanced `{...}` JSON object substring, if any.
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        match c {
            '"' if !escaped => in_str = !in_str,
            '\\' if in_str => {
                escaped = !escaped;
                continue;
            }
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
        escaped = false;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_met() {
        let (met, reason) = parse_evaluation(r#"{"met": true, "reason": "All tests pass."}"#);
        assert!(met);
        assert_eq!(reason, "All tests pass.");
    }

    #[test]
    fn parses_json_not_met_with_surrounding_text() {
        let (met, reason) =
            parse_evaluation("Here is my decision:\n{\"met\": false, \"reason\": \"lint failed\"}\nThanks");
        assert!(!met);
        assert_eq!(reason, "lint failed");
    }

    #[test]
    fn strips_think_block_then_parses() {
        let (met, _) = parse_evaluation("<think>let me check the tests...</think>{\"met\": true, \"reason\": \"ok\"}");
        assert!(met);
    }

    #[test]
    fn fallback_leading_yes_no() {
        assert!(parse_evaluation("YES — the build is clean").0);
        assert!(!parse_evaluation("No, two tests still fail").0);
        // "notable" must not count as "no"
        assert!(parse_evaluation("notable progress, but {\"met\": true, \"reason\":\"done\"}").0);
    }

    #[test]
    fn ambiguous_defaults_to_not_met() {
        assert!(!parse_evaluation("I think maybe it's close").0);
        assert!(!parse_evaluation("").0);
    }

    #[test]
    fn transcript_skips_system_and_empty() {
        let msgs = vec![
            ChatMessage::system("you are a helper".into()),
            ChatMessage::user("do X".into()),
            ChatMessage::assistant("done X".into()),
        ];
        let t = format_transcript(&msgs);
        assert!(!t.contains("helper"));
        assert!(t.contains("User: do X"));
        assert!(t.contains("Assistant: done X"));
    }
}

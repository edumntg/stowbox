//! Full-text search across the persisted agent transcripts.
//!
//! Matching is case-insensitive on the text a person reads in the chat: their
//! messages, the model's text and reasoning, tool calls (name and input
//! values), tool output, turn errors and stderr notes. JSON keys and ids are
//! not searched. Each transcript is streamed line by line, never loaded
//! whole: a first pass over the raw lines skips files that cannot contain the
//! query without parsing anything, and the second pass parses events and
//! stops keeping hits once the limits are reached. An event's index is its
//! position among the parsed events, as `Store::read_events` returns them,
//! so the UI can find it in the loaded transcript.

use crate::agent::{AgentEvent, Block};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::BufRead;
use std::path::Path;

/// Characters of context kept before the match in a snippet.
const BEFORE: usize = 60;
/// Characters of snippet in total, the match included, when there is room.
const SNIPPET: usize = 180;
/// Longest query taken; anything after is ignored.
pub const QUERY_MAX: usize = 200;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLimits {
    /// Most hits returned in total.
    pub max_hits: usize,
    /// Most hits kept per agent (its latest ones).
    pub per_agent: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self { max_hits: 50, per_agent: 5 }
    }
}

/// Where in an event the match was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HitKind {
    User,
    Assistant,
    Thinking,
    ToolCall,
    ToolOutput,
    Error,
    Note,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub agent_id: String,
    /// Index of the event among the agent's parsed events.
    pub event: usize,
    /// The turn it belongs to: how many user messages came up to it (0 for
    /// output before the first message).
    pub turn: u32,
    pub kind: HitKind,
    /// The snippet in three parts, whitespace collapsed; `before` starts and
    /// `after` ends with "…" when the text was cut.
    pub before: String,
    pub matched: String,
    pub after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// More matches exist than were returned.
    pub truncated: bool,
    /// Transcripts read (agents without a transcript file are not counted).
    pub scanned: u32,
}

/// A query ready to match: lower-cased, trimmed, cut at [`QUERY_MAX`].
#[derive(Debug, Clone)]
pub struct Needle {
    chars: Vec<char>,
    ascii: Option<Vec<u8>>,
    /// The query appears verbatim in a raw JSON line wherever it appears in
    /// the decoded text: serde_json escapes only quotes, backslashes and
    /// control characters.
    raw_safe: bool,
}

impl Needle {
    pub fn new(query: &str) -> Option<Self> {
        let q: String = query.trim().chars().take(QUERY_MAX).collect::<String>().to_lowercase();
        if q.is_empty() {
            return None;
        }
        let raw_safe = !q.chars().any(|c| c == '"' || c == '\\' || c.is_control());
        let ascii = q.is_ascii().then(|| q.as_bytes().to_vec());
        Some(Self { chars: q.chars().collect(), ascii, raw_safe })
    }

    /// The byte range of the first case-insensitive match in `hay`.
    pub fn find(&self, hay: &str) -> Option<(usize, usize)> {
        if let Some(n) = &self.ascii {
            // ASCII bytes never occur inside a multi-byte character, so the
            // offsets are character boundaries.
            let h = hay.as_bytes();
            if n.len() > h.len() {
                return None;
            }
            let first = n[0];
            return (0..=h.len() - n.len())
                .find(|&i| h[i].to_ascii_lowercase() == first && h[i..i + n.len()].eq_ignore_ascii_case(n))
                .map(|i| (i, i + n.len()));
        }
        let first = self.chars[0];
        for (i, c) in hay.char_indices() {
            if !lower_eq(c, first) {
                continue;
            }
            let mut end = i;
            let mut rest = hay[i..].char_indices();
            let ok = self.chars.iter().all(|&n| match rest.next() {
                Some((j, h)) if lower_eq(h, n) => {
                    end = i + j + h.len_utf8();
                    true
                }
                _ => false,
            });
            if ok {
                return Some((i, end));
            }
        }
        None
    }
}

fn lower_eq(c: char, lower: char) -> bool {
    if c.is_ascii() {
        return c.to_ascii_lowercase() == lower;
    }
    let mut l = c.to_lowercase();
    l.next() == Some(lower) && l.next().is_none()
}

/// The searchable texts of one event, with what they are.
fn texts(ev: &AgentEvent) -> Vec<(HitKind, String)> {
    match ev {
        AgentEvent::UserMessage { text, .. } => vec![(HitKind::User, text.clone())],
        AgentEvent::Assistant { blocks } => blocks
            .iter()
            .map(|b| match b {
                Block::Text { text } => (HitKind::Assistant, text.clone()),
                Block::Thinking { text } => (HitKind::Thinking, text.clone()),
                Block::ToolUse { name, input, .. } => {
                    let mut s = name.clone();
                    input_strings(input, &mut s);
                    (HitKind::ToolCall, s)
                }
            })
            .collect(),
        AgentEvent::ToolResult { content, .. } => vec![(HitKind::ToolOutput, content.clone())],
        AgentEvent::Result { message: Some(m), ok: false, .. } => vec![(HitKind::Error, m.clone())],
        AgentEvent::Stderr { text } => vec![(HitKind::Note, text.clone())],
        _ => Vec::new(),
    }
}

/// The string values in a tool's input (commands, paths, patterns), space
/// separated, in the order they appear.
fn input_strings(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push(' ');
            out.push_str(s);
        }
        Value::Array(a) => a.iter().for_each(|x| input_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| input_strings(x, out)),
        _ => {}
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cuts `text` around the match at `start..end` into before, match, after.
pub fn snippet(text: &str, start: usize, end: usize) -> (String, String, String) {
    let head = &text[..start];
    let tail = &text[end..];
    let matched = collapse(&text[start..end]);
    let head_chars = head.chars().count();
    let mut before = if head_chars > BEFORE {
        let skip = head.char_indices().nth(head_chars - BEFORE).map(|(i, _)| i).unwrap_or(0);
        format!("…{}", collapse(&head[skip..]).trim_start())
    } else {
        collapse(head)
    };
    if head.ends_with(char::is_whitespace) && !before.is_empty() {
        before.push(' ');
    }
    let room = SNIPPET.saturating_sub(before.chars().count() + matched.chars().count());
    let mut after = String::new();
    if tail.starts_with(char::is_whitespace) && !tail.trim().is_empty() {
        after.push(' ');
    }
    let tail_trim = tail.trim_start();
    let tail_chars = tail_trim.chars().count();
    if tail_chars > room {
        let cut = tail_trim.char_indices().nth(room).map(|(i, _)| i).unwrap_or(tail_trim.len());
        after.push_str(collapse(&tail_trim[..cut]).trim_end());
        after.push('…');
    } else {
        after.push_str(&collapse(tail_trim));
    }
    (before, matched, after)
}

/// The raw lines contain the query somewhere (keys and escapes included).
/// Only meaningful for a [`Needle`] that is raw-safe.
fn raw_has(reader: impl BufRead, needle: &Needle) -> std::io::Result<bool> {
    for line in split_lines(reader) {
        if needle.find(&line?).is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn split_lines(mut reader: impl BufRead) -> impl Iterator<Item = std::io::Result<String>> {
    let mut buf = Vec::new();
    std::iter::from_fn(move || {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                Some(Ok(String::from_utf8_lossy(&buf).into_owned()))
            }
            Err(e) => Some(Err(e)),
        }
    })
}

/// Searches one transcript. Returns its latest `keep` hits, newest first,
/// and whether older ones were dropped.
pub fn search_reader(reader: impl BufRead, agent_id: &str, needle: &Needle, keep: usize) -> std::io::Result<(Vec<SearchHit>, bool)> {
    let mut hits: VecDeque<SearchHit> = VecDeque::new();
    let mut dropped = false;
    let mut index = 0usize;
    let mut turn = 0u32;
    for line in split_lines(reader) {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<AgentEvent>(&line) else { continue };
        if matches!(ev, AgentEvent::UserMessage { .. }) {
            turn += 1;
        }
        // The raw line is a cheap first look: an event whose JSON lacks the
        // query cannot contain it once decoded.
        if !needle.raw_safe || needle.find(&line).is_some() {
            if let Some((kind, text, (s, e))) = texts(&ev).into_iter().find_map(|(k, t)| needle.find(&t).map(|r| (k, t, r))) {
                let (before, matched, after) = snippet(&text, s, e);
                hits.push_back(SearchHit { agent_id: agent_id.to_string(), event: index, turn, kind, before, matched, after });
                if hits.len() > keep {
                    hits.pop_front();
                    dropped = true;
                }
            }
        }
        index += 1;
    }
    Ok((hits.into_iter().rev().collect(), dropped))
}

/// Searches the transcripts of `agent_ids`, in that order (newest agent
/// first is what the UI wants), in `dir/<id>.jsonl`. Missing files are
/// skipped.
pub fn search_dir(dir: &Path, agent_ids: &[String], query: &str, limits: SearchLimits) -> crate::Result<SearchResults> {
    let mut out = SearchResults::default();
    let Some(needle) = Needle::new(query) else { return Ok(out) };
    if limits.max_hits == 0 || limits.per_agent == 0 {
        return Ok(out);
    }
    for id in agent_ids {
        let path = dir.join(format!("{id}.jsonl"));
        let open = || std::fs::File::open(&path).map(|f| std::io::BufReader::with_capacity(64 * 1024, f));
        let file = match open() {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        out.scanned += 1;
        if needle.raw_safe && !raw_has(file, &needle)? {
            continue;
        }
        let (hits, dropped) = search_reader(open()?, id, &needle, limits.per_agent)?;
        out.truncated |= dropped;
        let room = limits.max_hits - out.hits.len();
        if hits.len() > room {
            out.truncated = true;
        }
        out.hits.extend(hits.into_iter().take(room));
        if out.hits.len() >= limits.max_hits {
            // Agents after this one were not read; they may hold more.
            out.truncated |= agent_ids.last() != Some(id);
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jsonl(events: &[AgentEvent]) -> String {
        events.iter().map(|e| serde_json::to_string(e).unwrap() + "\n").collect()
    }

    fn user(t: &str) -> AgentEvent {
        AgentEvent::UserMessage { text: t.into(), attachments: vec![] }
    }

    fn says(t: &str) -> AgentEvent {
        AgentEvent::Assistant { blocks: vec![Block::Text { text: t.into() }] }
    }

    #[test]
    fn matches_case_insensitively_in_ascii_and_unicode() {
        let n = Needle::new("  LoGin ").unwrap();
        assert_eq!(n.find("Fix the LOGIN bug"), Some((8, 13)));
        assert_eq!(n.find("logi"), None);
        let n = Needle::new("ÉTÉ").unwrap();
        let hay = "un bel Été ici";
        let (s, e) = n.find(hay).unwrap();
        assert_eq!(&hay[s..e], "Été");
        assert!(Needle::new("   ").is_none());
    }

    #[test]
    fn ascii_needle_skips_multibyte_text_safely() {
        let n = Needle::new("ok").unwrap();
        let hay = "ñandú → OK";
        let (s, e) = n.find(hay).unwrap();
        assert_eq!(&hay[s..e], "OK");
    }

    #[test]
    fn snippet_cuts_long_text_and_collapses_whitespace() {
        let text = format!("{}\n\nthe   needle\tis here{}", "a ".repeat(100), " b".repeat(200));
        let (s, e) = Needle::new("needle").unwrap().find(&text).unwrap();
        let (before, matched, after) = snippet(&text, s, e);
        assert!(before.starts_with('…'), "{before}");
        assert!(before.ends_with("the "), "{before:?}");
        assert!(before.chars().count() <= BEFORE + 2);
        assert_eq!(matched, "needle");
        assert!(after.starts_with(" is here b"), "{after:?}");
        assert!(after.ends_with('…'));
        assert!(before.chars().count() + matched.chars().count() + after.chars().count() <= SNIPPET + 3);
        assert!(!before.contains('\n') && !after.contains('\t'));

        let (b, m, a) = snippet("needle", 0, 6);
        assert_eq!((b.as_str(), m.as_str(), a.as_str()), ("", "needle", ""));
    }

    #[test]
    fn finds_events_with_index_turn_and_kind() {
        let events = vec![
            AgentEvent::Init { session_id: "s".into(), model: None, cwd: None },
            user("Fix the login bug"),
            AgentEvent::Assistant {
                blocks: vec![
                    Block::Thinking { text: "look at auth".into() },
                    Block::ToolUse { id: "t1".into(), name: "Bash".into(), input: json!({"command": "grep -rn Login src/"}) },
                ],
            },
            AgentEvent::ToolResult { tool_use_id: "t1".into(), content: "src/auth.rs:12: fn login()".into(), is_error: false },
            says("The login handler drops the cookie."),
            user("thanks"),
            AgentEvent::Result { ok: false, subtype: "error".into(), message: Some("login quota hit".into()), cost_usd: None, duration_ms: None, turns: None, session_id: None, input_tokens: None, output_tokens: None },
        ];
        let n = Needle::new("LOGIN").unwrap();
        let (hits, dropped) = search_reader(jsonl(&events).as_bytes(), "a1", &n, 10).unwrap();
        assert!(!dropped);
        let got: Vec<_> = hits.iter().map(|h| (h.event, h.turn, h.kind)).collect();
        assert_eq!(got, vec![(6, 2, HitKind::Error), (4, 1, HitKind::Assistant), (3, 1, HitKind::ToolOutput), (2, 1, HitKind::ToolCall), (1, 1, HitKind::User)]);
        assert_eq!(hits[1].matched, "login");
        assert_eq!(hits[3].before, "Bash grep -rn ");
        assert!(hits.iter().all(|h| h.agent_id == "a1"));
    }

    #[test]
    fn json_keys_and_ids_do_not_match() {
        let events = vec![AgentEvent::ToolResult { tool_use_id: "toolu_text".into(), content: "nothing here".into(), is_error: false }, says("plain")];
        for q in ["tool_use_id", "toolu_text", "assistant", "type"] {
            let (hits, _) = search_reader(jsonl(&events).as_bytes(), "a", &Needle::new(q).unwrap(), 10).unwrap();
            assert!(hits.is_empty(), "{q} matched");
        }
    }

    #[test]
    fn quotes_and_newlines_in_the_query_still_match() {
        let events = vec![says("she said \"hi\"\nthen left")];
        let (hits, _) = search_reader(jsonl(&events).as_bytes(), "a", &Needle::new("said \"HI\"").unwrap(), 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched, "said \"hi\"");
    }

    #[test]
    fn index_skips_blank_and_broken_lines_like_read_events() {
        let text = format!("{}\n\n{{\"type\":\"nope\"}}\n{{truncated\n{}", serde_json::to_string(&user("one")).unwrap(), serde_json::to_string(&says("two needle")).unwrap());
        let (hits, _) = search_reader(text.as_bytes(), "a", &Needle::new("needle").unwrap(), 10).unwrap();
        assert_eq!(hits[0].event, 1);
    }

    #[test]
    fn keeps_the_latest_hits_per_agent() {
        let events: Vec<_> = (0..10).map(|i| says(&format!("needle {i}"))).collect();
        let (hits, dropped) = search_reader(jsonl(&events).as_bytes(), "a", &Needle::new("needle").unwrap(), 3).unwrap();
        assert!(dropped);
        assert_eq!(hits.iter().map(|h| h.event).collect::<Vec<_>>(), vec![9, 8, 7]);
    }

    #[test]
    fn searches_a_directory_in_order_with_limits() {
        let dir = std::env::temp_dir().join(format!("stowbox-search-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("new.jsonl"), jsonl(&[user("deploy the API"), says("Deployed.")])).unwrap();
        std::fs::write(dir.join("old.jsonl"), jsonl(&[user("deploy docs"), says("nothing to do")])).unwrap();
        std::fs::write(dir.join("none.jsonl"), jsonl(&[user("unrelated")])).unwrap();
        let ids: Vec<String> = ["new", "missing", "none", "old"].iter().map(|s| s.to_string()).collect();

        let r = search_dir(&dir, &ids, "DEPLOY", SearchLimits::default()).unwrap();
        assert_eq!(r.scanned, 3);
        assert!(!r.truncated);
        let got: Vec<_> = r.hits.iter().map(|h| (h.agent_id.as_str(), h.event)).collect();
        assert_eq!(got, vec![("new", 1), ("new", 0), ("old", 0)]);

        let r = search_dir(&dir, &ids, "deploy", SearchLimits { max_hits: 2, per_agent: 5 }).unwrap();
        assert_eq!(r.hits.len(), 2);
        assert!(r.truncated);
        assert_eq!(r.scanned, 1);

        assert!(search_dir(&dir, &ids, " ", SearchLimits::default()).unwrap().hits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_transcripts_stream_without_parsing_misses() {
        let dir = std::env::temp_dir().join(format!("stowbox-search-big-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = "x".repeat(4096);
        let events: Vec<_> = (0..500).map(|_| AgentEvent::ToolResult { tool_use_id: "t".into(), content: big.clone(), is_error: false }).collect();
        let body = jsonl(&events);
        let ids: Vec<String> = (0..20).map(|i| format!("a{i}")).collect();
        for id in &ids {
            std::fs::write(dir.join(format!("{id}.jsonl")), &body).unwrap();
        }
        let t = std::time::Instant::now();
        let r = search_dir(&dir, &ids, "needle", SearchLimits::default()).unwrap();
        assert!(r.hits.is_empty());
        assert_eq!(r.scanned, 20);
        // About 40 MB read; generous bound so slow CI machines pass.
        assert!(t.elapsed() < std::time::Duration::from_secs(20), "{:?}", t.elapsed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

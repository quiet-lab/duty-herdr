//! Screen confirmation for programmatic prompt submissions.
//!
//! A submitted prompt is confirmed when the tail of its text newly appears in the bottom
//! rows of the pane. Agents wrap their input line to the pane width and indent or frame
//! continuation rows, so both sides drop whitespace and box-drawing characters before
//! comparing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use super::terminal::PaneTerminal;

/// Graphemes from the end of the submitted text that must appear on screen.
const ECHO_TAIL_GRAPHEMES: usize = 24;

/// Placeholders some agents print instead of a long or multi-line paste, already
/// normalized: Claude Code shows `[Pasted text #1 +12 lines]`, Codex shows
/// `[Pasted Content 1234 chars]`. A new placeholder confirms the paste like the text would.
const PASTE_PLACEHOLDER_MARKERS: &[&str] = &["[Pastedtext#", "[PastedContent"];

/// Bottom rows of the pane the probe reads. The tail spans at most two rows of the input
/// line, and Claude Code and Codex draw a frame plus a few footer rows (status line, hints)
/// below it; twelve rows keep about a double margin over that layout.
const ECHO_PROBE_ROWS: usize = 12;

/// Returns how many new copies of the text are on screen; see `SubmissionEcho::probe`.
pub(crate) type EchoProbe = Box<dyn FnMut() -> usize + Send + 'static>;

/// Builds a probe that counts copies of `text` that reached the screen after this call.
///
/// The baseline is taken now, before the text is written, so a copy of the same text
/// already visible on screen does not confirm the new submission. The probe runs after
/// every PTY read while a submission waits; it reads the screen only when `content_seq`
/// moved since its last read and then formats only the bottom `ECHO_PROBE_ROWS` rows, so a
/// chatty program does not pay for a full-screen snapshot on each read.
pub(crate) fn echo_probe(
    terminal: Arc<PaneTerminal>,
    content_seq: Arc<AtomicU64>,
    text: &str,
) -> EchoProbe {
    let needle = echo_tail(text);
    if needle.is_empty() {
        return Box::new(|| 1);
    }
    let mut scratch = String::new();
    // Loaded before the read, so a write racing the baseline triggers a new read.
    let mut seen_seq = content_seq.load(Ordering::Acquire);
    let baseline = EchoCounts::of(&screen_bottom(&terminal), &needle, &mut scratch);
    let mut copies = 0;
    Box::new(move || {
        let seq = content_seq.load(Ordering::Acquire);
        if seq != seen_seq {
            seen_seq = seq;
            copies = EchoCounts::of(&screen_bottom(&terminal), &needle, &mut scratch)
                .new_copies(baseline);
        }
        copies
    })
}

fn screen_bottom(terminal: &PaneTerminal) -> String {
    terminal.recent_text_snapshot(ECHO_PROBE_ROWS).text
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EchoCounts {
    tail: usize,
    placeholders: usize,
}

impl EchoCounts {
    /// Counts on `screen`, normalized into the reused `scratch` buffer.
    fn of(screen: &str, needle: &str, scratch: &mut String) -> Self {
        scratch.clear();
        scratch.extend(normalized_chars(screen));
        Self {
            tail: scratch.matches(needle).count(),
            placeholders: PASTE_PLACEHOLDER_MARKERS
                .iter()
                .map(|marker| scratch.matches(marker).count())
                .sum(),
        }
    }

    /// New copies over `baseline`, by the text tail or by paste placeholders.
    fn new_copies(self, baseline: Self) -> usize {
        self.tail
            .saturating_sub(baseline.tail)
            .max(self.placeholders.saturating_sub(baseline.placeholders))
    }
}

fn normalized_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_box_drawing(*ch))
}

fn normalize(text: &str) -> String {
    normalized_chars(text).collect()
}

fn is_box_drawing(ch: char) -> bool {
    ('\u{2500}'..='\u{257F}').contains(&ch)
}

fn echo_tail(text: &str) -> String {
    let normalized = normalize(text);
    let graphemes: Vec<&str> = normalized.graphemes(true).collect();
    let start = graphemes.len().saturating_sub(ECHO_TAIL_GRAPHEMES);
    graphemes[start..].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_last_graphemes_without_whitespace() {
        assert_eq!(echo_tail("  a b\n c  "), "abc");
        let long = "x".repeat(10) + &"y".repeat(ECHO_TAIL_GRAPHEMES);
        assert_eq!(echo_tail(&long), "y".repeat(ECHO_TAIL_GRAPHEMES));
        assert_eq!(echo_tail("é".repeat(30).as_str()).chars().count(), 24);
        assert_eq!(echo_tail(" \n\t "), "");
    }

    fn counts(screen: &str, needle: &str) -> EchoCounts {
        EchoCounts::of(screen, needle, &mut String::new())
    }

    #[test]
    fn wrapped_input_line_matches_the_tail() {
        let text = "please review the branch and report findings";
        let screen = "> please review the branch and\n  report findings\n─────────";
        let needle = echo_tail(text);
        let before = counts("> \n─────────", &needle);
        assert_eq!(counts(screen, &needle).new_copies(before), 1);
    }

    #[test]
    fn text_already_on_screen_needs_a_new_copy() {
        let needle = echo_tail("same prompt");
        let before = counts("> same prompt\n\n> ", &needle);
        assert_eq!(counts("> same prompt\n\n> ", &needle).new_copies(before), 0);
        assert_eq!(
            counts("> same prompt\n\n> same prompt", &needle).new_copies(before),
            1
        );
    }

    #[test]
    fn doubled_input_line_counts_two_copies() {
        let needle = echo_tail("review it");
        let before = counts("> ", &needle);
        assert_eq!(
            counts("> review itreview it", &needle).new_copies(before),
            2
        );
        assert_eq!(
            counts(
                "> [Pasted text #1 +3 lines][Pasted text #2 +3 lines]",
                &needle
            )
            .new_copies(before),
            2
        );
    }

    #[test]
    fn paste_placeholder_confirms_collapsed_paste() {
        let needle = echo_tail("line one\nline two\nline three");
        let before = counts("> ", &needle);
        assert_eq!(
            counts("> [Pasted text #1 +3 lines]", &needle).new_copies(before),
            1
        );
        assert_eq!(
            counts("› [Pasted Content 1234 chars]", &needle).new_copies(before),
            1
        );
    }

    #[tokio::test]
    async fn input_echo_probe_reads_screen_only_after_content_changes() {
        let (runtime, _rx) = super::super::PaneRuntime::test_with_channel(80, 24);
        let mut probe = echo_probe(
            runtime.terminal.clone(),
            runtime.content_seq.clone(),
            "hello world",
        );
        assert_eq!(probe(), 0);
        // Parsed without a content revision: the probe keeps its last answer instead of
        // reading the screen again.
        let (tx, _responses) = tokio::sync::mpsc::channel(1);
        runtime
            .terminal
            .process_pty_bytes(runtime.pane_id, 0, b"> hello world", &tx);
        assert_eq!(probe(), 0);
        assert_eq!(probe(), 0);
        runtime
            .content_seq
            .fetch_add(2, std::sync::atomic::Ordering::AcqRel);
        assert_eq!(probe(), 1);
    }

    #[tokio::test]
    async fn input_echo_probe_reads_only_bottom_rows() {
        let (runtime, _rx) = super::super::PaneRuntime::test_with_channel(80, 24);
        let mut probe = echo_probe(
            runtime.terminal.clone(),
            runtime.content_seq.clone(),
            "hello world",
        );
        // A copy pushed above the bottom rows by later output is outside the probe.
        let mut screen = b"> hello world\r\n".to_vec();
        for _ in 0..ECHO_PROBE_ROWS {
            screen.extend_from_slice(b"output\r\n");
        }
        runtime.test_process_pty_bytes(&screen);
        assert_eq!(probe(), 0);
        runtime.test_process_pty_bytes(b"> hello world");
        assert_eq!(probe(), 1);
    }
}

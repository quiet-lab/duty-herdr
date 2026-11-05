//! Screen confirmation for programmatic prompt submissions.
//!
//! A submitted prompt is confirmed when the tail of its text newly appears in the
//! bottom-of-buffer detection text. Agents wrap their input line to the pane width and
//! indent or frame continuation rows, so both sides drop whitespace and box-drawing
//! characters before comparing.

use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

use super::terminal::PaneTerminal;

/// Graphemes from the end of the submitted text that must appear on screen.
const ECHO_TAIL_GRAPHEMES: usize = 24;

/// Placeholders some agents print instead of a long or multi-line paste, already
/// normalized: Claude Code shows `[Pasted text #1 +12 lines]`, Codex shows
/// `[Pasted Content 1234 chars]`. A new placeholder confirms the paste like the text would.
const PASTE_PLACEHOLDER_MARKERS: &[&str] = &["[Pastedtext#", "[PastedContent"];

pub(crate) type EchoProbe = Box<dyn FnMut() -> bool + Send + 'static>;

/// Builds a probe that reports whether `text` reached the screen after this call.
///
/// The baseline is taken now, before the text is written, so a copy of the same text
/// already visible on screen does not confirm the new submission.
pub(crate) fn echo_probe(terminal: Arc<PaneTerminal>, text: &str) -> EchoProbe {
    let needle = echo_tail(text);
    if needle.is_empty() {
        return Box::new(|| true);
    }
    let baseline = EchoCounts::of(&terminal.detection_text(), &needle);
    Box::new(move || EchoCounts::of(&terminal.detection_text(), &needle).exceeds(baseline))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EchoCounts {
    tail: usize,
    placeholders: usize,
}

impl EchoCounts {
    fn of(screen: &str, needle: &str) -> Self {
        let screen = normalize(screen);
        Self {
            tail: screen.matches(needle).count(),
            placeholders: PASTE_PLACEHOLDER_MARKERS
                .iter()
                .map(|marker| screen.matches(marker).count())
                .sum(),
        }
    }

    fn exceeds(self, baseline: Self) -> bool {
        self.tail > baseline.tail || self.placeholders > baseline.placeholders
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_box_drawing(*ch))
        .collect()
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

    #[test]
    fn wrapped_input_line_matches_the_tail() {
        let text = "please review the branch and report findings";
        let screen = "> please review the branch and\n  report findings\n─────────";
        let needle = echo_tail(text);
        let before = EchoCounts::of("> \n─────────", &needle);
        let after = EchoCounts::of(screen, &needle);
        assert!(after.exceeds(before));
    }

    #[test]
    fn text_already_on_screen_needs_a_new_copy() {
        let needle = echo_tail("same prompt");
        let before = EchoCounts::of("> same prompt\n\n> ", &needle);
        assert!(!EchoCounts::of("> same prompt\n\n> ", &needle).exceeds(before));
        assert!(EchoCounts::of("> same prompt\n\n> same prompt", &needle).exceeds(before));
    }

    #[test]
    fn paste_placeholder_confirms_collapsed_paste() {
        let needle = echo_tail("line one\nline two\nline three");
        let before = EchoCounts::of("> ", &needle);
        assert!(EchoCounts::of("> [Pasted text #1 +3 lines]", &needle).exceeds(before));
        assert!(EchoCounts::of("› [Pasted Content 1234 chars]", &needle).exceeds(before));
    }
}

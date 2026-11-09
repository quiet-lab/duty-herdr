//! Evaluates a pane's cursor shape rules when the pane's output changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use super::terminal::PaneTerminal;
use crate::config::{ScreenCursorShapeConfig, ScreenShapeDecision};
use crate::layout::PaneId;
use crate::render_signal::RenderSignal;

/// Wakes on output instead of the detection tick: a mode change is one short
/// screen update and the tick would delay the new shape by up to 300 ms. Panes
/// whose agent has no rules pay one relaxed load per PTY read and own no task.
#[derive(Default)]
pub(super) struct ScreenCursorEvaluator {
    /// Rules are bound; lets rule-less panes skip the lock on every agent sync.
    bound: AtomicBool,
    /// The rules read the screen, so output must wake the evaluation.
    watches_output: AtomicBool,
    wake: Notify,
    rules: Mutex<Option<Arc<ScreenCursorShapeConfig>>>,
    task: Mutex<Option<tokio::task::AbortHandle>>,
}

/// Render wake-up for a shape change, which no terminal content change announces.
#[derive(Clone)]
pub(crate) struct ScreenCursorRender {
    pub(crate) dirty: Arc<RenderSignal>,
    pub(crate) notify: Arc<Notify>,
}

impl ScreenCursorRender {
    fn request(&self, pane_id: PaneId) {
        if self.dirty.request_pty(pane_id) {
            self.notify.notify_one();
        }
    }
}

impl ScreenCursorEvaluator {
    pub(super) fn observe_output(&self) {
        if self.watches_output.load(Ordering::Relaxed) {
            self.wake.notify_one();
        }
    }

    pub(super) fn sync_rules(
        self: &Arc<Self>,
        rules: Option<Arc<ScreenCursorShapeConfig>>,
        pane_id: PaneId,
        terminal: &Arc<PaneTerminal>,
        render: &ScreenCursorRender,
    ) {
        if rules.is_none() && !self.bound.load(Ordering::Relaxed) {
            return;
        }
        let mut current = self.lock_rules();
        let unchanged = match (current.as_ref(), rules.as_ref()) {
            (None, None) => true,
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            _ => false,
        };
        if unchanged {
            return;
        }
        self.bound.store(rules.is_some(), Ordering::Relaxed);
        self.watches_output.store(
            rules.as_ref().is_some_and(|rules| !rules.rules.is_empty()),
            Ordering::Relaxed,
        );
        *current = rules;
        if current.is_some() {
            drop(current);
            self.ensure_task(pane_id, terminal, render);
            self.wake.notify_one();
        } else if terminal.set_screen_cursor_shape(None) {
            render.request(pane_id);
        }
    }

    /// Reads outside the rules lock; the result is dropped when the rules were
    /// replaced meanwhile, so a late evaluation cannot restore a cleared shape.
    pub(super) fn evaluate(
        &self,
        pane_id: PaneId,
        terminal: &PaneTerminal,
        render: &ScreenCursorRender,
    ) {
        let Some(rules) = self.lock_rules().clone() else {
            return;
        };
        // A synchronized frame is incomplete; its closing bytes wake this again.
        if terminal.synchronized_output_active() {
            return;
        }
        // A table with only `default` does not depend on the screen.
        let decision = if rules.rules.is_empty() {
            rules.shape_for_screen("")
        } else {
            rules.shape_for_screen(&terminal.detection_text())
        };
        let current = self.lock_rules();
        if !current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &rules))
        {
            return;
        }
        // `keep` leaves the shape the previous screen selected; before any
        // shape is applied it takes the table's default.
        let changed = match decision {
            ScreenShapeDecision::Set(shape) => terminal.set_screen_cursor_shape(shape),
            ScreenShapeDecision::Keep { fallback } => {
                terminal.set_screen_cursor_shape_if_unset(fallback)
            }
        };
        if changed {
            render.request(pane_id);
        }
    }

    pub(super) fn abort(&self) {
        let mut task = self.task.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(handle) = task.take() {
            handle.abort();
        }
    }

    fn ensure_task(
        self: &Arc<Self>,
        pane_id: PaneId,
        terminal: &Arc<PaneTerminal>,
        render: &ScreenCursorRender,
    ) {
        let mut task = self.task.lock().unwrap_or_else(|err| err.into_inner());
        if task.is_some() {
            return;
        }
        let evaluator = self.clone();
        let terminal = terminal.clone();
        let render = render.clone();
        *task = Some(
            tokio::spawn(async move {
                loop {
                    evaluator.wake.notified().await;
                    evaluator.evaluate(pane_id, &terminal, &render);
                }
            })
            .abort_handle(),
        );
    }

    fn lock_rules(&self) -> std::sync::MutexGuard<'_, Option<Arc<ScreenCursorShapeConfig>>> {
        self.rules.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PaneRuntime;
    use std::time::Duration;

    // The test passes on the wake-up event; this only bounds a hang. A live run on
    // the workstation delivered the shape in 12-18 ms; no CI stand runs this fork, so
    // the bound is far above any doubled measurement rather than derived from one.
    const WAKE_HANG_BOUND: Duration = Duration::from_secs(5);

    fn render() -> ScreenCursorRender {
        ScreenCursorRender {
            dirty: Arc::new(RenderSignal::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    fn rules(text: &str) -> Option<Arc<ScreenCursorShapeConfig>> {
        Some(Arc::new(toml::from_str(text).expect("rules")))
    }

    const MARKER_BAR: &str = r#"rules = [{ contains = "MARKER", shape = "steady_bar" }]"#;

    fn sync(
        runtime: &PaneRuntime,
        rules: Option<Arc<ScreenCursorShapeConfig>>,
        render: &ScreenCursorRender,
    ) {
        runtime.sync_screen_cursor_rules(rules, render);
    }

    fn evaluate(runtime: &PaneRuntime, render: &ScreenCursorRender) {
        runtime
            .screen_cursor
            .evaluate(runtime.pane_id, &runtime.terminal, render);
    }

    fn shape(runtime: &PaneRuntime) -> u8 {
        runtime.terminal.cursor_state().expect("cursor").shape
    }

    #[tokio::test]
    async fn matching_rule_wins_over_own_shape_until_it_stops_matching() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"");
        runtime.test_process_pty_bytes(b"\x1b[3 qMARKER");
        let render = render();
        let own = shape(&runtime);
        assert_ne!(own, 0);
        sync(&runtime, rules(MARKER_BAR), &render);
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 6);

        runtime.test_process_pty_bytes(b"\x1b[2J\x1b[Hother");
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), own);
    }

    #[tokio::test]
    async fn default_applies_without_match_and_lifting_rules_restores_zero() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"other");
        let render = render();
        assert_eq!(shape(&runtime), 0);
        sync(
            &runtime,
            rules(&format!("default = \"steady_block\"\n{MARKER_BAR}")),
            &render,
        );
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 2);

        render.dirty.take();
        sync(&runtime, None, &render);
        assert_eq!(shape(&runtime), 0);
        assert!(render.dirty.is_pending());
    }

    #[tokio::test]
    async fn shape_change_requests_a_render_only_when_it_changes() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"MARKER");
        let render = render();
        sync(&runtime, rules(MARKER_BAR), &render);
        evaluate(&runtime, &render);
        assert!(render.dirty.is_pending());

        render.dirty.take();
        evaluate(&runtime, &render);
        assert!(!render.dirty.is_pending());
    }

    #[tokio::test]
    async fn output_wakes_the_evaluation_task() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"other");
        let render = render();
        sync(&runtime, rules(MARKER_BAR), &render);

        runtime.test_process_pty_bytes(b"\r\nMARKER");
        runtime.screen_cursor.observe_output();
        tokio::time::timeout(WAKE_HANG_BOUND, render.notify.notified())
            .await
            .expect("shape change wakes the renderer");
        assert_eq!(shape(&runtime), 6);
    }

    const CLAUDE_FOOTER: &str = r#"
default = "steady_block"
rules = [
  { contains = "-- INSERT --", shape = "steady_bar", after_last_line_containing = "────", top_non_empty_lines = 1 },
  { contains = "Press Ctrl-C again to exit", shape = "keep", after_last_line_containing = "────", top_non_empty_lines = 1 },
]"#;

    fn screen_with_status(status: &str) -> Vec<u8> {
        format!("\x1b[2J\x1b[H────────\r\n❯ hi\r\n────────\r\n{status}").into_bytes()
    }

    #[tokio::test]
    async fn keep_rule_leaves_the_shape_of_the_previous_screen() {
        for (before, expected) in [("  -- INSERT --", 6), ("  ⏵⏵ bypass", 2)] {
            let runtime = PaneRuntime::test_with_screen_bytes(40, 10, b"");
            let render = render();
            sync(&runtime, rules(CLAUDE_FOOTER), &render);
            runtime.test_process_pty_bytes(&screen_with_status(before));
            evaluate(&runtime, &render);
            assert_eq!(shape(&runtime), expected);

            runtime.test_process_pty_bytes(&screen_with_status("  Press Ctrl-C again to exit"));
            evaluate(&runtime, &render);
            assert_eq!(shape(&runtime), expected);
        }
    }

    #[tokio::test]
    async fn keep_on_a_fresh_binding_applies_the_default() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 10, b"");
        let render = render();
        runtime.test_process_pty_bytes(&screen_with_status("  Press Ctrl-C again to exit"));
        sync(&runtime, rules(CLAUDE_FOOTER), &render);
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 2);
    }

    #[tokio::test]
    async fn output_with_insert_label_above_footer_lines_selects_the_bar() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 10, b"");
        let render = render();
        sync(&runtime, rules(CLAUDE_FOOTER), &render);
        runtime.test_process_pty_bytes(&screen_with_status(
            "  -- INSERT --\r\n\r\n  ● main\r\n  ◯ agent",
        ));
        runtime.screen_cursor.observe_output();
        tokio::time::timeout(WAKE_HANG_BOUND, render.notify.notified())
            .await
            .expect("shape change wakes the renderer");
        assert_eq!(shape(&runtime), 6);
    }

    #[tokio::test]
    async fn synchronized_frame_is_evaluated_once_it_closes() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"other");
        let render = render();
        sync(&runtime, rules(MARKER_BAR), &render);

        runtime.test_process_pty_bytes(b"\x1b[?2026h\r\nMARKER");
        evaluate(&runtime, &render);
        runtime.test_process_pty_bytes(b"\x1b[?2026l");
        assert_eq!(shape(&runtime), 0);
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 6);
    }

    #[tokio::test]
    async fn pane_without_rules_owns_no_task_and_ignores_output() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"MARKER");
        let render = render();
        sync(&runtime, None, &render);
        runtime.screen_cursor.observe_output();
        assert!(runtime.screen_cursor.task.lock().unwrap().is_none());
        assert!(!render.dirty.is_pending());
    }

    #[tokio::test]
    async fn default_only_table_does_not_watch_output_or_read_the_screen() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"other");
        let render = render();
        sync(&runtime, rules("default = \"steady_bar\""), &render);
        assert!(!runtime.screen_cursor.watches_output.load(Ordering::Relaxed));
        let reads = runtime
            .terminal
            .ghostty
            .detection_text_reads
            .load(Ordering::Relaxed);
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 6);
        assert_eq!(
            runtime
                .terminal
                .ghostty
                .detection_text_reads
                .load(Ordering::Relaxed),
            reads
        );
    }

    /// Cost of one evaluation of a full screen, the work added per PTY read of a
    /// pane whose agent has rules. Run with `just bench-render-scale`.
    #[tokio::test]
    #[ignore = "profile"]
    async fn render_scale_profile_screen_cursor_evaluation() {
        const ITERATIONS: usize = 1000;
        // The footer rules scan the screen from the bottom to the input box border.
        const FOOTER_RULES: &str = r#"rules = [
  { contains = "-- INSERT --", shape = "steady_bar", after_last_line_containing = "────", top_non_empty_lines = 1 },
  { contains = "Press Ctrl-C again to exit", shape = "keep", after_last_line_containing = "────", top_non_empty_lines = 1 },
]"#;
        for (name, table, footer) in [
            ("marker", MARKER_BAR, false),
            ("footer", FOOTER_RULES, true),
        ] {
            for (cols, rows) in [(80u16, 24u16), (200, 60)] {
                let line = format!("{}\r\n", "x".repeat(usize::from(cols) - 1));
                let mut screen = line.repeat(usize::from(rows) - if footer { 4 } else { 0 });
                if footer {
                    let border = "─".repeat(usize::from(cols) - 1);
                    screen.push_str(&format!("{border}\r\n❯ hi\r\n{border}\r\n  ⏵⏵ status"));
                }
                let runtime = PaneRuntime::test_with_screen_bytes(cols, rows, screen.as_bytes());
                let render = render();
                sync(&runtime, rules(table), &render);
                let mut samples = Vec::with_capacity(ITERATIONS);
                for _ in 0..ITERATIONS {
                    let started = std::time::Instant::now();
                    evaluate(&runtime, &render);
                    samples.push(started.elapsed().as_nanos());
                }
                samples.sort_unstable();
                println!(
                    "screen_cursor_evaluation {name} {cols}x{rows} median_us={} p95_us={}",
                    samples[ITERATIONS / 2] / 1000,
                    samples[ITERATIONS * 95 / 100] / 1000
                );
            }
        }
    }

    #[tokio::test]
    async fn evaluation_of_replaced_rules_is_dropped() {
        let runtime = PaneRuntime::test_with_screen_bytes(40, 6, b"MARKER");
        let render = render();
        sync(&runtime, rules(MARKER_BAR), &render);
        sync(&runtime, None, &render);
        evaluate(&runtime, &render);
        assert_eq!(shape(&runtime), 0);
    }
}

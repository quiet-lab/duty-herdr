use std::fs;
use std::io::{self, Write};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::Direction;

use super::{App, Mode};

static NEXT_COMMAND_NAMESPACE: AtomicU64 = AtomicU64::new(1);

pub(super) fn new_command_namespace() -> String {
    let counter = NEXT_COMMAND_NAMESPACE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}-{counter}", std::process::id())
}

#[derive(Debug)]
pub(super) struct EndpointCommandRegistry {
    entries: Vec<EndpointCommand>,
}

#[derive(Debug)]
struct EndpointCommand {
    id: String,
    binding: crate::config::CustomCommandKeybind,
    action: crate::protocol::ClientShellCommandAction,
}

impl EndpointCommandRegistry {
    pub(super) fn new(bindings: &[crate::config::CustomCommandKeybind]) -> Self {
        let namespace = new_command_namespace();
        let entries = bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| EndpointCommand {
                action: binding.action.into(),
                id: format!("cmd_{namespace}_{index}"),
                binding: binding.clone(),
            })
            .collect();
        Self { entries }
    }
}

impl App {
    pub(crate) fn client_shell_keybindings_profile(&self) -> Option<&str> {
        self.client_shell_keybindings_profile.as_deref()
    }

    pub(crate) fn client_shell_command_manifest(&self) -> Vec<crate::protocol::ClientShellCommand> {
        self.endpoint_commands
            .entries
            .iter()
            .map(|entry| crate::protocol::ClientShellCommand {
                command_id: entry.id.clone(),
                binding_label: entry.binding.label.clone(),
                binding_labels: entry.binding.bindings.labels(),
                action: entry.action,
                description: entry.binding.description.clone(),
            })
            .collect()
    }

    pub(crate) fn resolve_client_shell_command(
        &self,
        id: &str,
    ) -> Option<crate::config::CustomCommandKeybind> {
        self.endpoint_commands
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.binding.clone())
    }

    pub(crate) fn handle_command_invoke(
        &mut self,
        id: String,
        params: crate::api::schema::CommandInvokeParams,
    ) -> String {
        let Some(binding) = self.resolve_client_shell_command(&params.command_id) else {
            return crate::app::api::responses::encode_error(
                id,
                "command_not_found",
                "custom command manifest is stale; reload configuration",
            );
        };
        if let Err((code, message)) = self.focus_client_shell_command_target(&params) {
            return crate::app::api::responses::encode_error(id, code, message);
        }
        // The pane the client had focused; validated above.
        let target = params
            .pane_id
            .as_deref()
            .and_then(|pane_id| self.parse_pane_id(pane_id));
        let selected_text = if binding.action == crate::config::CustomCommandAction::PluginAction {
            let Some(selection) = params.selection.as_ref() else {
                return self.execute_custom_command_response(id, &binding, None, target);
            };
            if params.pane_id.as_deref() != Some(selection.pane_id.as_str()) {
                return crate::app::api::responses::encode_error(
                    id,
                    "command_target_mismatch",
                    "command selection does not belong to the requested pane",
                );
            }
            match self.pane_selection_text(selection) {
                Ok(text) => Some(text),
                Err((code, message)) => {
                    return crate::app::api::responses::encode_error(id, code, message);
                }
            }
        } else {
            None
        };
        self.execute_custom_command_response(id, &binding, selected_text, target)
    }

    fn execute_custom_command_response(
        &mut self,
        id: String,
        binding: &crate::config::CustomCommandKeybind,
        selected_text: Option<String>,
        target: Option<(usize, crate::layout::PaneId)>,
    ) -> String {
        match self.execute_custom_command_binding(binding, selected_text, target) {
            Ok(()) => crate::app::api::responses::encode_success(
                id,
                crate::api::schema::ResponseResult::Ok {},
            ),
            Err(error) => {
                crate::app::api::responses::encode_error(id, "command_failed", error.to_string())
            }
        }
    }

    fn focus_client_shell_command_target(
        &mut self,
        params: &crate::api::schema::CommandInvokeParams,
    ) -> Result<(), (&'static str, String)> {
        if let Some(pane_id) = params.pane_id.as_deref() {
            let Some((workspace_index, pane)) = self.parse_pane_id(pane_id) else {
                return Err(("pane_not_found", format!("pane not found: {pane_id}")));
            };
            let Some(tab_index) =
                self.state.workspaces[workspace_index].find_tab_index_for_pane(pane)
            else {
                return Err(("pane_not_found", format!("pane not found: {pane_id}")));
            };
            self.validate_client_shell_command_parent_ids(params, workspace_index, tab_index)?;
            self.state.focus_pane_in_workspace(workspace_index, pane);
            return Ok(());
        }

        if let Some(tab_id) = params.tab_id.as_deref() {
            let Some((workspace_index, tab_index)) = self.parse_tab_id(tab_id) else {
                return Err(("tab_not_found", format!("tab not found: {tab_id}")));
            };
            self.validate_client_shell_command_workspace_id(params, workspace_index)?;
            self.state.switch_workspace_tab(workspace_index, tab_index);
            return Ok(());
        }

        if let Some(workspace_id) = params.workspace_id.as_deref() {
            let Some(workspace_index) = self.parse_workspace_id(workspace_id) else {
                return Err((
                    "workspace_not_found",
                    format!("workspace not found: {workspace_id}"),
                ));
            };
            self.state.switch_workspace(workspace_index);
        }
        Ok(())
    }

    fn validate_client_shell_command_parent_ids(
        &self,
        params: &crate::api::schema::CommandInvokeParams,
        workspace_index: usize,
        tab_index: usize,
    ) -> Result<(), (&'static str, String)> {
        self.validate_client_shell_command_workspace_id(params, workspace_index)?;
        if let Some(tab_id) = params.tab_id.as_deref() {
            let actual = self
                .public_tab_id(workspace_index, tab_index)
                .unwrap_or_default();
            if tab_id != actual {
                return Err((
                    "command_target_mismatch",
                    "command pane does not belong to the requested tab".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn validate_client_shell_command_workspace_id(
        &self,
        params: &crate::api::schema::CommandInvokeParams,
        workspace_index: usize,
    ) -> Result<(), (&'static str, String)> {
        if params
            .workspace_id
            .as_deref()
            .is_some_and(|workspace_id| workspace_id != self.public_workspace_id(workspace_index))
        {
            return Err((
                "command_target_mismatch",
                "command target does not belong to the requested workspace".to_owned(),
            ));
        }
        Ok(())
    }
    /// Runs a custom command. `target` is the pane a `type = "shell"` command
    /// runs for; without it the server's focused pane is used.
    pub(crate) fn execute_custom_command_binding(
        &mut self,
        binding: &crate::config::CustomCommandKeybind,
        selected_text: Option<String>,
        target: Option<(usize, crate::layout::PaneId)>,
    ) -> io::Result<()> {
        match binding.action {
            crate::config::CustomCommandAction::Shell => self.spawn_custom_command(binding, target),
            crate::config::CustomCommandAction::Pane => {
                self.spawn_pane_command(&binding.command, Vec::new())
            }
            crate::config::CustomCommandAction::Popup => self.spawn_custom_popup_command(binding),
            crate::config::CustomCommandAction::PluginAction => self
                .invoke_plugin_action_from_keybind(binding.command.clone(), selected_text)
                .map_err(io::Error::other),
        }
    }

    fn spawn_custom_popup_command(
        &mut self,
        binding: &crate::config::CustomCommandKeybind,
    ) -> io::Result<()> {
        self.spawn_popup_shell_command(
            &binding.command,
            None,
            self.custom_command_env().0,
            crate::app::popup::PopupGeometry {
                width: binding.width,
                height: binding.height,
            },
        )
    }

    pub(crate) fn custom_command_env(&self) -> (Vec<(String, String)>, Option<std::path::PathBuf>) {
        let pane_id = self
            .state
            .active
            .and_then(|ws_idx| self.state.workspaces.get(ws_idx)?.focused_pane_id());
        self.custom_command_env_for(self.state.active, pane_id)
    }

    /// Custom command variables for a workspace and pane: the socket, the
    /// executable and `HERDR_ACTIVE_*`. The working directory is the pane's.
    fn custom_command_env_for(
        &self,
        ws_idx: Option<usize>,
        pane_id: Option<crate::layout::PaneId>,
    ) -> (Vec<(String, String)>, Option<std::path::PathBuf>) {
        let mut env = vec![(
            crate::api::SOCKET_PATH_ENV_VAR.to_string(),
            crate::api::socket_path().display().to_string(),
        )];
        if let Ok(executable) = crate::platform::launch_executable() {
            env.push((
                "HERDR_BIN_PATH".to_string(),
                executable.display().to_string(),
            ));
        }

        let mut cwd = None;
        let Some(ws_idx) = ws_idx else {
            return (env, cwd);
        };
        env.push((
            "HERDR_ACTIVE_WORKSPACE_ID".to_string(),
            self.public_workspace_id(ws_idx),
        ));
        let Some(workspace) = self.state.workspaces.get(ws_idx) else {
            return (env, cwd);
        };
        let tab_idx = pane_id
            .and_then(|pane_id| workspace.find_tab_index_for_pane(pane_id))
            .unwrap_or_else(|| workspace.active_tab_index());
        if let Some(tab_id) = self.public_tab_id(ws_idx, tab_idx) {
            env.push(("HERDR_ACTIVE_TAB_ID".to_string(), tab_id));
        }
        if let Some(pane_id) = pane_id {
            if let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) {
                env.push(("HERDR_ACTIVE_PANE_ID".to_string(), public_pane_id));
            }
            if let Some(pane_cwd) = workspace.tabs.get(tab_idx).and_then(|tab| {
                tab.cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
            }) {
                env.push((
                    "HERDR_ACTIVE_PANE_CWD".to_string(),
                    pane_cwd.display().to_string(),
                ));
                if pane_cwd.is_dir() {
                    cwd = Some(pane_cwd);
                }
            }
        }
        (env, cwd)
    }

    fn spawn_custom_command(
        &mut self,
        binding: &crate::config::CustomCommandKeybind,
        target: Option<(usize, crate::layout::PaneId)>,
    ) -> std::io::Result<()> {
        let target = target.or_else(|| {
            let ws_idx = self.state.active?;
            Some((
                ws_idx,
                self.state.workspaces.get(ws_idx)?.focused_pane_id()?,
            ))
        });
        let (env, cwd) = match target {
            Some((ws_idx, pane_id)) => self.detached_command_env(ws_idx, pane_id),
            None => self.custom_command_env(),
        };
        // One run per binding: labels are unique because duplicate keys are disabled.
        let key = format!("key:{}", binding.label);
        self.spawn_detached_command(key, &binding.command, Some(binding.label.clone()), env, cwd)
            .map(|_| ())
    }

    /// Environment of a detached shell command run for a pane: the custom
    /// command variables (`HERDR_ACTIVE_*` name that pane) plus the identity a
    /// process inside the pane gets (`HERDR_ENV`, `HERDR_WORKSPACE_ID`,
    /// `HERDR_TAB_ID`, `HERDR_PANE_ID`). The working directory is the pane's.
    pub(crate) fn detached_command_env(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> (Vec<(String, String)>, Option<std::path::PathBuf>) {
        let (mut env, cwd) = self.custom_command_env_for(Some(ws_idx), Some(pane_id));
        let identity = env
            .iter()
            .filter_map(|(key, value)| {
                let identity_key = match key.as_str() {
                    "HERDR_ACTIVE_WORKSPACE_ID" => crate::integration::HERDR_WORKSPACE_ID_ENV_VAR,
                    "HERDR_ACTIVE_TAB_ID" => crate::integration::HERDR_TAB_ID_ENV_VAR,
                    "HERDR_ACTIVE_PANE_ID" => crate::integration::HERDR_PANE_ID_ENV_VAR,
                    _ => return None,
                };
                Some((identity_key.to_string(), value.clone()))
            })
            .collect::<Vec<_>>();
        env.push((
            crate::HERDR_ENV_VAR.to_string(),
            crate::HERDR_ENV_VALUE.to_string(),
        ));
        env.extend(identity);
        (env, cwd)
    }

    /// Starts `command` through the platform shell outside any pane, with no
    /// terminal and stdin closed. Returns `Ok(false)` without starting it while
    /// a command started under the same `key` is still running. stderr goes to
    /// an unlinked temporary file, so descendants that keep it open block
    /// nothing. A worker thread waits for the command: the exit frees the key,
    /// a non-zero status or a wait error becomes `DetachedCommandFailed` with
    /// the tail of stderr.
    pub(crate) fn spawn_detached_command(
        &mut self,
        key: String,
        command: &str,
        label: Option<String>,
        env: Vec<(String, String)>,
        cwd: Option<std::path::PathBuf>,
    ) -> io::Result<bool> {
        if self.running_detached_commands.contains(&key) {
            tracing::info!(?label, "detached command still running; press ignored");
            return Ok(false);
        }
        let stderr = DetachedCommandStderr::create()?;
        let mut process = crate::platform::detached_custom_command_process(command);
        for name in crate::pane::AGENT_SESSION_ENV_VARS {
            process.env_remove(name);
        }
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr.file.try_clone()?))
            .envs(env);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let child = process.spawn()?;
        tracing::info!(?label, pid = child.id(), "detached command started");
        let event_tx = self.event_tx.clone();
        let worker_key = key.clone();
        match crate::thread_spawn::spawn_named_with(
            "herdr-detached-command",
            (child, stderr),
            move |(child, stderr)| {
                wait_detached_command(child, stderr, worker_key, label, event_tx)
            },
        ) {
            Ok(()) => {
                self.running_detached_commands.insert(key);
            }
            Err(((child, _stderr), err)) => {
                // Without a waiter nobody reports the exit; reap it like any
                // other detached child and keep the key free.
                tracing::warn!(err = %err, "failed to spawn detached command waiter");
                self.detached_process_children.push(child);
            }
        }
        Ok(true)
    }

    pub(crate) fn open_focused_scrollback_in_editor(&mut self) -> std::io::Result<()> {
        let ws_idx = self
            .state
            .active
            .ok_or_else(|| std::io::Error::other("no active workspace"))?;
        let ws = self
            .state
            .workspaces
            .get(ws_idx)
            .ok_or_else(|| std::io::Error::other("active workspace disappeared"))?;
        let pane_id = ws
            .focused_pane_id()
            .ok_or_else(|| std::io::Error::other("no focused pane"))?;
        let scrollback = self
            .state
            .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
            .ok_or_else(|| std::io::Error::other("focused pane has no scrollback runtime"))?
            .recent_unwrapped_text_snapshot(usize::MAX)
            .text;

        let path = write_scrollback_temp_file(&scrollback)?;

        let argv = match crate::platform::scrollback_editor_argv(&path) {
            Ok(argv) => argv,
            Err(err) => {
                let _ = fs::remove_file(&path);
                return Err(err);
            }
        };
        let (env, _) = self.custom_command_env();
        let new_pane = match self.spawn_overlay_argv_command(&argv, None, env, vec![path.clone()]) {
            Ok((_, new_pane)) => new_pane,
            Err(err) => {
                let _ = fs::remove_file(&path);
                return Err(err);
            }
        };
        let terminal_id = new_pane.terminal.id.clone();
        self.terminal_runtimes
            .insert(terminal_id.clone(), new_pane.runtime);
        self.state
            .remove_alias_shadowed_by_new_pane(new_pane.pane_id);
        self.state.terminals.insert(terminal_id, new_pane.terminal);

        if let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) {
            self.state.toast = Some(crate::app::state::ToastNotification {
                kind: crate::app::state::ToastKind::Finished,
                title: "opened scrollback".to_string(),
                context: format!("focused pane {public_pane_id}"),
                position: None,
                target: None,
            });
        }
        Ok(())
    }

    fn spawn_pane_command(
        &mut self,
        command: &str,
        temp_files: Vec<std::path::PathBuf>,
    ) -> std::io::Result<()> {
        let Some(ws_idx) = self.state.active else {
            return Err(std::io::Error::other("no active workspace"));
        };
        let previous_focus_target = self.state.current_pane_focus_target();
        let (rows, cols) = self
            .state
            .new_pane_size(crate::ui::NewPanePlacement::ZoomedOverlay);
        let (env, _) = self.custom_command_env();
        let host_terminal_appearance = self.pane_host_terminal_appearance();

        let ws = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .ok_or_else(|| std::io::Error::other("active workspace disappeared"))?;
        let tab_idx = ws.active_tab_index();
        let previous_focus = ws
            .focused_pane_id()
            .ok_or_else(|| std::io::Error::other("no focused pane"))?;
        let previous_zoomed = ws.active_tab().map(|tab| tab.zoomed).unwrap_or(false);
        let cwd = ws.active_tab().and_then(|tab| {
            tab.cwd_for_pane(
                previous_focus,
                &self.state.terminals,
                &self.terminal_runtimes,
            )
        });
        let new_pane = ws.split_focused_command(
            Direction::Horizontal,
            rows,
            cols,
            cwd,
            command,
            env,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            host_terminal_appearance,
        )?;
        let new_pane_id = new_pane.pane_id;
        self.terminal_runtimes
            .insert(new_pane.terminal.id.clone(), new_pane.runtime);
        self.state
            .terminals
            .insert(new_pane.terminal.id.clone(), new_pane.terminal);
        let new_focus_target = crate::app::state::PaneFocusTarget {
            workspace_id: ws.id.clone(),
            pane_id: new_pane_id,
        };
        if previous_focus_target.as_ref() != Some(&new_focus_target) {
            self.state.previous_pane_focus = previous_focus_target;
        }
        ws.active_tab_mut()
            .expect("workspace must have an active tab")
            .layout
            .focus_pane(new_pane_id);
        ws.active_tab_mut()
            .expect("workspace must have an active tab")
            .zoomed = true;
        self.overlay_panes.insert(
            new_pane_id,
            super::OverlayPaneState {
                ws_idx,
                tab_idx,
                previous_focus,
                previous_zoomed,
                temp_files,
            },
        );
        self.state.remove_alias_shadowed_by_new_pane(new_pane_id);
        self.state.mode = Mode::Terminal;
        Ok(())
    }

    pub(crate) fn spawn_overlay_argv_command(
        &mut self,
        argv: &[String],
        cwd: Option<std::path::PathBuf>,
        extra_env: Vec<(String, String)>,
        temp_files: Vec<std::path::PathBuf>,
    ) -> std::io::Result<(usize, crate::workspace::NewPane)> {
        let Some(ws_idx) = self.state.active else {
            return Err(std::io::Error::other("no active workspace"));
        };
        let previous_focus_target = self.state.current_pane_focus_target();
        let (rows, cols) = self
            .state
            .new_pane_size(crate::ui::NewPanePlacement::ZoomedOverlay);

        let ws = self
            .state
            .workspaces
            .get(ws_idx)
            .ok_or_else(|| std::io::Error::other("active workspace disappeared"))?;
        let previous_focus = ws
            .focused_pane_id()
            .ok_or_else(|| std::io::Error::other("no focused pane"))?;
        let cwd = cwd.or_else(|| {
            ws.active_tab().and_then(|tab| {
                tab.cwd_for_pane(
                    previous_focus,
                    &self.state.terminals,
                    &self.terminal_runtimes,
                )
            })
        });

        let host_terminal_appearance = self.pane_host_terminal_appearance();
        let (tab_idx, new_pane, workspace_id) = {
            let ws = self
                .state
                .workspaces
                .get_mut(ws_idx)
                .ok_or_else(|| std::io::Error::other("active workspace disappeared"))?;
            let previous_zoomed = ws.active_tab().map(|tab| tab.zoomed).unwrap_or(false);
            let result = ws.split_pane_argv_command(
                previous_focus,
                Direction::Horizontal,
                rows,
                cols,
                cwd,
                argv,
                extra_env,
                self.state.pane_scrollback_limit_bytes,
                self.state.host_terminal_theme,
                host_terminal_appearance,
                true,
            );
            let (tab_idx, new_pane) = match result {
                Some(Ok(result)) => result,
                Some(Err(err)) => return Err(err),
                None => return Err(std::io::Error::other("focused pane disappeared")),
            };
            ws.tabs
                .get_mut(tab_idx)
                .ok_or_else(|| std::io::Error::other("plugin overlay tab disappeared"))?
                .zoomed = true;
            self.overlay_panes.insert(
                new_pane.pane_id,
                super::OverlayPaneState {
                    ws_idx,
                    tab_idx,
                    previous_focus,
                    previous_zoomed,
                    temp_files,
                },
            );
            (tab_idx, new_pane, ws.id.clone())
        };

        let new_focus_target = crate::app::state::PaneFocusTarget {
            workspace_id,
            pane_id: new_pane.pane_id,
        };
        if previous_focus_target.as_ref() != Some(&new_focus_target) {
            self.state.previous_pane_focus = previous_focus_target;
        }
        self.state.switch_workspace_tab(ws_idx, tab_idx);
        self.state.mode = Mode::Terminal;
        Ok((ws_idx, new_pane))
    }
}

/// Bytes of stderr kept for the failure notification.
const DETACHED_COMMAND_STDERR_TAIL_BYTES: u64 = 4096;
/// Characters of stderr shown in the failure notification.
const DETACHED_COMMAND_STDERR_TAIL_CHARS: usize = 200;

/// Temporary file that receives a detached command's stderr. It is unlinked
/// at once where the platform allows it and removed again when dropped.
struct DetachedCommandStderr {
    path: std::path::PathBuf,
    file: fs::File,
}

impl DetachedCommandStderr {
    fn create() -> io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("herdr-command-stderr-{}", new_command_namespace()));
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        // Unix keeps the open file; Windows refuses and removes it on drop.
        let _ = fs::remove_file(&path);
        Ok(Self { path, file })
    }

    fn tail(&mut self) -> Vec<u8> {
        use std::io::{Read, Seek, SeekFrom};
        let mut tail = Vec::new();
        let Ok(len) = self.file.seek(SeekFrom::End(0)) else {
            return tail;
        };
        let start = len.saturating_sub(DETACHED_COMMAND_STDERR_TAIL_BYTES);
        if self.file.seek(SeekFrom::Start(start)).is_ok() {
            let _ = self.file.read_to_end(&mut tail);
        }
        tail
    }
}

impl Drop for DetachedCommandStderr {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Waits for a detached command on its own thread and reports its exit.
fn wait_detached_command(
    mut child: std::process::Child,
    mut stderr: DetachedCommandStderr,
    key: String,
    label: Option<String>,
    event_tx: tokio::sync::mpsc::Sender<crate::events::AppEvent>,
) {
    let wait = child.wait();
    let _ = event_tx.blocking_send(crate::events::AppEvent::DetachedCommandExited { key });
    let status = match wait {
        Ok(status) if status.success() => {
            tracing::info!(?label, "detached command finished");
            return;
        }
        Ok(status) => status.to_string(),
        Err(err) => format!("wait failed: {err}"),
    };
    let stderr_tail = stderr_tail_text(&stderr.tail());
    let _ = event_tx.blocking_send(crate::events::AppEvent::DetachedCommandFailed {
        label,
        status,
        stderr_tail,
    });
}

/// The last characters of stderr with whitespace runs folded to one space.
fn stderr_tail_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let skip = folded
        .chars()
        .count()
        .saturating_sub(DETACHED_COMMAND_STDERR_TAIL_CHARS);
    folded.chars().skip(skip).collect()
}

fn write_scrollback_temp_file(content: &str) -> io::Result<std::path::PathBuf> {
    let mut last_collision = None;
    for attempt in 0..16 {
        let path = unique_scrollback_path(attempt);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(content.as_bytes())?;
                return Ok(path);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                last_collision = Some(err);
            }
            Err(err) => return Err(err),
        }
    }

    Err(last_collision.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "failed to create unique scrollback temp file",
        )
    }))
}

fn unique_scrollback_path(attempt: u32) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "herdr-scrollback-{}-{nanos}-{attempt}.txt",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    fn test_app() -> crate::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    fn binding(action: crate::config::CustomCommandAction) -> crate::config::CustomCommandKeybind {
        crate::config::CustomCommandKeybind {
            bindings: crate::config::ActionKeybinds::prefix("z"),
            label: "prefix+z".into(),
            command: "secret-command --token hidden".into(),
            action,
            description: Some("safe description".into()),
            width: None,
            height: None,
        }
    }

    fn install(app: &mut crate::app::App, binding: crate::config::CustomCommandKeybind) {
        app.endpoint_commands = super::EndpointCommandRegistry::new(&[binding]);
    }

    #[test]
    fn manifest_exposes_opaque_ids_without_command_text() {
        let mut app = test_app();
        install(&mut app, binding(crate::config::CustomCommandAction::Shell));
        let manifest = app.client_shell_command_manifest();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].binding_label, "prefix+z");
        assert_eq!(manifest[0].binding_labels, ["prefix+z"]);
        assert_eq!(manifest[0].description.as_deref(), Some("safe description"));
        assert!(!format!("{:?}", manifest).contains("secret-command"));
        assert_eq!(
            app.resolve_client_shell_command(&manifest[0].command_id)
                .map(|binding| binding.command),
            Some("secret-command --token hidden".into())
        );
    }

    #[test]
    fn stale_command_id_is_rejected_after_definition_changes() {
        let mut app = test_app();
        install(&mut app, binding(crate::config::CustomCommandAction::Shell));
        let old_id = app.client_shell_command_manifest()[0].command_id.clone();
        let mut replacement = binding(crate::config::CustomCommandAction::Shell);
        replacement.command = "replacement-command".into();
        install(&mut app, replacement);

        let response = app.handle_command_invoke(
            "request-1".into(),
            crate::api::schema::CommandInvokeParams {
                command_id: old_id,
                workspace_id: None,
                tab_id: None,
                pane_id: None,
                selection: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "command_not_found");
    }

    #[test]
    fn foreground_keybinding_projection_cannot_remap_command_ids() {
        let mut app = test_app();
        install(&mut app, binding(crate::config::CustomCommandAction::Shell));
        let command_id = app.client_shell_command_manifest()[0].command_id.clone();
        let mut foreground_binding = binding(crate::config::CustomCommandAction::Shell);
        foreground_binding.command = "different-foreground-command".into();
        app.state.keybinds.custom_commands = vec![foreground_binding];

        assert_eq!(
            app.resolve_client_shell_command(&command_id)
                .map(|binding| binding.command),
            Some("secret-command --token hidden".into())
        );
    }

    #[test]
    fn command_ids_do_not_alias_across_endpoint_restarts() {
        let mut old_app = test_app();
        install(
            &mut old_app,
            binding(crate::config::CustomCommandAction::Shell),
        );
        let old_id = old_app.client_shell_command_manifest()[0]
            .command_id
            .clone();

        let mut replacement_app = test_app();
        let mut replacement = binding(crate::config::CustomCommandAction::Shell);
        replacement.command = "replacement-command".into();
        install(&mut replacement_app, replacement);
        assert_ne!(
            replacement_app.client_shell_command_manifest()[0].command_id,
            old_id
        );
        let response = replacement_app.handle_command_invoke(
            "request-1".into(),
            crate::api::schema::CommandInvokeParams {
                command_id: old_id,
                workspace_id: None,
                tab_id: None,
                pane_id: None,
                selection: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "command_not_found");
    }

    #[tokio::test]
    async fn plugin_command_rejects_stale_client_selection_before_invocation() {
        let mut app = test_app();
        let workspace = crate::workspace::Workspace::test_new("plugin-selection");
        let pane_id = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.terminal_runtimes.insert(
            terminal_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b"selected text\n"),
        );
        let mut plugin = binding(crate::config::CustomCommandAction::PluginAction);
        plugin.command = "missing.plugin-action".into();
        install(&mut app, plugin);
        let command_id = app.client_shell_command_manifest()[0].command_id.clone();
        let workspace_id = app.public_workspace_id(0);
        let tab_id = app.public_tab_id(0, 0).unwrap();
        let pane_id = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_command_invoke(
            "request-selection".into(),
            crate::api::schema::CommandInvokeParams {
                command_id,
                workspace_id: Some(workspace_id),
                tab_id: Some(tab_id),
                pane_id: Some(pane_id.clone()),
                selection: Some(crate::api::schema::PaneSelectionReadParams {
                    pane_id,
                    anchor: crate::api::schema::PaneTextPoint { row: 0, col: 0 },
                    cursor: crate::api::schema::PaneTextPoint { row: 0, col: 7 },
                    content_revision: Some(u64::MAX),
                }),
            },
        );

        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "stale_content");
    }

    #[cfg(unix)]
    #[test]
    fn shell_command_invocation_executes_endpoint_owned_definition() {
        let mut app = test_app();
        let path = std::path::PathBuf::from(format!(
            "/var/tmp/herdr-command-invoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut command = binding(crate::config::CustomCommandAction::Shell);
        command.command = format!("printf invoked > {}", path.display());
        install(&mut app, command);
        let command_id = app.client_shell_command_manifest()[0].command_id.clone();

        let response = app.handle_command_invoke(
            "request-1".into(),
            crate::api::schema::CommandInvokeParams {
                command_id,
                workspace_id: None,
                tab_id: None,
                pane_id: None,
                selection: None,
            },
        );
        let success: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.result, crate::api::schema::ResponseResult::Ok {});
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invoked");
        let _ = std::fs::remove_file(path);
    }

    fn app_with_pane() -> (crate::app::App, String) {
        let mut app = test_app();
        let workspace = crate::workspace::Workspace::test_new("detached-command");
        let pane = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane_id = app.public_pane_id(0, pane).unwrap();
        (app, pane_id)
    }

    fn run_command(app: &mut crate::app::App, pane_id: &str, command: &str) -> serde_json::Value {
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "run".into(),
            method: crate::api::schema::Method::PaneRunCommand(
                crate::api::schema::PaneRunCommandParams {
                    pane_id: pane_id.into(),
                    command: command.into(),
                },
            ),
        });
        serde_json::from_str(&response).unwrap()
    }

    /// Next detached-command event; the worker thread sends them in order.
    fn next_detached_event(app: &mut crate::app::App) -> crate::events::AppEvent {
        loop {
            let event = app.event_rx.blocking_recv().expect("event channel open");
            if matches!(
                event,
                crate::events::AppEvent::DetachedCommandExited { .. }
                    | crate::events::AppEvent::DetachedCommandFailed { .. }
            ) {
                return event;
            }
        }
    }

    fn scratch_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "herdr-detached-{name}-{}-{}",
            std::process::id(),
            super::new_command_namespace()
        ))
    }

    #[cfg(unix)]
    #[test]
    fn pane_run_command_gets_the_pane_environment() {
        let (mut app, pane_id) = app_with_pane();
        let path = scratch_path("env");
        let response = run_command(&mut app, &pane_id, &format!("env > {}", path.display()));
        assert_eq!(response["result"]["type"], "pane_run_command");
        assert_eq!(response["result"]["run"]["started"], true);
        assert_eq!(response["result"]["run"]["pane_id"], pane_id.as_str());
        let event = next_detached_event(&mut app);
        assert!(matches!(
            event,
            crate::events::AppEvent::DetachedCommandExited { .. }
        ));
        app.handle_internal_event(event);
        assert!(app.running_detached_commands.is_empty());
        let env = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let workspace_id = app.public_workspace_id(0);
        let tab_id = app.public_tab_id(0, 0).unwrap();
        for line in [
            "HERDR_ENV=1".to_string(),
            format!("HERDR_PANE_ID={pane_id}"),
            format!("HERDR_TAB_ID={tab_id}"),
            format!("HERDR_WORKSPACE_ID={workspace_id}"),
            format!("HERDR_ACTIVE_PANE_ID={pane_id}"),
            format!("HERDR_SOCKET_PATH={}", crate::api::socket_path().display()),
        ] {
            assert!(
                env.lines().any(|env_line| env_line == line),
                "{line} in {env}"
            );
        }
    }

    #[test]
    fn pane_run_command_rejects_unknown_pane_and_empty_command() {
        let (mut app, pane_id) = app_with_pane();
        let response = run_command(&mut app, "w9:p9", "true");
        assert_eq!(response["error"]["code"], "pane_not_found");
        let response = run_command(&mut app, &pane_id, "  ");
        assert_eq!(response["error"]["code"], "invalid_params");
        assert!(app.running_detached_commands.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn running_command_text_is_not_started_twice() {
        let (mut app, pane_id) = app_with_pane();
        let fifo = scratch_path("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        // The command blocks until the test opens the FIFO for writing.
        let command = format!("cat {} > /dev/null", fifo.display());
        assert_eq!(
            run_command(&mut app, &pane_id, &command)["result"]["run"]["started"],
            true
        );
        assert_eq!(
            run_command(&mut app, &pane_id, &command)["result"]["run"]["started"],
            false
        );
        drop(std::fs::OpenOptions::new().write(true).open(&fifo).unwrap());
        let event = next_detached_event(&mut app);
        assert!(matches!(
            event,
            crate::events::AppEvent::DetachedCommandExited { .. }
        ));
        app.handle_internal_event(event);
        let _ = std::fs::remove_file(&fifo);
        assert_eq!(
            run_command(&mut app, &pane_id, "true")["result"]["run"]["started"],
            true
        );
        let event = next_detached_event(&mut app);
        app.handle_internal_event(event);
        assert!(app.running_detached_commands.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn failing_command_reports_status_and_stderr_tail() {
        let (mut app, pane_id) = app_with_pane();
        run_command(&mut app, &pane_id, "echo first >&2; echo boom >&2; exit 3");
        let exited = next_detached_event(&mut app);
        assert!(matches!(
            exited,
            crate::events::AppEvent::DetachedCommandExited { .. }
        ));
        let crate::events::AppEvent::DetachedCommandFailed {
            label,
            status,
            stderr_tail,
        } = next_detached_event(&mut app)
        else {
            panic!("expected a failure event");
        };
        assert_eq!(label, None);
        assert!(status.contains('3'), "{status}");
        assert_eq!(stderr_tail, "first boom");
    }

    #[cfg(unix)]
    #[test]
    fn failure_is_reported_while_a_descendant_keeps_stderr_open() {
        let (mut app, pane_id) = app_with_pane();
        let fifo = scratch_path("held-stderr");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        // The background `cat` inherits stderr and lives until the test opens
        // the FIFO; the failure must be reported without waiting for it.
        let command = format!("cat {} > /dev/null & echo boom >&2; exit 3", fifo.display());
        run_command(&mut app, &pane_id, &command);
        let exited = next_detached_event(&mut app);
        assert!(matches!(
            exited,
            crate::events::AppEvent::DetachedCommandExited { .. }
        ));
        let crate::events::AppEvent::DetachedCommandFailed { stderr_tail, .. } =
            next_detached_event(&mut app)
        else {
            panic!("expected a failure event");
        };
        assert_eq!(stderr_tail, "boom");
        drop(std::fs::OpenOptions::new().write(true).open(&fifo).unwrap());
        let _ = std::fs::remove_file(&fifo);
    }

    #[cfg(unix)]
    #[test]
    fn shell_keybinding_runs_for_the_client_pane() {
        let (mut app, pane_id) = app_with_pane();
        let path = scratch_path("invoke");
        let mut command = binding(crate::config::CustomCommandAction::Shell);
        command.command = format!("printf %s \"$HERDR_PANE_ID\" > {}", path.display());
        install(&mut app, command);
        let command_id = app.client_shell_command_manifest()[0].command_id.clone();
        let response = app.handle_command_invoke(
            "request-1".into(),
            crate::api::schema::CommandInvokeParams {
                command_id,
                workspace_id: Some(app.public_workspace_id(0)),
                tab_id: app.public_tab_id(0, 0),
                pane_id: Some(pane_id.clone()),
                selection: None,
            },
        );
        let success: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.result, crate::api::schema::ResponseResult::Ok {});
        let event = next_detached_event(&mut app);
        app.handle_internal_event(event);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), pane_id);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn stderr_tail_folds_whitespace_and_keeps_the_end() {
        assert_eq!(super::stderr_tail_text(b"  a\n\tb  \n"), "a b");
        let long = "x".repeat(super::DETACHED_COMMAND_STDERR_TAIL_CHARS + 10) + "END";
        let tail = super::stderr_tail_text(long.as_bytes());
        assert_eq!(
            tail.chars().count(),
            super::DETACHED_COMMAND_STDERR_TAIL_CHARS
        );
        assert!(tail.ends_with("END"));
    }

    #[test]
    fn popup_commands_are_advertised_without_exposing_the_command_text() {
        let mut app = test_app();
        install(&mut app, binding(crate::config::CustomCommandAction::Popup));

        let manifest = app.client_shell_command_manifest();
        assert_eq!(manifest.len(), 1);
        assert_eq!(
            manifest[0].action,
            crate::protocol::ClientShellCommandAction::Popup
        );
        assert!(!format!("{manifest:?}").contains("secret-command"));
    }
}

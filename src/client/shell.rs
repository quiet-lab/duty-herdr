use std::collections::{HashMap, HashSet, VecDeque};

mod actions;
mod agent_sidebar;
mod aggregate_navigation;
mod machine_diagnostics;
mod workspace_navigation;
use workspace_navigation::{PendingWorkspaceHighlight, WorkspaceNavigationTarget};
mod composition;
mod config;
mod context_menu;
mod copy_mode;
mod endpoint_agent_state;
mod endpoint_agents;
mod endpoint_navigation;
mod endpoint_notices;
mod endpoint_sidebar;
mod endpoints;
mod follow_blocked;
pub(super) use endpoints::*;
mod global_menu;
mod graphics;
mod input;
mod input_source;
mod labels;
mod link_hover;
mod mobile;
mod mouse;
mod notification_policy;
mod notifications;
mod overlay_input;
mod preferences;
mod render;
mod scroll;
mod settings;
mod state;
mod status_frames;
mod surface_patch;
mod text_editor;
mod word_selection;
mod worktrees;
use text_editor::TextEditor;
use word_selection::ClientWordSelection;

pub(in crate::client::shell) use render::sidebar;
pub(crate) use state::*;
#[cfg(test)]
pub(super) use surface_patch::apply_composed_surface_patch;
pub(super) use surface_patch::{ClientComposedSurfacePatch, ClientPaneSurfacePatchOutcome};

use crossterm::event::KeyCode;
#[cfg(test)]
use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::endpoint::{ClientEndpointId, ClientEndpointStatus, SavedSshEndpoint};
use crate::app::state::Palette;
use crate::config::{
    Config, LiveKeybindConfig, SidebarCollapsedModeConfig, SpacesSidebarConfig,
    TabBarPositionConfig,
};
use crate::protocol::{
    ClientMessage, ClientMousePosition, ClientPaneInputEvent, ClientShellSnapshot, ClientShellTab,
    ClientShellWorkspace, ClientSurfaceSize, FrameData, PaneSurfaceFrame, SemanticNotification,
    SemanticNotificationKind, SemanticNotificationSound,
};
#[cfg(test)]
use crate::raw_input::RawInputEvent;

fn target_event_message(target: ClientInputTarget, event: ClientPaneInputEvent) -> ClientMessage {
    match target {
        ClientInputTarget::Pane(pane_id) => ClientMessage::ClientShellPaneInput {
            pane_id,
            events: vec![event],
        },
        ClientInputTarget::Popup(terminal_id) => ClientMessage::ClientShellPopupInput {
            terminal_id,
            events: vec![event],
        },
    }
}

fn push_target_event(
    target: ClientInputTarget,
    event: ClientPaneInputEvent,
    outcome: &mut ClientShellInput,
) {
    match target {
        ClientInputTarget::Pane(pane_id) => {
            if let Some(ClientMessage::ClientShellPaneInput {
                pane_id: pending_pane,
                events,
            }) = outcome.requests.last_mut()
            {
                if *pending_pane == pane_id {
                    events.push(event);
                    return;
                }
            }
            outcome.requests.push(target_event_message(
                ClientInputTarget::Pane(pane_id),
                event,
            ));
        }
        ClientInputTarget::Popup(terminal_id) => {
            if let Some(ClientMessage::ClientShellPopupInput {
                terminal_id: pending_terminal,
                events,
            }) = outcome.requests.last_mut()
            {
                if *pending_terminal == terminal_id {
                    events.push(event);
                    return;
                }
            }
            outcome.requests.push(target_event_message(
                ClientInputTarget::Popup(terminal_id),
                event,
            ));
        }
    }
}

fn contains(rect: Rect, point: (u16, u16)) -> bool {
    rect.width > 0
        && rect.height > 0
        && point.0 >= rect.x
        && point.0 < rect.right()
        && point.1 >= rect.y
        && point.1 < rect.bottom()
}

fn pane_surface_topology_signature(surface: &PaneSurfaceFrame) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn write(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(PRIME);
        }
        *hash ^= 0xff;
        *hash = hash.wrapping_mul(PRIME);
    }

    let mut pane_ids = surface
        .panes
        .iter()
        .map(|pane| pane.pane_id.as_bytes())
        .collect::<Vec<_>>();
    pane_ids.sort_unstable();
    let mut hash = OFFSET;
    for pane_id in pane_ids {
        write(&mut hash, pane_id);
    }
    let mut splits = surface.splits.iter().collect::<Vec<_>>();
    splits.sort_by(|left, right| left.path.cmp(&right.path));
    for split in splits {
        write(
            &mut hash,
            &[match split.direction {
                crate::protocol::PaneSurfaceSplitDirection::Horizontal => 0,
                crate::protocol::PaneSurfaceSplitDirection::Vertical => 1,
            }],
        );
        write(
            &mut hash,
            &split
                .path
                .iter()
                .map(|right| u8::from(*right))
                .collect::<Vec<_>>(),
        );
    }
    hash
}

/// Returns the current frame of the `[ui.status_icons]` override for `status`,
/// or the built-in symbol of `style` when no override is set. `phase` is the
/// row's seed from `status_frames::row_phase`, `None` for an aggregate without a row.
fn status_icon(
    status: crate::api::schema::AgentStatus,
    style: crate::config::StatusIndicatorStyle,
    icons: &status_frames::ShellStatusIcons,
    phase: Option<u64>,
) -> &str {
    icons
        .override_for(status, phase)
        .unwrap_or_else(|| builtin_status_icon(status, style))
}

fn builtin_status_icon(
    status: crate::api::schema::AgentStatus,
    style: crate::config::StatusIndicatorStyle,
) -> &'static str {
    use crate::api::schema::AgentStatus;
    use crate::config::StatusIndicatorStyle;
    match (style, status) {
        (
            StatusIndicatorStyle::Dots,
            AgentStatus::Working | AgentStatus::Blocked | AgentStatus::Done,
        ) => "●",
        (StatusIndicatorStyle::Dots, AgentStatus::Idle) => "○",
        (StatusIndicatorStyle::Dots, AgentStatus::Unknown) => "·",
        (StatusIndicatorStyle::Symbols, AgentStatus::Blocked) => "×",
        (StatusIndicatorStyle::Symbols, AgentStatus::Working) => "◐",
        (StatusIndicatorStyle::Symbols, AgentStatus::Done) => "✓",
        (StatusIndicatorStyle::Symbols, AgentStatus::Idle) => "○",
        (StatusIndicatorStyle::Symbols, AgentStatus::Unknown) => "·",
    }
}

fn status_dot(
    status: crate::api::schema::AgentStatus,
    icons: &status_frames::ShellStatusIcons,
) -> &str {
    status_icon(
        status,
        crate::config::StatusIndicatorStyle::Dots,
        icons,
        None,
    )
}

fn status_priority(status: crate::api::schema::AgentStatus) -> u8 {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => 4,
        AgentStatus::Done => 3,
        AgentStatus::Working => 2,
        AgentStatus::Idle => 1,
        AgentStatus::Unknown => 0,
    }
}

fn status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Idle => "idle",
        AgentStatus::Unknown => "unknown",
    }
}

fn status_color(
    status: crate::api::schema::AgentStatus,
    palette: &Palette,
) -> ratatui::style::Color {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Working => palette.yellow,
        AgentStatus::Blocked => palette.red,
        AgentStatus::Done => palette.teal,
        AgentStatus::Idle => palette.green,
        AgentStatus::Unknown => palette.overlay0,
    }
}

fn panel_contrast_fg(palette: &Palette) -> ratatui::style::Color {
    match palette.panel_bg {
        ratatui::style::Color::Reset => palette.surface_dim,
        color => color,
    }
}

/// Converts the shell chrome `buffer` into a frame with the pane surface `source`
/// placed at `area`.
///
/// The result equals `FrameData::from_ratatui_buffer_with_hyperlinks(buffer, None, &[])`
/// followed by `blit_pane_surface(&mut frame, source, area)`, but chrome cells that
/// the pane surface covers are never converted, allocated and then dropped: every
/// output cell is produced once. Geometry the single pass does not cover (a buffer
/// not anchored at the origin, a pane surface reaching past the frame) takes the
/// two-step path.
///
/// Output cells are taken from `recycled` while it lasts and overwritten in place,
/// so their symbols reuse earlier allocations.
fn compose_pane_surface_frame(
    buffer: &Buffer,
    source: &FrameData,
    area: Rect,
    recycled: &mut Vec<crate::protocol::CellData>,
) -> FrameData {
    let width = usize::from(buffer.area.width);
    let height = usize::from(buffer.area.height);
    let copy_width = usize::from(source.width.min(area.width));
    let copy_height = usize::from(source.height.min(area.height));
    let left = usize::from(area.x);
    let top = usize::from(area.y);
    if buffer.area.x != 0
        || buffer.area.y != 0
        || buffer.content.len() != width * height
        || left + copy_width > width
        || top + copy_height > height
    {
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(buffer, None, &[]);
        blit_pane_surface(&mut frame, source, area);
        return frame;
    }
    let source_width = usize::from(source.width);
    let source_hyperlinks = source.hyperlinks.len();
    let mut cells = Vec::with_capacity(width * height);
    let pane_cell = |recycled: &mut Vec<crate::protocol::CellData>,
                     pane: &crate::protocol::CellData| {
        let mut cell = match recycled.pop() {
            Some(mut cell) => {
                cell.clone_from(pane);
                cell
            }
            None => pane.clone(),
        };
        cell.hyperlink = shifted_hyperlink(cell.hyperlink, source_hyperlinks, 0);
        cell
    };
    for (y, row) in buffer.content.chunks_exact(width.max(1)).enumerate() {
        if !(top..top + copy_height).contains(&y) {
            cells.extend(row.iter().map(|cell| chrome_cell(recycled, cell)));
            continue;
        }
        let source_start = (y - top) * source_width;
        cells.extend(row[..left].iter().map(|cell| chrome_cell(recycled, cell)));
        match source.cells.get(source_start..source_start + copy_width) {
            Some(source_row) => {
                cells.extend(source_row.iter().map(|pane| pane_cell(recycled, pane)));
            }
            None => {
                for (col, chrome) in row[left..left + copy_width].iter().enumerate() {
                    let cell = match source.cells.get(source_start + col) {
                        Some(pane) => pane_cell(recycled, pane),
                        None => chrome_cell(recycled, chrome),
                    };
                    cells.push(cell);
                }
            }
        }
        cells.extend(
            row[left + copy_width..]
                .iter()
                .map(|cell| chrome_cell(recycled, cell)),
        );
    }
    if recycled.is_empty() {
        // Release the emptied pool's allocation instead of keeping it until the next
        // surface replacement.
        *recycled = Vec::new();
    }
    FrameData {
        cells,
        width: buffer.area.width,
        height: buffer.area.height,
        cursor: projected_surface_cursor(source, area),
        hyperlinks: source.hyperlinks.clone(),
        graphics: Vec::new(),
    }
}

/// Hyperlink index of a pane surface cell in a frame whose hyperlink table holds
/// the surface's `source_len` entries from `base` on; an index past the surface's
/// table is dropped.
fn shifted_hyperlink(index: Option<u32>, source_len: usize, base: u32) -> Option<u32> {
    index.and_then(|index| ((index as usize) < source_len).then_some(base + index))
}

/// Cursor of the pane surface `source` placed at `area`, kept only inside the copied
/// part of the surface.
fn projected_surface_cursor(
    source: &FrameData,
    area: Rect,
) -> Option<crate::protocol::CursorState> {
    let copy_width = source.width.min(area.width);
    let copy_height = source.height.min(area.height);
    source.cursor.as_ref().and_then(|cursor| {
        (cursor.x < copy_width && cursor.y < copy_height).then(|| crate::protocol::CursorState {
            x: area.x + cursor.x,
            y: area.y + cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        })
    })
}

/// Converts a chrome cell, reusing a recycled cell when one is left.
fn chrome_cell(
    recycled: &mut Vec<crate::protocol::CellData>,
    cell: &ratatui::buffer::Cell,
) -> crate::protocol::CellData {
    match recycled.pop() {
        Some(mut recycled) => {
            recycled.assign_from_ratatui_cell(cell);
            recycled
        }
        None => crate::protocol::CellData::from_ratatui_cell(cell),
    }
}

fn blit_pane_surface(target: &mut FrameData, source: &FrameData, area: Rect) {
    let copy_width = source.width.min(area.width);
    let copy_height = source.height.min(area.height);
    let hyperlink_base = target.hyperlinks.len() as u32;
    target.hyperlinks.extend(source.hyperlinks.iter().cloned());

    for row in 0..copy_height {
        for col in 0..copy_width {
            let source_index = row as usize * source.width as usize + col as usize;
            let target_x = area.x + col;
            let target_y = area.y + row;
            let target_index = target_y as usize * target.width as usize + target_x as usize;
            let (Some(source_cell), Some(target_cell)) = (
                source.cells.get(source_index),
                target.cells.get_mut(target_index),
            ) else {
                continue;
            };
            target_cell.clone_from(source_cell);
            target_cell.hyperlink = shifted_hyperlink(
                source_cell.hyperlink,
                source.hyperlinks.len(),
                hyperlink_base,
            );
        }
    }

    target.cursor = projected_surface_cursor(source, area);
    target.graphics.clear();
}

#[cfg(test)]
mod tests;

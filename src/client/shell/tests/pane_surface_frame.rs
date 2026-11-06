use super::*;
use crate::protocol::{CellData, CursorState};
use ratatui::style::Color;

/// Chrome conversion followed by the pane surface blit, written out cell by cell
/// as composition did before the single pass.
fn convert_then_blit(buffer: &Buffer, source: &FrameData, area: Rect) -> FrameData {
    let mut cells = Vec::new();
    for row in 0..buffer.area.height {
        for col in 0..buffer.area.width {
            cells.push(CellData::from_ratatui_cell(&buffer[(col, row)]));
        }
    }
    let mut frame = FrameData {
        cells,
        width: buffer.area.width,
        height: buffer.area.height,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let copy_width = source.width.min(area.width);
    let copy_height = source.height.min(area.height);
    frame.hyperlinks.extend(source.hyperlinks.iter().cloned());
    for row in 0..copy_height {
        for col in 0..copy_width {
            let source_index = usize::from(row) * usize::from(source.width) + usize::from(col);
            let target_index =
                usize::from(area.y + row) * usize::from(frame.width) + usize::from(area.x + col);
            let (Some(source_cell), Some(target_cell)) = (
                source.cells.get(source_index),
                frame.cells.get_mut(target_index),
            ) else {
                continue;
            };
            *target_cell = source_cell.clone();
            target_cell.hyperlink = source_cell
                .hyperlink
                .and_then(|index| ((index as usize) < source.hyperlinks.len()).then_some(index));
        }
    }
    frame.cursor = source.cursor.as_ref().and_then(|cursor| {
        (cursor.x < copy_width && cursor.y < copy_height).then(|| CursorState {
            x: area.x + cursor.x,
            y: area.y + cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        })
    });
    frame
}

fn chrome() -> Buffer {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 6));
    buffer.set_style(buffer.area, Style::default().bg(Color::Indexed(236)));
    buffer.set_string(0, 0, "space-1 界", Style::default().fg(Color::Rgb(1, 2, 3)));
    buffer.set_string(
        0,
        5,
        "mode bar ok",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    );
    buffer[(11, 3)].skip = true;
    buffer
}

fn pane_source(width: u16, height: u16) -> FrameData {
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
    for row in 0..height {
        buffer.set_string(
            0,
            row,
            format!("r{row}é{}", "x".repeat(usize::from(width))),
            Style::default().fg(Color::Indexed(row as u8)),
        );
    }
    let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &buffer,
        Some(CursorState {
            x: 1,
            y: height.saturating_sub(1),
            visible: true,
            shape: 5,
        }),
        &[],
    );
    frame.hyperlinks = vec!["https://a.example".into(), "https://b.example".into()];
    frame.cells[0].hyperlink = Some(1);
    if let Some(cell) = frame.cells.get_mut(2) {
        // An index past the frame's hyperlink table is dropped by the blit.
        cell.hyperlink = Some(7);
    }
    frame
}

fn recycled_pools() -> Vec<Vec<CellData>> {
    let junk = CellData {
        symbol: "stale-symbol".into(),
        fg: 0x02_ff_00_00,
        bg: 0x01_00_00_07,
        modifier: 0xffff,
        skip: true,
        hyperlink: Some(3),
    };
    vec![
        Vec::new(),
        vec![junk.clone(); 10],
        vec![junk; 200],
        vec![
            CellData {
                symbol: String::new(),
                fg: 0,
                bg: 0,
                modifier: 0,
                skip: false,
                hyperlink: None,
            };
            72
        ],
    ]
}

#[test]
fn single_pass_pane_surface_frame_matches_convert_then_blit() {
    let buffer = chrome();
    let mut truncated = pane_source(6, 3);
    truncated.cells.truncate(10);
    let cases = [
        // Pane surface narrower than its area, cursor inside.
        (pane_source(5, 3), Rect::new(4, 1, 7, 4)),
        // Pane surface larger than its area: clipped, cursor outside the copy.
        (pane_source(9, 6), Rect::new(3, 1, 6, 3)),
        // Too few cells: the rest of the area keeps the chrome.
        (truncated, Rect::new(2, 2, 6, 3)),
        // Area at the frame edge.
        (pane_source(4, 2), Rect::new(8, 4, 4, 2)),
        // Area past the frame edge takes the two-step path.
        (pane_source(6, 2), Rect::new(9, 0, 6, 2)),
        // Empty area.
        (pane_source(3, 3), Rect::new(0, 0, 0, 0)),
    ];
    for (source, area) in cases {
        let expected = convert_then_blit(&buffer, &source, area);
        for mut recycled in recycled_pools() {
            let frame = compose_pane_surface_frame(&buffer, &source, area, &mut recycled);
            assert_eq!(frame, expected, "area {area:?}");
        }
    }
}

#[test]
fn composition_after_surface_replacement_matches_fresh_state() {
    let now = std::time::Instant::now();
    let mut replaced_surface = surface();
    replaced_surface.frame = pane_source(30, 12);
    replaced_surface.panes[0].rect = SurfaceRect {
        x: 0,
        y: 0,
        width: 30,
        height: 12,
    };
    replaced_surface.panes[0].inner_rect = replaced_surface.panes[0].rect;
    let mut next_surface = surface();
    next_surface.surface_revision = 2;

    let mut recycled = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    recycled.set_snapshot(Box::new(snapshot()));
    recycled.set_pane_surface(replaced_surface);
    recycled.compose_at(80, 16, now).expect("first frame");
    recycled.set_pane_surface(next_surface.clone());
    assert!(!recycled.recycled_cells.is_empty());
    let frame = recycled.compose_at(80, 16, now).expect("recycled frame");

    let mut fresh = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    fresh.set_snapshot(Box::new(snapshot()));
    fresh.set_pane_surface(next_surface);
    fresh.compose_at(80, 16, now).expect("first fresh frame");
    let expected = fresh.compose_at(80, 16, now).expect("fresh frame");

    assert_eq!(frame.frame, expected.frame);
}

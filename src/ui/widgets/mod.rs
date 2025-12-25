use ratatui::prelude::Alignment;

pub mod async_image;
pub mod browser;
pub mod button;
pub mod header;
pub mod input;
pub mod progress_bar;
pub mod scan_status;
pub mod scrolling_line;
pub mod tabs;
pub mod volume;
pub(crate) mod element;
pub mod item_list;
pub mod queue_panel;
pub mod queue_view;
pub mod interactive_list_view;
pub mod list_view_state;
pub mod filter_state;
pub mod nav_stack;
pub mod detail_stack;

fn get_line_offset(line_width: u16, text_area_width: u16, alignment: Alignment) -> u16 {
    match alignment {
        Alignment::Center => (text_area_width / 2).saturating_sub(line_width / 2),
        Alignment::Right => text_area_width.saturating_sub(line_width),
        Alignment::Left => 0,
    }
}

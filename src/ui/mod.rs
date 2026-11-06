pub mod components;
pub mod kitty;
pub mod theme;
pub mod utils;

use crate::app::{ActivePane, App};
use crate::mode::AppMode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    Frame,
};

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    let theme = &app.theme;

    // Vertical split: Main content area + Bottom Status Bar
    let [content_area, status_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).areas(size);

    // Responsive Column Check (Tailored for niri scrollable tiling)
    let is_compact = size.width < app.config.ui.single_column_breakpoint;

    let (chats_area, messages_area) = if is_compact {
        // Single Column Focused Mode
        match app.active_pane {
            ActivePane::Chats => (Some(content_area), None),
            ActivePane::Messages => (None, Some(content_area)),
        }
    } else {
        // Dual Column Standard Mode
        let ratio = app.config.ui.chats_ratio_percent;
        let [left, right] = Layout::horizontal([
            Constraint::Percentage(ratio),
            Constraint::Percentage(100 - ratio),
        ])
        .areas(content_area);
        (Some(left), Some(right))
    };

    if let Some(area) = chats_area {
        components::chat_list::render(
            f,
            area,
            &app.chats,
            app.selected_chat_idx,
            is_compact || app.active_pane == ActivePane::Chats,
            &app.chat_scroll,
            theme,
        );
    }

    if let Some(area) = messages_area {
        render_messages_pane(f, area, app, is_compact || app.active_pane == ActivePane::Messages);
    }

    let active_title = app.active_chat().map(|c| c.title.as_str());
    // Like other clients, muted chats don't count towards the total.
    let unread_total = app.chats.iter().filter(|c| !c.is_muted).map(|c| c.unread_count).sum();
    components::status_bar::render(
        f,
        status_area,
        app.mode,
        active_title,
        unread_total,
        &app.connection,
        app.status_msg.as_deref(),
        theme,
    );

    // Overlays / Popups
    match app.mode {
        AppMode::FuzzyFind => {
            let matches: Vec<_> = app
                .filtered_chat_indices()
                .into_iter()
                .map(|i| &app.chats[i])
                .collect();
            components::fuzzy_finder::render(
                f,
                size,
                &app.search_query,
                &matches,
                app.fuzzy_selected_idx,
                &app.fuzzy_scroll,
                theme,
            );
        }
        AppMode::Help => components::help_popup::render(f, size, theme),
        _ => {}
    }
}

fn render_messages_pane(f: &mut Frame, area: Rect, app: &App, focused: bool) {
    let [messages_area, input_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(components::input_box::height(app.replying_to.is_some(), app.attachments.len())),
    ])
    .areas(area);

    components::message_view::render(
        f,
        messages_area,
        app.active_chat(),
        &app.messages,
        app.selected_msg_idx,
        focused,
        &app.msg_scroll,
        &app.images,
        &app.theme,
    );

    components::input_box::render(
        f,
        input_area,
        &app.input_buffer,
        app.replying_to.as_deref(),
        &app.attachments,
        app.mode,
        &app.theme,
    );
}

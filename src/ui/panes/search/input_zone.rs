//! SearchInputZone - Adapter wrapping InputGroups for InputContentView.
//!
//! This adapter implements the `InputZone` trait, enabling InputGroups to work
//! with the unified `InputContentView` component from the new architecture.

use ratatui::{Frame, layout::Rect};

use crate::{
    config::keys::CommonAction,
    ctx::Ctx,
    shared::key_event::KeyEvent,
    ui::panes::navigator_types::InputMode,
    ui::widgets::{
        edit_command::{EditCommand, apply_edit_command, resolve_edit_command},
        input_content_view::{InputZone, InputZoneAction},
    },
};

use super::inputs::{ActionResult, InputGroups, InputType, TextboxInput};

/// Adapter wrapping InputGroups to implement InputZone trait.
///
/// This allows SearchPane to use InputContentView while preserving
/// all existing InputGroups functionality.
#[derive(Debug)]
pub(crate) struct SearchInputZone {
    /// The wrapped InputGroups component
    pub(crate) inputs: InputGroups,
}

impl SearchInputZone {
    /// Create a new SearchInputZone wrapping InputGroups.
    pub(crate) fn new(inputs: InputGroups) -> Self {
        Self { inputs }
    }

    /// Get search mode from the underlying inputs.
    pub(crate) fn search_mode(&self) -> super::inputs::SearchMode {
        self.inputs.search_mode()
    }

    /// Get fold case setting.
    pub(crate) fn fold_case(&self) -> bool {
        self.inputs.fold_case()
    }

    /// Reset all input fields.
    pub(crate) fn reset_all(&mut self) {
        self.inputs.reset_all();
    }

    /// Check if we're in insert mode (typing in text field).
    pub(crate) fn is_insert_mode(&self) -> bool {
        self.inputs.insert_mode
    }

    /// Get the underlying inputs for building filters.
    pub(crate) fn underlying(&self) -> &InputGroups {
        &self.inputs
    }

    /// Get mutable reference to underlying inputs.
    pub(crate) fn underlying_mut(&mut self) -> &mut InputGroups {
        &mut self.inputs
    }
}

impl InputZone for SearchInputZone {
    fn render(&mut self, frame: &mut Frame, area: Rect, _ctx: &Ctx, _is_focused: bool) {
        // InputGroups implements Widget trait
        frame.render_widget(&mut self.inputs, area);
    }

    fn handle_key(&mut self, key: &mut KeyEvent, ctx: &Ctx) -> InputZoneAction {
        let config = &ctx.config;

        // Handle insert mode (typing in text fields)
        if self.inputs.insert_mode {
            key.stop_propagation();
            match resolve_edit_command(key) {
                EditCommand::Cancel => {
                    self.inputs.insert_mode = false;
                    if let InputType::Numberbox(TextboxInput { value, cursor, .. }) =
                        self.inputs.focused_mut()
                    {
                        if value.is_empty() {
                            value.push('0');
                            *cursor = value.chars().count();
                        }
                    }
                    return InputZoneAction::Handled;
                }
                EditCommand::Accept => {
                    self.inputs.insert_mode = false;
                    if let InputType::Numberbox(TextboxInput { value, cursor, .. }) =
                        self.inputs.focused_mut()
                    {
                        if value.is_empty() {
                            value.push('0');
                            *cursor = value.chars().count();
                        }
                    }
                    // Return Submit to trigger search
                    return InputZoneAction::Submit(self.build_query_string());
                }
                command => {
                    match self.inputs.focused_mut() {
                        InputType::Textbox(TextboxInput { value, cursor, .. }) => {
                            apply_edit_command(command, value, cursor, false);
                        }
                        InputType::Numberbox(TextboxInput { value, cursor, .. }) => {
                            apply_edit_command(command, value, cursor, true);
                        }
                        _ => {}
                    }

                    return InputZoneAction::Handled;
                }
            }
        }

        // Normal mode handling
        if let Some(action) = key.as_common_action(ctx) {
            match action {
                CommonAction::Down => {
                    if config.wrap_navigation {
                        self.inputs.next();
                    } else {
                        self.inputs.next_non_wrapping();
                    }
                    key.stop_propagation();
                    return InputZoneAction::Handled;
                }
                CommonAction::Up => {
                    if config.wrap_navigation {
                        self.inputs.prev();
                    } else {
                        self.inputs.prev_non_wrapping();
                    }
                    key.stop_propagation();
                    return InputZoneAction::Handled;
                }
                CommonAction::Right => {
                    // Signal to focus content zone
                    key.stop_propagation();
                    return InputZoneAction::FocusContent;
                }
                CommonAction::Confirm => {
                    let result = self.inputs.activate_focused();
                    key.stop_propagation();
                    match result {
                        ActionResult::Search => {
                            return InputZoneAction::Submit(self.build_query_string());
                        }
                        ActionResult::Reset => {
                            return InputZoneAction::Handled;
                        }
                        ActionResult::None => {
                            return InputZoneAction::Handled;
                        }
                    }
                }
                CommonAction::FocusInput => {
                    self.inputs.enter_insert_mode();
                    key.stop_propagation();
                    return InputZoneAction::Handled;
                }
                CommonAction::Top => {
                    self.inputs.first();
                    key.stop_propagation();
                    return InputZoneAction::Handled;
                }
                CommonAction::Bottom => {
                    self.inputs.last();
                    key.stop_propagation();
                    return InputZoneAction::Handled;
                }
                _ => {}
            }
        }

        InputZoneAction::Passthrough
    }

    fn mode(&self) -> InputMode {
        if self.inputs.insert_mode { InputMode::Edit } else { InputMode::Normal }
    }

    fn focus_first(&mut self) {
        self.inputs.first();
    }

    fn focus_last(&mut self) {
        self.inputs.last();
    }

    fn is_at_last(&self) -> bool {
        // Check if focused on last focusable input
        let len = self.inputs.inputs.len();
        if len == 0 {
            return true;
        }

        // Find last non-separator index
        for i in (0..len).rev() {
            if !matches!(&self.inputs.inputs[i], InputType::Separator) {
                return self.inputs.focused_idx() == i;
            }
        }
        true
    }

    fn is_at_first(&self) -> bool {
        // Check if focused on first focusable input
        let len = self.inputs.inputs.len();
        if len == 0 {
            return true;
        }

        // Find first non-separator index
        for i in 0..len {
            if !matches!(&self.inputs.inputs[i], InputType::Separator) {
                return self.inputs.focused_idx() == i;
            }
        }
        true
    }
}

impl SearchInputZone {
    /// Build a simple query string from the inputs (for Submit action).
    fn build_query_string(&self) -> String {
        // Just return an empty string - the actual search logic uses the filter system
        // The pane will call search() with full filter construction
        String::new()
    }
}

// =============================================================================
// Extension trait for InputGroups to expose focused_idx
// =============================================================================

trait InputGroupsExt {
    fn focused_idx(&self) -> usize;
}

impl InputGroupsExt for InputGroups {
    fn focused_idx(&self) -> usize {
        // We need to access the private focused_idx field
        // Since InputGroups doesn't expose this, we'll use a workaround
        // by checking which input matches the focused one
        for (i, input) in self.inputs.iter().enumerate() {
            if std::ptr::eq(input, self.focused()) {
                return i;
            }
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests would require mocking Ctx and InputGroups setup
    // which is complex. Defer to integration tests.
}

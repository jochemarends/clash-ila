use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::{layout::Rect, style::Style, widgets::WidgetRef};

#[derive(Debug, Clone)]
/// A checkbox list widget
///
/// It renders a list of options the user can choose from. Navigation is done via UP/DOWN and
/// selection of the items via SPACE.
///
/// The items are always rendered vertically.
pub struct Checkbox {
    /// Vector of items and their marked state
    items: Vec<(String, bool)>,
    /// Indicates the focussed state of the widget, when set to false it will not process any input
    focussed: bool,
    /// The index the cursor is currently hovering at
    currently_hovering: usize,
}

#[allow(unused)]
impl Checkbox {
    /// Creates a `SelectableListState` from a list of possible options to select from
    pub fn new<T: Into<String>>(items: Vec<T>) -> Checkbox {
        Checkbox {
            items: items.into_iter().map(|name| (name.into(), false)).collect(),
            focussed: false,
            currently_hovering: 0,
        }
    }

    /// Marks the element as active, when inactive it does not listen to input
    pub fn set_focus(&mut self, focus: bool) {
        self.focussed = focus;
    }

    /// Sets which item in the list it is currently hovering on, out of range values will be
    /// clamped
    pub fn set_hover(&mut self, hover: usize) {
        self.currently_hovering = hover.clamp(0, self.items.len() - 1);
    }

    /// Checks if the cursor is at the boundary of the possible selected items
    pub fn is_cursor_at_bounds(&self) -> bool {
        self.currently_hovering == 0 || self.currently_hovering == self.items.len() - 1
    }

    /// Hover the current selection up by one
    ///
    /// Sets active to false if the selection goes out of range
    ///
    /// Returns if the input has been 'consumed' by the function
    pub fn hover_up(&mut self) -> bool {
        if self.currently_hovering == 0 {
            self.focussed = false;
            false
        } else {
            self.currently_hovering -= 1;
            true
        }
    }

    /// Hover the current selection down by one
    ///
    /// Sets active to false if the selection goes out of range
    ///
    /// Returns if the input has been 'consumed' by the function
    pub fn hover_down(&mut self) -> bool {
        if self.currently_hovering == self.items.len() - 1 {
            self.focussed = false;
            false
        } else {
            self.currently_hovering += 1;
            true
        }
    }

    /// Set the marked status of any individual item
    pub fn set_marked(&mut self, index: usize, selected: bool) -> Option<()> {
        let addr = self.items.get_mut(index)?;
        addr.1 = selected;
        Some(())
    }

    /// Check if any individual item is marked or not
    pub fn is_marked(&self, index: usize) -> bool {
        self.items
            .get(index)
            .map(|bundle| bundle.1)
            .unwrap_or(false)
    }

    /// Toggle the marked status of any individual item
    pub fn toggle_marked(&mut self, index: usize) -> Option<()> {
        let addr = self.items.get_mut(index)?;
        addr.1 = !addr.1;
        Some(())
    }

    /// Retrieve all the marked checkbox items
    pub fn get_all_marked(&self) -> Vec<bool> {
        self.items.iter().map(|(_, marked)| *marked).collect()
    }

    /// Manages input given to this widget
    ///
    /// Returns if the input has been 'consumed' by the function
    pub fn handle_input(&mut self, event: &Event) -> bool {
        if !self.focussed {
            return false;
        }

        let key_event = match event {
            Event::Key(key) => key,
            _ => return false,
        };

        match key_event {
            KeyEvent {
                code: KeyCode::Up, ..
            } => self.hover_up(),
            KeyEvent {
                code: KeyCode::Down,
                ..
            } => self.hover_down(),
            KeyEvent {
                code: KeyCode::Char(' '),
                ..
            } => {
                self.toggle_marked(self.currently_hovering);
                true
            }
            _ => false,
        }
    }
}

impl WidgetRef for Checkbox {
    fn render_ref(&self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        for (index, (line, selected)) in self.items.iter().enumerate() {
            let selected = if *selected { '*' } else { ' ' };
            let add_cursor = if self.focussed && index == self.currently_hovering {
                "> "
            } else {
                ""
            };
            let output = format!("{add_cursor}[{selected}] - {line}");

            buf.set_string(area.x, area.y + index as u16, output, Style::default());
        }
    }
}

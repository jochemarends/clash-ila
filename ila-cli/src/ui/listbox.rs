use crossterm::event::{Event, KeyCode, KeyEvent};
use ratatui::{layout::Rect, style::Style, widgets::WidgetRef};

#[derive(Debug, Clone)]
/// A listbox widget
///
/// It renders a list of options the user can choose from. Nativation is done via UP/DOWN and
/// selection of the items via SPACE.
///
/// The main different compared to a `Checkbox` is that this only allows for *one* selected item,
/// where a checkbox may allow the user to pick several, or all items.
///
/// The items are always rendered vertically.
pub struct Listbox {
    /// Vector of elements to select from
    items: Vec<String>,
    /// Index of the element currently selected
    selected: usize,
    /// Indicates the focus status of the element, when it's not focussed, no input will be
    /// processed
    focussed: bool,
    /// The index of the element the cursor is hovering on
    currently_hovering: usize,
}

#[allow(unused)]
impl Listbox {
    /// Creates a `SelectableListState` from a list of possible options to select from
    pub fn new<T: Into<String>>(items: Vec<T>, selected: usize) -> Listbox {
        let selected = selected.clamp(0, items.len() - 1);
        Listbox {
            items: items.into_iter().map(|name| name.into()).collect(),
            selected,
            focussed: false,
            currently_hovering: 0,
        }
    }

    /// Sets which item in the list it is currently hovering on, out of range values will be
    /// clamped
    pub fn set_hover(&mut self, hover: usize) {
        self.currently_hovering = hover.clamp(0, self.items.len() - 1);
    }

    /// Sets the focus status of the element
    pub fn set_focus(&mut self, focus: bool) {
        self.focussed = focus;
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

    /// Select an item of the listbox
    pub fn select(&mut self, selected: usize) {
        self.selected = selected.clamp(0, self.items.len() - 1);
    }

    /// Get the selected item
    pub fn get_selected(&mut self) -> usize {
        self.selected
    }

    // The items of the listbox
    pub fn items(&self) -> &[String] {
        &self.items
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
                self.select(self.currently_hovering);
                true
            }
            _ => false,
        }
    }
}

impl WidgetRef for Listbox {
    fn render_ref(&self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        for (index, line) in self.items.iter().enumerate() {
            let selected = if index == self.selected { '*' } else { ' ' };
            let add_cursor = if self.focussed && index == self.currently_hovering { "> " } else { "" };
            let output = format!("{add_cursor}({selected}) - {line}");

            buf.set_string(area.x, area.y + index as u16, output, Style::default());
        }
    }
}

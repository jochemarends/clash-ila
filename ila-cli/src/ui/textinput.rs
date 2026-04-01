use std::marker::PhantomData;

use crossterm::event::KeyCode;
use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::*, Frame};

/// The state for the TextPrompt widget
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TextPromptState<T> {
    /// The input field, gets appended to as the user types
    pub input: String,
    /// The location of the cursor within the buffer, note that this is NOT the location on screen!
    pub cursor: usize,
    /// The calculated offset the cursor has in relation to the on screen text
    pub cursor_offset: usize,
    /// A number indicating how many characters should be skipped before displaying visible text on
    /// screen
    pub visible: usize,
    /// The minimum and maximum bounds that text can be displayed on. These simply mean available
    /// space, not actual occupied space by text
    pub render_bounds: (u16, u16),
    /// The reason for this prompt to be prompted
    pub reason: T,
}

impl<R> TextPromptState<R>
where
    R: Clone,
{
    /// Create a new TextPromptState
    pub fn new<S>(default: Option<S>, reason: R) -> TextPromptState<R>
    where
        S: Into<String>,
    {
        let def = default.map(|s| s.into()).unwrap_or_default();
        let len = def.len();
        TextPromptState {
            input: def,
            cursor: len,
            cursor_offset: 0,
            visible: 0,
            render_bounds: (u16::MIN, u16::MAX),
            reason,
        }
    }

    /// Calculate from what point the text should be visible
    pub fn calculate_visible(&mut self, area: Rect) {
        self.visible = self
            .input
            .len()
            .saturating_sub(area.width as usize)
            .saturating_sub(self.cursor_offset);
    }

    /// Move the cursor one step to the left
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        if self.cursor != 0 && self.get_cursor_x() == self.render_bounds.0 {
            self.cursor_offset = (self.cursor_offset + 1).min(self.input.len())
        }
    }

    /// Move the cursor one step to the right
    pub fn right(&mut self) {
        self.cursor = self.input.len().min(self.cursor + 1);
        if self.cursor != self.input.len() && self.get_cursor_x() == self.render_bounds.1 {
            self.cursor_offset = self.cursor_offset.saturating_sub(1);
        }
    }

    /// Calculate where the cursor should be placed in the TUI
    pub fn get_cursor_x(&self) -> u16 {
        (self.render_bounds.0 + self.cursor.saturating_sub(self.visible) as u16)
            .min(self.render_bounds.1)
    }

    pub fn handle_input(&mut self, event: KeyCode) {
        match event {
            KeyCode::Char(c) => {
                self.input.insert(self.cursor, c);
                self.right();
            }
            KeyCode::Backspace => {
                if self.cursor != 0 && self.cursor <= self.input.len() {
                    self.input.remove(self.cursor - 1);
                    self.left();
                }
            }
            KeyCode::Delete => {
                if self.cursor != self.input.len() && self.cursor <= self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                self.left();
            }
            KeyCode::Right => {
                self.right();
            }
            _ => (),
        }
    }

    pub fn render(&mut self, area: Rect, f: &mut Frame<'_>, active: bool) {
        f.render_stateful_widget(TextPrompt::<R>(PhantomData), area, self);

        if active {
            f.set_cursor_position((self.get_cursor_x(), area.y));
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TextPrompt<R: Clone>(pub PhantomData<R>);

impl<R> StatefulWidget for TextPrompt<R>
where
    R: Clone,
{
    type State = TextPromptState<R>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.calculate_visible(area);
        state.render_bounds.0 = area.x;
        state.render_bounds.1 = area.x + area.width;

        let limit: String = state
            .input
            .chars()
            .skip(state.visible)
            .take(area.width as usize)
            .collect();
        buf.set_string(area.x, area.y, limit, Style::new());
    }
}

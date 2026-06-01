use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, WidgetRef},
};

/// Event emitted from a polar prompt in response to a terminal event.
#[derive(Debug, Clone, Copy)]
pub enum PolarPromptEvent {
    /// The terminal event was consumed.
    ///
    /// Contains the confirmed option as `true` (positive) or `false` (negative), or `None` if the
    /// terminal event did not result in a confirmation.
    Consumed(Option<bool>),

    /// The terminal event was not consumed.
    Ignored,
}

/// Widget for prompting the user with polar questions (e.g. yes/no).
///
/// Consists of a message with a horizontal list presenting the two options.
///
/// The left and right options correspond to the positive and negative answers, respectively.
///
/// # Examples
///
/// ```
///  use ratatui::{prelude::*, widgets::{Block, Paragraph, WidgetRef}};
///  use crate::ui::polarinput::PolarPrompt;
//
///  let block = Block::bordered();
//
///  let prompt = PolarPrompt::new(
///      Paragraph::new("Make a coffee?").centered(),
///      ("Confirm".to_string(), "Cancel".to_string()),
///  );
//
///  let width = 40u16;
///  let area = Rect {x: 0, y: 0, width, height: (prompt.line_count(width) + 2) as u16};
///  let mut buf = Buffer::empty(area);
//
///  block.render_ref(area, &mut buf);
///  prompt.render_ref(block.inner(area), &mut buf);
//
///  // Renders
///  // ┌──────────────────────────────────────┐
///  // │            Make a coffee?            │
///  // │                                      │
///  // │     [Confirm]           Cancel       │
///  // └──────────────────────────────────────┘
/// ```
#[derive(Debug, Clone)]
pub struct PolarPrompt<'a> {
    message: Paragraph<'a>,
    labels: (String, String),
    selected: bool,
}

#[allow(unused)]
impl<'a> PolarPrompt<'a> {
    /// Create a polar prompt with the given message and option labels.
    ///
    /// The first label is for the positive option and the second label is for the negative option.
    ///
    /// By default the positive option is selected.
    pub fn new(message: Paragraph<'a>, labels: (String, String)) -> Self {
        PolarPrompt {
            message,
            labels,
            selected: true,
        }
    }

    /// Returns the message displayed by this prompt.
    pub fn message(&self) -> &Paragraph<'a> {
        &self.message
    }

    /// Select an option.
    ///
    /// `true` corresponds to the positive option, and `false` to the negative option.
    pub fn select(&mut self, choice: bool) {
        self.selected = choice;
    }

    fn render_options(&self, area: Rect, buf: &mut Buffer) {
        let [positive_area, negative_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);

        let (positive, negative) = if self.selected == true {
            (
                Paragraph::new(Line::from_iter(["[", self.labels.0.as_str(), "]"])).centered().bold(),
                Paragraph::new(Line::from_iter([" ", self.labels.1.as_str(), " "])).centered(),
            )
        } else {
            (
                Paragraph::new(Line::from_iter([" ", self.labels.0.as_str(), " "])).centered(),
                Paragraph::new(Line::from_iter(["[", self.labels.1.as_str(), "]"])).centered().bold(),
            )
        };

        positive.render(positive_area, buf);
        negative.render(negative_area, buf);
    }

    /// The number of lines required to fully render this widget.
    pub fn line_count(&self, width: u16) -> usize {
        // The +2 accounts for the two options that are rendered below the message.
        self.message.line_count(width) + 2
    }

    /// Handle input events.
    ///
    /// Pressing the left or right arrow key navigates between the positive and negative options.
    /// Pressing Enter confirms the selected option.
    pub fn handle_key_event(&mut self, event: &KeyEvent) -> PolarPromptEvent {
        match event {
            KeyEvent {
                code: KeyCode::Left | KeyCode::Right,
                ..
            } => {
                self.selected = !self.selected;
                PolarPromptEvent::Consumed(None)
            }
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => PolarPromptEvent::Consumed(Some(self.selected)),
            _ => PolarPromptEvent::Ignored,
        }
    }
}

impl WidgetRef for PolarPrompt<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let [message_area, options_area] = Layout::vertical([
            Constraint::Length(self.message.line_count(area.width) as u16),
            Constraint::Length(1),
        ])
        .spacing(1)
        .areas(area);

        self.message.render_ref(message_area, buf);
        self.render_options(options_area, buf)
    }
}

//! TUI state and logic for auto-exporting samples as VCD

use std::io::Stdout;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::{Borders, Padding, Paragraph, Wrap};
use ratatui::{layout::Flex, prelude::*, widgets::Block};

use crate::ui::textinput::TextPromptState;
use crate::ui::listbox::Listbox;
use crate::auto_export::{AutoExportConfig, AutoExportMode};

const HELP_MESSAGE: &str = r#"UP and DOWN to navigate between elements
ENTER to start an auto-export session, ESC to cancel"#;

/// Represents the focused UI element
#[derive(Debug, Copy, Clone, PartialEq)]
enum Selected {
    FileName,
    Mode,
}

/// The response of the UI to a terminal event
pub enum AutoExportEventResponse {
    /// Close the program
    QuitProgram,
    /// Return to the main menu
    MainMenu(Option<AutoExportConfig>),
    /// Do nothing
    Nothing,
}

/// State of the auto-export UI
#[derive(Debug, Clone)]
pub struct State {
    file_name_state: TextPromptState<()>,
    mode_state: Listbox,
    selected: Selected,
}

impl State {
    /// Create a new state for this TUI page
    pub fn new() -> State {
        Self {
            file_name_state: TextPromptState::new(Some("dump.vcd"), ()),
            mode_state: Listbox::new(
                vec![
                    format!("{}" , AutoExportMode::Truncate),
                    format!("{}" , AutoExportMode::Append),
                ],
                0,
            ),
            selected: Selected::FileName,
        }
    }

    /// Renders the TUI
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        let _ = terminal.draw(|f| {
            let help_msg = Paragraph::new(HELP_MESSAGE)
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title("Keybinds"));

            let [main_area, help_message_area] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(help_msg.line_count(f.area().width) as u16),
            ])
            .areas(f.area());

            f.render_widget(help_msg, help_message_area);

            let main_block = Block::bordered().title("Auto-Export Options");

            let modes_description = Paragraph::new(concat!(
                "There are two auto-export modes: TRUNCATE overwrites a file with the lastest buffer as VCD, ",
                "while APPEND writes all buffers sequentially to the same VCD file."
            ))
            .wrap(Wrap { trim: true });

            // Account for borders
            let modes_description_line_count = modes_description.line_count(f.area().width.saturating_sub(2)) as u16;

            let [modes_description_area, file_name_area, mode_area] = Layout::vertical([
                    Constraint::Length(modes_description_line_count),
                    Constraint::Length(3),
                    Constraint::Length(1 + self.mode_state.items().len() as u16),
                ])
                .flex(Flex::Start)
                .spacing(1)
                .areas(main_block.inner(main_area));

            f.render_widget(&main_block, main_area);

            f.render_widget(&modes_description, modes_description_area);

            let file_name_block = Block::bordered().title("File Name");
            let file_name_inner_area = file_name_block.clone().padding(Padding::left(1)).inner(file_name_area);

            f.render_widget(file_name_block, file_name_area);

            if self.selected == Selected::FileName {
                self.file_name_state.render(file_name_inner_area, f, true);
                let block = Block::new().borders(Borders::TOP | Borders::BOTTOM);
                let inner_area = block.inner(file_name_area);
                f.render_widget("> ", inner_area);
            } else {
                self.file_name_state.render(file_name_inner_area, f, false);
            }

            let [mode_title_area, mode_state_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(self.mode_state.items().len() as u16)
            ])
            .areas(mode_area);

            f.render_widget("Auto-Export Mode", mode_title_area);
            self.mode_state.render(mode_state_area, f.buffer_mut());
        });
    }

    /// Handle TUI events
    pub fn handle_event(&mut self, event: &Event) -> AutoExportEventResponse {
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => AutoExportEventResponse::QuitProgram,
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => AutoExportEventResponse::MainMenu(None),
            Event::Key(KeyEvent {
                code: KeyCode::Enter, ..
            }) => {
                let file_name = self.file_name_state.input.clone();

                let Ok(mode) =
                    AutoExportMode::try_from(self.mode_state.get_selected() as u32)
                else {
                    return AutoExportEventResponse::Nothing;
                };

                AutoExportEventResponse::MainMenu(Some(AutoExportConfig { file_name, mode, }))
            },
            _ => {
                self.handle_input(event);
                AutoExportEventResponse::Nothing
            }
        }
    }

    fn handle_input(&mut self, event: &Event) {
        let Event::Key(KeyEvent {
            code: event_key, ..
        }) = event else {
            return;
        };

        let consumed = self.mode_state.handle_input(event);

        if consumed  {
            return;
        };

        if self.selected == Selected::FileName {
            self.file_name_state.handle_input(*event_key);
        }

        #[derive(Debug, Clone, Copy)]
        enum Direction {
            Up,
            Down,
        }

        let direction = match event_key {
            KeyCode::Up => Direction::Up,
            KeyCode::Down => Direction::Down,
            _ => return,
        };

        self.selected = match (&self.selected, direction) {
            (Selected::FileName, Direction::Up) => Selected::FileName,
            (Selected::FileName, Direction::Down) => Selected::Mode,
            (Selected::Mode, Direction::Up) => Selected::FileName,
            (Selected::Mode, Direction::Down) => Selected::Mode,
        };

        if let Event::Key(KeyEvent { code, .. }) = event {
            self.file_name_state.handle_input(*code);
        }

        if self.selected == Selected::Mode {
            self.mode_state.set_focus(true);
            self.mode_state.set_hover(usize::MIN);
        }
    }
}

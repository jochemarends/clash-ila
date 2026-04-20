use std::io::Stdout;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Flex, prelude::*, widgets::Block};

use crate::ui::textinput::TextPromptState;
use crate::ui::listbox::Listbox;

/// Represents the different modes for auto-exporting as VCD.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum AutoExportMode {
    /// Before writing each [`SignalCluster`], truncates the file, then writes the VCD header,
    /// variable definition section, and variable initialization section.
    Truncate,
    /// Writes the VCD header, variable definition section, and variable initialization section
    /// before writing the first [`SignalCluster`]. Subsequent [`SignalCluster`]s are appended.
    Append,
}

/// Auto-export options.
#[derive(Debug, Clone)]
pub struct AutoExportOptions {
    pub file_name: String,
    pub mode: AutoExportMode,
}

impl TryFrom<u32> for AutoExportMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AutoExportMode::Truncate),
            1 => Ok(AutoExportMode::Append),
            _ => Err(()),
        }
    }
}

/// Represents the focused UI element.
#[derive(Debug, Copy, Clone, PartialEq)]
enum Selected {
    FileName,
    Mode,
}

/// The response of the auto-export UI on input events.
pub enum AutoExportEventResponse {
    /// Close the program
    QuitProgram,
    /// Return to the main menu.
    ///
    /// * `message` - Message to display in the log of the main menu.
    /// * `options` - The confirmed export options, or `None` if the prompt was cancelled.
    MainMenu{ message: String, options: Option<AutoExportOptions> },
    Nothing,
}

/// State of the auto-export UI.
#[derive(Debug, Clone)]
pub struct State {
    file_name_state: TextPromptState<()>,
    mode_state: Listbox,
    selected: Selected,
}

impl State {
    pub fn new() -> State {
        Self {
            file_name_state: TextPromptState::new(Some("dump.vcd"), ()),
            mode_state: Listbox::new(vec!["TRUNCATE", "APPEND"], 0),
            selected: Selected::FileName,
        }
    }

    pub fn render(&mut self, area: Rect, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        let _ = terminal.draw(|f| {
            let main_block = Block::bordered()
                .title("Auto-export");

            f.render_widget(&main_block, area);

            let layout = Layout::new(
                    Direction::Vertical,
                    [
                        Constraint::Length(3),
                        Constraint::Length(2),
                    ],
                )
                .flex(Flex::Start)
                .spacing(0)
                .split(area);

            {
                let area = main_block.inner(layout[0]);
                let borders = Block::bordered().title("file name");
                let border_area = Rect::new(area.x, area.y, area.width, 3);
                let input_area = Rect::new(area.x + 2, area.y + 1, area.width - 1, 1);
                let input_select = Rect::new(area.x, area.y + 1, 1, 1);

                f.render_widget(borders, border_area);

                let element_active = self.selected == Selected::FileName;
                self.file_name_state.render(input_area, f, element_active);

                if self.selected == Selected::FileName {
                    f.render_widget(">", input_select);
                }
            }

            {
                self.mode_state.render(main_block.inner(layout[1]), f.buffer_mut());
            }
        });
    }

    pub fn handle_event(&mut self, event: &Event) -> AutoExportEventResponse {
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => AutoExportEventResponse::QuitProgram,
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => AutoExportEventResponse::MainMenu { message: "Cancelled auto-export".into(), options: None },
            Event::Key(KeyEvent {
                code: KeyCode::Enter, ..
            }) => {
                let file_name = self.file_name_state.input.clone();

                let Ok(mode) =
                    AutoExportMode::try_from(self.mode_state.get_selected() as u32)
                else {
                    return AutoExportEventResponse::Nothing;
                };

                AutoExportEventResponse::MainMenu { message: "Setup auto-export".into(), options: Some(AutoExportOptions { file_name, mode, }) }
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

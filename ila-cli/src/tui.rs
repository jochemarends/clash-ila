use std::io::{Read, Write};
use std::path::Path;
use std::time::Instant;
use std::{io, time::Duration};

use crossterm::event::{Event as TuiEvent, KeyEvent, KeyModifiers};
use crossterm::{
    event::{poll, read, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::style::Stylize;
use ratatui::text::{Line, Text};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Margin, Rect},
    widgets::*,
    *,
};

use crate::cli_registers::IlaRegisters;
use crate::communication::{RegisterOutput, SignalCluster, perform_buffer_reads, perform_register_operation};
use crate::config::IlaConfig;
use crate::predicates::IlaPredicate;
use crate::predicates::PredicateTarget;
use crate::predicates_tui::State as PredState;
use crate::ui::polarinput::{PolarPrompt, PolarPromptEvent};
use crate::ui::textinput::TextPromptState;
use crate::vcd::write_to_vcd;
use crate::auto_export::AutoExportSession;
use crate::auto_export_tui::State as AutoExportState;

/// The keybind text displayed in the TUI
const KEYBIND_TEXT: &str = r#"  CTRL-c ---   Exit
  space  ---   Read samples (if triggered)
  t      ---   Change trigger point
  p      ---   Change trigger predicates
  c      ---   Change capture predicates
  R      ---   Toggle automatic reading of samples
  r      ---   Re-arm trigger
  a      ---   Toggle auto trigger re-arm
  v      ---   Write signals to VCD dump
  e      ---   Toggle auto-export
"#;

/// The reason to prompt the user with, mostly important to decide what to do next after a user has
/// inputted data to the prompt
#[derive(Debug, PartialEq, Eq, Clone)]
enum TextPromptReason {
    /// Prompt for the filename to save the VCD too
    SaveVcd,
    /// Prompt to change the trigger
    ChangeTrigger,
}

#[derive(Debug, Clone)]
enum Prompt<'a> {
    TextPrompt(TextPromptState<TextPromptReason>),
    StopAutoExportPrompt(PolarPrompt<'a>),
}

/// The state of the TUI
#[derive(Debug, Clone)]
enum TuiState<'a> {
    /// The TUI is in an idle state
    Main,
    /// The TUI is currently prompting the user for input
    InPrompt(Prompt<'a>),
    /// Manages the trigger predicates
    Predicates(PredState<'a>),
    AutoExport(AutoExportState),
}

/// The response of the TUI key event handler
#[derive(Debug, Clone, Copy)]
enum KeyResponse {
    /// Keep the current state
    Nothing,
    /// Quit the program
    QuitProgram,
    /// Changes to the ILA were applied, possibly re-arm the trigger
    AppliedChanges,
}

/// A TUI session
///
/// This struct owns the TUI and once it goes out of scope, so will the TUI
/// It handles everything from rendering to interacting with the FPGA, and that's why it also needs
/// the ILA configuration.
pub struct TuiSession<'a> {
    /// The terminal backend to render the TUI on
    term: Terminal<CrosstermBackend<io::Stdout>>,
    /// In what state is the TUI currently at?
    state: TuiState<'a>,
    /// The ILA configuration, specifying certain aspects of the ILA
    config: &'a IlaConfig,
    /// A log for interactions to log their activity too, regularly gets truncated to fit the
    /// screen during rendering
    log: Vec<String>,
    /// A list of signal clusters captured by the ILA
    captured: Vec<SignalCluster>,
    /// Checks if the predicate is triggered or not
    triggered: bool,
    /// Will automatically sample when the system detected a trigger
    /// NOTE: due to the trigger only being sampled X every seconds, this cannot be used to
    /// determine the time of trigger
    auto_sample: bool,
    /// The amount of samples currently stored in the buffer
    sample_count: u32,
    /// The time since the last triggered check has been performed
    last_trigger_check: Instant,
    /// If a configuration change has been made and this is set, it will rearm the trigger as well
    /// Otherwise it will only upload the new changes
    auto_reset: bool,
    /// The connected device path
    device_path: String,
    /// Holds the auto-export session, if active
    auto_export_session: Option<AutoExportSession>,
}

impl<'a> TuiSession<'a> {
    /// Create a new TUI session associated with a certain IlaConfig
    ///
    /// * `config` - The ILA configuration the TUI should use to properly communicate with the ILA
    pub fn new(config: &'a IlaConfig, device_path: &Path) -> Result<TuiSession<'a>, io::Error> {
        enable_raw_mode()?;

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);

        Ok(TuiSession {
            term: Terminal::new(backend)?,
            state: TuiState::Main,
            config,
            log: Vec::with_capacity(64),
            captured: vec![],
            triggered: false,
            auto_sample: true,
            sample_count: 0,
            last_trigger_check: Instant::now(),
            auto_reset: false,
            device_path: device_path.display().to_string(),
            auto_export_session: None,
        })
    }

    fn render_main(&mut self) {
        let _ = self.term.draw(|f| {
            let size = f.area();
            let title_block = Block::default()
                .title(format!("ILA - Port {}", self.device_path))
                .borders(Borders::ALL);
            f.render_widget(title_block, size);

            let main_layout = Layout::default()
                .direction(layout::Direction::Vertical)
                .constraints([
                    Constraint::Length(KEYBIND_TEXT.lines().count() as u16 + 2),
                    Constraint::Min(1),
                ])
                .split(size.inner(Margin {
                    vertical: 1,
                    horizontal: 1,
                }));
            let info_layout = Layout::default()
                .direction(layout::Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Length(6), Constraint::Fill(1)])
                .split(main_layout[1]);

            // Ensure the lines fit within the Paragraph's range
            // If a string spans multiple lines / overflows, too bad*!*
            self.log = self
                .log
                .drain(
                    self.log
                        .len()
                        .saturating_sub(info_layout[1].height as usize)..,
                )
                .collect();

            let help_menu = Paragraph::new(KEYBIND_TEXT)
                .block(Block::default().title("Keybinds").borders(Borders::ALL));
            let info_menu_block = Block::default().title("Info").borders(Borders::ALL);
            let info_info_section = Paragraph::new(Text::from_iter([
                Line::from_iter([
                    "Received ".into(),
                    self.captured.len().to_string().bold().blue(),
                    " captured".into(),
                ]),
                Line::from_iter([
                    "Buffer currently contains ".into(),
                    self.sample_count.to_string().bold().blue(),
                    " samples".into(),
                ]),
                Line::from_iter([
                    "The ILA is ".into(),
                    match self.triggered {
                        true => "TRIGGERED".bold().green(),
                        false => "NOT TRIGGERED".bold().red(),
                    },
                ]),
                Line::from_iter([
                    "Auto-rearm is ".into(),
                    match self.auto_reset {
                        true => "ENABLED".bold().green(),
                        false => "DISABLED".bold().red(),
                    },
                ]),
                Line::from_iter([
                    "Auto-sample is ".into(),
                    match self.auto_sample {
                        true => "ENABLED".bold().green(),
                        false => "DISABLED".bold().red(),
                    },
                ]),
                Line::from_iter({
                    if let Some(ref session) = self.auto_export_session {
                        vec![
                            "Auto-export session is".into(),
                            " RUNNING".bold().green(),
                            format!(
                                " | file: '{}' | mode: {} | duration: {}",
                                session.config().file_name,
                                session.config().mode,
                                {
                                    let seconds = session.started_at().elapsed().as_secs();
                                    format!("{}h {}m {}s", seconds / 3600, (seconds % 3600) / 60, (seconds % 60))
                                },
                            ).into(),
                        ]
                    } else {
                        vec![
                            "Auto-export session is".into(),
                            " NOT RUNNING".bold().red(),
                        ]
                    }
                }),
            ]));
            let info_log_section = Paragraph::new(
                self.log
                    .iter()
                    .cloned()
                    .map(|mut s| {
                        s.push('\n');
                        s
                    })
                    .collect::<String>(),
            )
            .block(Block::default().borders(Borders::TOP));

            f.render_widget(help_menu, main_layout[0]);
            f.render_widget(info_menu_block, main_layout[1]);
            f.render_widget(info_info_section, info_layout[0]);
            f.render_widget(info_log_section, info_layout[1]);

            if let TuiState::InPrompt(prompt) = &mut self.state {
                match prompt {
                    Prompt::TextPrompt(text_prompt) => {
                        let width = size.width.saturating_sub(10);
                        let around = Rect::new(
                            size.x + size.width.div_ceil(2).saturating_sub(width / 2),
                            size.y + size.height.div_ceil(2).saturating_sub(1),
                            width.min(size.width),
                            3.min(size.height),
                        );

                        let title = match text_prompt.reason {
                            TextPromptReason::SaveVcd => "Save VCD file (default: dump.vcd)".into(),
                            TextPromptReason::ChangeTrigger => {
                                format!("Change trigger point [0-{}]", self.config.buffer_size)
                            }
                        };

                        f.render_widget(Clear, around);
                        let decoration = Block::default().title(title).borders(Borders::ALL);
                        f.render_widget(decoration, around);

                        let center = Rect::new(
                            around.x + 1,
                            around.y + 1,
                            around.width.saturating_sub(2),
                            around.height.saturating_sub(2),
                        );

                        text_prompt.render(center, f, true);
                    },
                    Prompt::StopAutoExportPrompt(polar_prompt) => {
                        let block = Block::bordered();

                        let width = (size.width / 2).min(size.width);

                        // +2 to account for the borders
                        let height = polar_prompt.line_count(width) as u16 + 2;

                        let area = Rect {
                            x: size.x + size.width.div_ceil(2).saturating_sub(width / 2),
                            y: size.y + size.height.div_ceil(2).saturating_sub(height / 2),
                            width,
                            height,
                        };

                        f.render_widget(Clear, area);

                        block.render_ref(area, f.buffer_mut());
                        polar_prompt.render(block.inner(area), f.buffer_mut());
                    },
                }
            }
        });
    }

    /// Render the TUI
    pub fn render(&mut self) {
        match &mut self.state {
            TuiState::Main => self.render_main(),
            TuiState::InPrompt(_) => self.render_main(),
            TuiState::Predicates(state) => {
                state.render(&mut self.term);
            },
            TuiState::AutoExport(state) => {
                state.render(&mut self.term);
            },
        }
    }

    /// Handle the keypresses. Returns wether or not it should break out of the main loop or not
    fn on_key_event<T: Read + Write>(&mut self, event: KeyEvent, tx_port: &mut T) -> KeyResponse {
        let response = match (&mut self.state, event.code, event.modifiers) {
            (_, KeyCode::Char('c'), KeyModifiers::CONTROL) => KeyResponse::QuitProgram,
            (TuiState::Main, KeyCode::Char('r'), _) => {
                if let Err(err) =
                    perform_register_operation(tx_port, self.config, &IlaRegisters::TriggerReset)
                {
                    self.log.push(format!("Error: {err}"));
                } else {
                    self.log.push("Re-armed trigger succesfully".into());
                }
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('a'), _) => {
                self.auto_reset = !self.auto_reset;
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('R'), _) => {
                self.auto_sample = !self.auto_sample;
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char(' '), _) => {
                self.triggered = match perform_register_operation(
                    tx_port,
                    self.config,
                    &IlaRegisters::TriggerState,
                ) {
                    Ok(RegisterOutput::TriggerState(state)) => state,
                    _ => {
                        self.log.push("Unable to check for trigger status".into());
                        false
                    }
                };

                if !self.triggered {
                    self.log
                        .push("System is not triggered, refusing to read samples".into());
                } else {
                    let indices: Vec<u32> = (0_u32..self.sample_count).collect();
                    match perform_buffer_reads(tx_port, self.config, 0_u32..self.sample_count) {
                        Ok(RegisterOutput::BufferContent(cluster)) => {
                            if let Some(ref mut session) = self.auto_export_session {
                                if let Err(err) = session.export_cluster(&cluster) {
                                    self.log.push(format!("Error: {err}"));
                                }
                            }
                            self.captured.push(cluster);
                        }
                        Ok(_) => self
                            .log
                            .push("Unexpected output when reading buffer".into()),
                        Err(err) => self.log.push(format!("Error: {err}")),
                    }
                }
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('t'), _) => {
                self.state = TuiState::InPrompt(Prompt::TextPrompt(TextPromptState::new(
                    Some("0"),
                    TextPromptReason::ChangeTrigger,
                )));
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('p'), _) => {
                if let Ok(predicate) =
                    IlaPredicate::from_ila(tx_port, self.config, PredicateTarget::Trigger)
                {
                    self.state = TuiState::Predicates(PredState::new(self.config, predicate));
                } else {
                    self.log.push(
                        "Unable to retrieve current trigger predicate configuration from the ILA"
                            .to_string(),
                    );
                }
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('c'), _) => {
                if let Ok(predicate) =
                    IlaPredicate::from_ila(tx_port, self.config, PredicateTarget::Capture)
                {
                    self.state = TuiState::Predicates(PredState::new(self.config, predicate));
                } else {
                    self.log.push(
                        "Unable to retrieve current capture predicate configuration from the ILA"
                            .to_string(),
                    );
                }
                KeyResponse::Nothing
            }
            (TuiState::Main, KeyCode::Char('v'), _) => {
                self.state = TuiState::InPrompt(Prompt::TextPrompt(TextPromptState::new(
                    Some("dump.vcd"),
                    TextPromptReason::SaveVcd,
                )));
                KeyResponse::Nothing
            },
            (TuiState::Main, KeyCode::Char('e'), _) => {
                if self.auto_export_session.is_some() {
                    let mut state = PolarPrompt::new(
                        Paragraph::new("Stop the running auto-export session?")
                            .wrap(Wrap { trim: true })
                            .centered(),
                        ("Yes".to_string(), "No".to_string()),
                    );
                    state.select(false);

                    self.state = TuiState::InPrompt(Prompt::StopAutoExportPrompt(state));
                } else {
                    self.state = TuiState::AutoExport(AutoExportState::new());
                }
                KeyResponse::Nothing
            },
            (TuiState::InPrompt(prompt), KeyCode::Esc, _) => {
                if let Prompt::TextPrompt(text_prompt) = prompt {
                    if text_prompt.reason == TextPromptReason::SaveVcd {
                        self.log.push("Cancelled save".to_string());
                    }
                }
                self.state = TuiState::Main;
                KeyResponse::Nothing
            }
            (TuiState::InPrompt(Prompt::TextPrompt(prompt)), KeyCode::Enter, _) => {
                // Handle the case of whenever a prompt gets completed
                // I want to move this to a seperate function, however due to borrow limits I can't
                // and that's kind of very annoying
                let response = match prompt.reason {
                    TextPromptReason::SaveVcd => {
                        if let Some(sample) = self.captured.last() {
                            if let Err(err) =
                                write_to_vcd(sample, self.config, prompt.input.clone())
                            {
                                self.log.push(format!("Error when saving VCD: {}", err));
                            } else {
                                self.log.push("Saved succesfully!".to_string());
                            }
                        } else {
                            self.log.push("Nothing to save".to_string());
                        }
                        KeyResponse::Nothing
                    }
                    TextPromptReason::ChangeTrigger => match prompt.input.parse() {
                        Ok(n) if n > self.config.buffer_size as u32 => {
                            self.log
                                .push("Invalid input; must be specified range".to_string());
                            KeyResponse::Nothing
                        }
                        Ok(n) => {
                            let (msg, response) = match perform_register_operation(
                                tx_port,
                                self.config,
                                &IlaRegisters::TriggerPoint(n),
                            ) {
                                Ok(_) => (
                                    String::from("Trigger point change made"),
                                    KeyResponse::AppliedChanges,
                                ),
                                Err(err) => (format!("Error: {err}"), KeyResponse::Nothing),
                            };
                            self.log.push(msg);
                            response
                        }
                        Err(_) => {
                            self.log.push(
                                "Invalid input; must be a unsigned 32 bit number".to_string(),
                            );
                            KeyResponse::Nothing
                        }
                    },
                };

                self.state = TuiState::Main;
                response
            }
            (TuiState::InPrompt(Prompt::TextPrompt(text_prompt)), keycode, _) => {
                text_prompt.handle_input(keycode);
                KeyResponse::Nothing
            },
            (TuiState::InPrompt(Prompt::StopAutoExportPrompt(prompt)), _, _) => {
                if let PolarPromptEvent::Consumed(Some(stop_auto_export)) = prompt.handle_key_event(&event) {
                    if stop_auto_export {
                        self.auto_export_session = None;
                        self.log.push("Auto-export session stopped".to_string());
                    }
                    self.state = TuiState::Main;
                };
                KeyResponse::Nothing
            },
            _ => KeyResponse::Nothing,
        };

        self.render();

        response
    }

    /// The main TUI loop
    ///
    /// Handles everything from rendering to the input of the TUI interface
    pub fn main_loop<T: Read + Write>(&mut self, mut tx_port: T) {
        self.render();

        let mut last_should_sample = false;
        loop {
            if let Ok(true) = poll(Duration::from_millis(100)) {
                let raw_event = read();
                let mut should_rearm = false;

                // This is structured a bit weirdly, due to me not expecting the TUI to be very
                // complicated at first. It expanded more than I initially anticipated
                //
                // Rewriting it would take quite some time, time I do not have at the moment
                if let TuiState::Predicates(state) = &mut self.state {
                    if let Ok(ref event) = raw_event {
                        let stop_program = state.handle_event(&mut tx_port, self.config, event);
                        self.render();

                        match stop_program {
                            crate::predicates_tui::PredicateEventResponse::QuitProgram => return,
                            crate::predicates_tui::PredicateEventResponse::MainMenu((
                                log,
                                changes,
                            )) => {
                                self.state = TuiState::Main;
                                self.log.push(log);
                                should_rearm = changes;
                            }
                            crate::predicates_tui::PredicateEventResponse::Nothing => continue,
                        }
                    }
                }

                if let TuiState::AutoExport(state) = &mut self.state {
                    if let Ok(ref event) = raw_event {
                        let stop_program = state.handle_event(event);
                        self.render();

                        match stop_program {
                            crate::auto_export_tui::AutoExportEventResponse::QuitProgram => return,
                            crate::auto_export_tui::AutoExportEventResponse::MainMenu(config) => {
                                self.state = TuiState::Main;
                                if let Some(config) = config {
                                    if let Ok(session) = AutoExportSession::new(config, &self.config) {
                                        self.auto_export_session = Some(session);
                                        self.log.push("Auto-export session started".to_string());
                                    } else {
                                        self.log.push("Auto-export session failed to start".to_string());
                                    }
                                } else {
                                    self.log.push("Auto-export cancelled".to_string());
                                }
                            }
                            crate::auto_export_tui::AutoExportEventResponse::Nothing => continue,
                        }
                    }
                }

                let event = match raw_event {
                    Ok(TuiEvent::Key(key)) => key,
                    Ok(TuiEvent::Resize(..)) => {
                        self.render();
                        continue;
                    }
                    _ => continue,
                };

                match self.on_key_event(event, &mut tx_port) {
                    KeyResponse::Nothing => (),
                    KeyResponse::QuitProgram => break,
                    KeyResponse::AppliedChanges => should_rearm = true,
                };

                if should_rearm && self.auto_reset {
                    let log_message = match perform_register_operation(
                        &mut tx_port,
                        self.config,
                        &IlaRegisters::TriggerReset,
                    ) {
                        Ok(_) => "Auto-rearmed the trigger",
                        Err(_) => "Failed to auto-rearm the trigger",
                    };
                    self.log.push(log_message.into())
                }
            }

            let now = Instant::now();
            let duration = now.duration_since(self.last_trigger_check);
            if duration >= Duration::from_millis(500) {
                self.triggered = match perform_register_operation(
                    &mut tx_port,
                    self.config,
                    &IlaRegisters::TriggerState,
                ) {
                    Ok(RegisterOutput::TriggerState(state)) => state,
                    _ => {
                        self.log.push("Unable to check for trigger status".into());
                        false
                    }
                };
                self.sample_count = match perform_register_operation(
                    &mut tx_port,
                    self.config,
                    &IlaRegisters::SampleCount,
                ) {
                    Ok(RegisterOutput::SampleCount(count)) => count,
                    _ => {
                        self.log.push("Unable to check for sample count".into());
                        self.sample_count
                    }
                };
                let should_sample = self.auto_sample && self.triggered && self.sample_count > 0;
                if should_sample && !last_should_sample {
                    match perform_buffer_reads(&mut tx_port, self.config, 0_u32..self.sample_count) {
                        Ok(RegisterOutput::BufferContent(cluster)) => {
                            if let Some(ref mut session) = self.auto_export_session {
                                if let Err(err) = session.export_cluster(&cluster) {
                                    self.log.push(format!("Error: {err}"));
                                }
                            }
                            self.captured.push(cluster);
                        }
                        Ok(_) => self
                            .log
                            .push("Unexpected output when reading buffer".into()),
                        Err(err) => self.log.push(format!("Error: {err}")),
                    }
                }
                last_should_sample = should_sample;

                self.render();
            }
        }
    }
}

impl Drop for TuiSession<'_> {
    // Make sure that when the TuiSession goes out of scope, we exit the alternative screen, as to
    // not break the user's terminal session (hopefully)
    fn drop(&mut self) {
        let _ = execute!(self.term.backend_mut(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

//! TUI state and logic for driving the ILA's output signals

use std::io::{Stdout, Write as IoWrite, Read as IoRead, Result as IoResult};

use bitvec::{order::Msb0, vec::BitVec};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use num::{BigInt, BigUint, bigint::Sign};
use ratatui::{Terminal, prelude::*, widgets::{Block, Borders, Padding, Paragraph, Wrap}};

use crate::{cli_registers::IlaRegisters, communication::bv_to_bytes, config::IlaSignals};
use crate::communication::{Signal, SignalCluster, perform_register_operation};
use crate::config::{IlaConfig, IlaSignal};
use crate::predicates_tui::NumericState;
use crate::ui::textinput::TextPromptState;

const HELP_MESSAGE: &str = r#"UP and DOWN to navigate between elements
ENTER to apply the values to the output signals, ESC to discard"#;

/// The response of the TUI to a terminal events
pub enum EventResponse {
    QuitProgram,
    MainMenu,
    Nothing,
    Error(String),
}

/// State of the TUI for driving the ILA's output signals
#[derive(Debug, Clone)]
pub struct State<'a> {
    signals: &'a IlaSignals,
    input_states: Vec<TextPromptState<()>>,
    cursor_position: usize,
}

impl<'a> State<'a> {
    /// Create a new state for the TUI that drives the ILA's output signals
    ///
    /// The initial values do not reflect the actual values of the ILA's outputs and are initalized
    /// to zero.
    pub fn new(ila: &'a IlaConfig, init: &SignalCluster) -> IoResult<State<'a>> {
        if init.cluster.len() != ila.outputs.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Expected {} output signals, got {}",
                    init.cluster.len(),
                    ila.outputs.len(),
                )
            ));
        }

        let input_states: Vec<_> = init.cluster
            .iter()
            .filter_map(|signal| {
                signal
                    .samples
                    .first()
                    .map(|sample| (bv_to_bytes(sample), signal.width))
            })
            .map(|(v, width)| {
                BigInt::from_bytes_be(Sign::Plus, &v).clamp(
                    BigInt::new(Sign::Plus, vec![0]),
                    BigInt::new(Sign::Plus, vec![2]).pow(width as u32)
                    - BigInt::new(Sign::Plus, vec![1]),
                )
            })
            .map(|byte_vec| {
                TextPromptState::new(Some(format!("0x{}", byte_vec.to_str_radix(16))), ())
            })
            .collect();

        Ok(State {
            signals: &ila.outputs,
            input_states,
            cursor_position: 0,
        })
    }

    /// Renders the TUI for output signals.
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        let _ = terminal.draw(|f| {
            let help_msg = Paragraph::new(HELP_MESSAGE)
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title("Keybinds"));

            let [area, help_msg_area] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(help_msg.line_count(f.area().width) as u16),
            ])
            .areas(f.area());

            f.render_widget(help_msg, help_msg_area);
            self.render_body(area, f);
        });
    }

    fn render_body(&mut self, area: Rect, f: &mut Frame) {
        let block = Block::bordered().title("Outputs");
        f.render_widget(&block, area);
        let area = block.inner(area);

        let description = Paragraph::new("The page for driving output signals")
            .wrap(Wrap { trim: true });

        let input_status = {
            let is_valid = self.signals
                .iter()
                .map(|s| s.width)
                .zip(&self.input_states)
                .all(|(width, state)| {
                    NumericState::immediate_parse(&state.input)
                        .is_ok_and(|uint| uint < BigUint::from(2u32).pow(width as u32))
                });

            Paragraph::new(if is_valid {
                "Provided input is valid".green()
            } else {
                "Provided input is INVALID".red()
            }).wrap(Wrap { trim: true })
        };

        let [description_area, input_status_area, area] = Layout::vertical([
            Constraint::Length(description.line_count(area.width) as u16),
            Constraint::Length(input_status.line_count(area.width) as u16),
            Constraint::Fill(1),
        ])
        .spacing(1)
        .areas(area);

        f.render_widget(description, description_area);

        f.render_widget(input_status, input_status_area);

        let layout = Layout::vertical(std::iter::repeat_n(Constraint::Length(3), self.signals.len()))
            .split(area);

        for (index, signal) in self.signals.iter().enumerate() {
            let area = layout[index];
            let prompt = &mut self.input_states[index];

            let signal_label = {
                let max_bound = BigInt::from(2).pow(signal.width as u32) - 1;
                format!(" {} - input range [0 - {:#x}] ", signal.name, max_bound)
            };

            let block = Block::bordered().title(signal_label);
            f.render_widget(&block, area);

            let inner_area = block.padding(Padding::left(1)).inner(area);
            let is_selected = self.cursor_position == index;
            prompt.render(inner_area, f, is_selected);

            if is_selected {
                let block = Block::new().borders(Borders::TOP | Borders::BOTTOM);
                let inner_area = block.inner(area);
                f.render_widget("> ", inner_area);
            }
        }
    }

    /// Handles TUI events for driving output signals.
    pub fn handle_event<T>(&mut self, medium: &mut T, ila: &IlaConfig, event: &Event) -> EventResponse
    where
        T: IoRead + IoWrite {
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                EventResponse::QuitProgram
            },
            Event::Key(KeyEvent { code: KeyCode::Esc, .. }) => {
                EventResponse::MainMenu
            },
            Event::Key(KeyEvent { code: KeyCode::Enter, .. }) => {
                self.apply_outputs(medium, ila)
            },
            _ => {
                self.handle_input(event);
                EventResponse::Nothing
            },
        }
    }

    // Writes the entered inputs to the ILA's staging output signal buffer, then commits the staged
    // values.
    fn apply_outputs<T>(&mut self, medium: &mut T, ila: &IlaConfig) -> EventResponse
    where
        T: IoRead + IoWrite {
        fn biguint_to_signal((n, signal): (BigUint, &IlaSignal)) -> Signal {
            let reference: BitVec<u8, Msb0> = BitVec::from_vec(n.to_bytes_be());
            let mut base: BitVec<u8, Msb0> = BitVec::with_capacity(signal.width);
            for index in (0..signal.width).rev() {
                base.push(match reference.len().checked_sub(index + 1) {
                    Some(index) => reference[index],
                    None => false,
                });
            }

            Signal {
                name: signal.name.clone(),
                width: signal.width,
                samples: vec![base],
            }
        }

        let valid_inputs: Vec<Signal> = self.signals
            .iter()
            .zip(&self.input_states)
            .filter_map(|(s, state)| {
                NumericState::immediate_parse(&state.input)
                    .ok()
                    .map(|n| (n, s))
            })
            .map(biguint_to_signal)
            .collect();

        // Make sure the inputs are valid
        if valid_inputs.len() != self.signals.len() {
            // Above the input fields, there's already a message indicating whether the inputs are
            // valid, so we don't have to mention it again.
            return EventResponse::Nothing;
        }

        let cluster = SignalCluster {
            cluster: valid_inputs,
            timestamp: std::time::Duration::ZERO,
        };

        let result = perform_register_operation(
            medium,
            ila,
            &IlaRegisters::OutputBackBuffer(cluster.to_data()),
        )
        .and_then(|_| perform_register_operation(medium, ila, &IlaRegisters::OutputBufferSync));

        match result {
            Ok(_) => EventResponse::Nothing,
            Err(_) => EventResponse::Error("Something went wrong while trying to update the output signals".to_string()),
        }
    }

    fn handle_input(&mut self, event: &Event) {
        if let Event::Key(KeyEvent { code, .. }) = event {
            if let Some(ref mut prompt) = self.input_states.get_mut(self.cursor_position) {
                prompt.handle_input(*code);
            }

            self.cursor_position = match code {
                KeyCode::Up => self.cursor_position.saturating_sub(1),
                KeyCode::Down => self.cursor_position.saturating_add(1).min(self.signals.len() - 1),
                _ => self.cursor_position,
            };
        }
    }

}

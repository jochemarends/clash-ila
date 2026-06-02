use std::{
    io::{Read, Stdout, Write},
    time::Duration,
};

use crate::{
    communication::{Signal, SignalCluster, bv_to_bytes},
    config::{IlaConfig, IlaSignal},
    predicates::{IlaPredicate, PredicateOperation, PredicateTarget},
    ui::textinput::TextPromptState,
};
use bitvec::{
    field::BitField, order::{Lsb0, Msb0}, vec::BitVec, view::BitView
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use num::{
    bigint::{ParseBigIntError, Sign},
    BigInt, BigUint, Num,
};
use ratatui::{
    layout::{Constraint, Flex, Layout, Margin, Rect},
    prelude::CrosstermBackend,
    style::Stylize,
    widgets::{Block, Borders, Paragraph, Tabs, WidgetRef},
    Frame, Terminal,
};

use crate::ui::checkbox::Checkbox;
use crate::ui::listbox::Listbox;

const HELP_MESSAGE: &str = r#"CTRL-LEFT and CTRL-RIGHT to navigate tabs
UP and DOWN to navigate between elements
ENTER to save changes, ESC to discard"#;

trait PredicatePage {
    /// Render the predicate page
    fn render(&self, state: &mut State, f: &mut Frame<'_>, area: Rect);
    /// Navigate the predicate page
    fn navigate(&self, state: &mut State, event: &Event);
}

/// An enum for determining in which base a user wrote numeric inputs too
///
/// This is indicates by the prefix of the number, if the number has no prefix it is assumed to be
/// base-10 (Decimal)
#[derive(Debug, Clone, Copy)]
pub enum NumericState {
    Decimal,
    Hex,
    Octal,
    Binary,
}

impl NumericState {
    /// Create a `NumericState` from a string, will look at the first two characters to see if it
    /// has a prefix or not
    pub fn new(str: &str) -> Self {
        match str.get(0..2) {
            Some("0x") => NumericState::Hex,
            Some("0X") => NumericState::Hex,
            Some("0b") => NumericState::Binary,
            Some("0B") => NumericState::Binary,
            Some("0o") => NumericState::Octal,
            Some("0O") => NumericState::Octal,
            _ => NumericState::Decimal,
        }
    }

    /// If this `NumericState` has a prefix to identify itself within a string
    pub fn has_prefix(&self) -> bool {
        !matches!(self, NumericState::Decimal)
    }

    /// Attempt to parse the number into a `BigUint` based on the `NumericState`
    pub fn parse(&self, str: &str) -> Result<BigUint, ParseBigIntError> {
        BigUint::from_str_radix(
            str,
            match self {
                NumericState::Decimal => 10,
                NumericState::Hex => 16,
                NumericState::Octal => 8,
                NumericState::Binary => 2,
            },
        )
    }

    /// A short hand function, combining both `new()` and `parse()` into one
    pub fn immediate_parse(str: &str) -> Result<BigUint, ParseBigIntError> {
        let state = Self::new(str);
        let non_prefix = match state.has_prefix() {
            true => str.get(2..).unwrap_or_default(),
            false => str,
        };

        state.parse(non_prefix)
    }
}

/// General predicate page
///
/// Displays the trigger selection and trigger operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GeneralPage;

impl PredicatePage for GeneralPage {
    fn render(&self, state: &mut State, f: &mut Frame<'_>, area: Rect) {
        let layout = Layout::new(
            ratatui::layout::Direction::Vertical,
            [
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Fill(0),
            ],
        )
        .flex(ratatui::layout::Flex::Start)
        .spacing(0)
        .split(area);

        f.render_widget("General predicate settings".bold(), layout[0]);
        f.render_widget(
            match state.target {
                PredicateTarget::Trigger => "The general predicates page for trigger configuration",
                PredicateTarget::Capture => "The general predicates page for capture configuration",
            },
            layout[1],
        );

        f.render_widget("Predicate operation", layout[2]);
        state
            .predicate_op_ui_state
            .render_ref(layout[3], f.buffer_mut());
        f.render_widget("Predicate Selector", layout[4]);
        state
            .predicate_select_ui_state
            .render_ref(layout[5], f.buffer_mut());
    }

    fn navigate(&self, state: &mut State, event: &Event) {
        let consumed = state.predicate_op_ui_state.handle_input(event)
            || state.predicate_select_ui_state.handle_input(event);

        if consumed {
            return;
        };

        #[derive(Debug, Clone, Copy)]
        enum Direction {
            Up,
            Down,
        }

        let direction = match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => Direction::Up,
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => Direction::Down,
            _ => return,
        };

        state.selected = match (&state.selected, direction) {
            (Selected::Operation, Direction::Down) => Selected::Select,
            (Selected::Operation, Direction::Up) => Selected::Operation,
            (Selected::Select, Direction::Up) => Selected::Operation,
            (Selected::Select, Direction::Down) => Selected::Select,
        };

        match &state.selected {
            Selected::Operation => {
                state.predicate_op_ui_state.set_focus(true);
                state.predicate_op_ui_state.set_hover(usize::MAX);
            }
            Selected::Select => {
                state.predicate_select_ui_state.set_focus(true);
                state.predicate_select_ui_state.set_hover(0);
            }
        }
    }
}

/// Predicate mask and compare page
///
/// Due to both looking similar and sharing a lot of functionality, they are merged into a single
/// page but with a bool to distinguish the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaskComparePage {
    is_mask: bool,
}

impl MaskComparePage {
    fn get_input_states<'a>(&self, state: &'a mut State) -> &'a mut Vec<TextPromptState<()>> {
        match self.is_mask {
            true => &mut state.mask_state,
            false => &mut state.compare_state,
        }
    }
}

impl PredicatePage for MaskComparePage {
    fn render(&self, state: &mut State, f: &mut Frame<'_>, area: Rect) {
        let layout = Layout::new(
            ratatui::layout::Direction::Vertical,
            [
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Fill(0),
            ],
        )
        .flex(Flex::Start)
        .split(area);

        let title = match self.is_mask {
            true => "Predicate mask",
            false => "Predicate compare value",
        };
        let description = match self.is_mask {
            true => "Filter bits before data is given to the predicate, allows you to ignore bits you don't care about",
            false => "The value given to the predicate to compare the incoming data to",
        };

        f.render_widget(title.bold(), layout[0]);
        f.render_widget(description, layout[1]);

        let signals = state.signals;
        let is_valid =
            self.get_input_states(state)
                .iter()
                .zip(signals)
                .all(|(input_state, signal)| {
                    NumericState::immediate_parse(&input_state.input)
                        .is_ok_and(|uint| uint < BigUint::new(vec![2]).pow(signal.width as u32))
                });
        let is_valid_label = match is_valid {
            true => "Provided input is valid".green(),
            false => "Provided input is INVALID".red(),
        };

        f.render_widget(is_valid_label, layout[2]);

        let base_area = layout[3];
        for (index, signal) in state.signals.iter().enumerate() {
            let element_active = index == state.mask_compare_cursor_position;

            let mut max_bound = BigInt::new(Sign::Plus, vec![2]).pow(signal.width as u32);
            max_bound += BigInt::new(Sign::Minus, vec![1]);

            let signal_label = format!(" {} - input range [0 - {:#x}] ", signal.name, max_bound);

            let borders = Block::new().title(signal_label).borders(Borders::ALL);

            let y_pos = base_area.y + index as u16 * 3;
            let border_area = Rect::new(base_area.x, y_pos, base_area.width, 3);
            let input_area = Rect::new(base_area.x + 2, y_pos + 1, base_area.width - 2, 1);
            let input_select = Rect::new(base_area.x, y_pos + 1, 2, 1);

            let input_select_text = if index == state.mask_compare_cursor_position {
                "> "
            } else {
                ""
            };

            f.render_widget(borders, border_area);
            f.render_widget(input_select_text, input_select);

            self.get_input_states(state)[index].render(input_area, f, element_active);
        }
    }

    fn navigate(&self, state: &mut State, event: &Event) {
        state.mask_compare_cursor_position = match event {
            Event::Key(KeyEvent {
                code: KeyCode::Up, ..
            }) => state.mask_compare_cursor_position.saturating_sub(1),
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                ..
            }) => {
                (state.mask_compare_cursor_position + 1).min(self.get_input_states(state).len() - 1)
            }
            _ => state.mask_compare_cursor_position,
        };

        let Event::Key(KeyEvent {
            code: key_event, ..
        }) = event
        else {
            return;
        };

        let index = state.mask_compare_cursor_position;
        let input_states = self.get_input_states(state);
        let input_state = &mut input_states[index];

        input_state.handle_input(*key_event);
    }
}

/// The different tabs for the predicate UI
#[derive(Debug, Clone, PartialEq)]
enum Tab {
    General(GeneralPage),
    MaskCompare(MaskComparePage),
}

impl Tab {
    /// The names for each tab
    fn names() -> [&'static str; 3] {
        ["General", "Mask", "Compare"]
    }

    /// The index of the current tab
    fn index(&self) -> usize {
        match self {
            Tab::General(_) => 0,
            Tab::MaskCompare(MaskComparePage { is_mask: true }) => 1,
            Tab::MaskCompare(MaskComparePage { is_mask: false }) => 2,
        }
    }

    /// Go to the previous tab
    fn prev(&self) -> Tab {
        match self {
            Tab::General(_) => Tab::MaskCompare(MaskComparePage { is_mask: false }),
            Tab::MaskCompare(MaskComparePage { is_mask: true }) => Tab::General(GeneralPage),
            Tab::MaskCompare(MaskComparePage { is_mask: false }) => {
                Tab::MaskCompare(MaskComparePage { is_mask: true })
            }
        }
    }

    /// Go to the next tab
    fn next(&self) -> Tab {
        match self {
            Tab::General(_) => Tab::MaskCompare(MaskComparePage { is_mask: true }),
            Tab::MaskCompare(MaskComparePage { is_mask: true }) => {
                Tab::MaskCompare(MaskComparePage { is_mask: false })
            }
            Tab::MaskCompare(MaskComparePage { is_mask: false }) => Tab::General(GeneralPage),
        }
    }

    /// Render the current tab
    fn render(&self, state: &mut State, f: &mut Frame<'_>, area: Rect) {
        match self {
            Tab::General(page) => page.render(state, f, area),
            Tab::MaskCompare(page) => page.render(state, f, area),
        }
    }
}

/// The response of the predicate UI on input events
#[derive(Debug, Clone)]
pub enum PredicateEventResponse {
    /// Close the program
    QuitProgram,
    /// Return to the main menu and display a message in the log
    /// The bool indicates wether or not changes to the ILA configuration have been made
    MainMenu((String, bool)),
    /// Do nothing, remain in the predicate UI
    Nothing,
}

/// What option is selected in the general predicate page
#[derive(Debug, Clone)]
pub enum Selected {
    Operation,
    Select,
}

/// The state of the predicate UI
#[derive(Debug, Clone)]
pub struct State<'a> {
    /// The current tab
    tab: Tab,
    /// The current option selected in the general page
    selected: Selected,
    /// The predicate target
    target: PredicateTarget,
    /// The state for the predicate operation
    predicate_op_ui_state: Listbox,
    /// The state for the predicate select
    predicate_select_ui_state: Checkbox,
    /// Metadata over the signals the ILA is expected to process
    signals: &'a [IlaSignal],
    /// Which element is selected on the mask/compare tabs
    mask_compare_cursor_position: usize,
    /// The mask tab text input states
    mask_state: Vec<TextPromptState<()>>,
    /// The compare tab text input states
    compare_state: Vec<TextPromptState<()>>,
}

impl State<'_> {
    /// Create a new predicates configuration interface
    ///
    /// An initial `IlaPredicate` configuration has to be provided to set the initial values within
    /// the UI. This UI will take up the entire screen
    pub fn new(ila: &IlaConfig, predicate: IlaPredicate) -> State {

        /// Create the text prompts from a `SignalCluster`
        fn signals_to_prompts(data: SignalCluster) -> Vec<TextPromptState<()>> {
            data.cluster
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
                .collect()
        }

        State {
            tab: Tab::General(GeneralPage),
            selected: Selected::Operation,
            target: predicate.target,
            predicate_op_ui_state: {
                let mut list = Listbox::new(vec!["AND", "OR"], predicate.operation as usize);
                list.set_focus(true);
                list
            },
            predicate_select_ui_state: {
                let mut checkbox = Checkbox::new(ila.trigger_names.clone());
                for (index, bit) in predicate
                    .predicate_select
                    .view_bits::<Lsb0>()
                    .iter()
                    .enumerate()
                {
                    let _ = checkbox.set_marked(index, *bit);
                }
                checkbox
            },
            signals: &ila.signals,
            mask_compare_cursor_position: 0,
            mask_state: signals_to_prompts(predicate.mask),
            compare_state: signals_to_prompts(predicate.compare),
        }
    }

    /// Render the predicate UI
    pub fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
        let _ = terminal.draw(|f| {
            let layout = Layout::new(
                ratatui::layout::Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Fill(0),
                    Constraint::Length(5),
                ],
            )
            .flex(Flex::Start)
            .split(f.area());

            let tabs = Tabs::new(Tab::names()).select(self.tab.index());
            f.render_widget(tabs, layout[0]);

            f.render_widget(Block::new().borders(Borders::ALL), layout[1]);

            self.tab
                .clone() // Pesky borrow
                .render(self, f, layout[1].inner(Margin::new(1, 1)));

            f.render_widget(
                Paragraph::new(HELP_MESSAGE).block(Block::bordered().title("keybinds")),
                layout[2],
            );
        });
    }

    /// Move the predicate UI to the next tab
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
    }

    /// Move the predicate UI to the previous tab
    pub fn prev_tab(&mut self) {
        self.tab = self.tab.prev();
    }

    /// Handle input for the selected tab
    ///
    /// This shouldn't be invoked manually as it ignores 'global' keybinds, use `handle_event()`
    /// to feed input to the predicate UI
    fn manage_page_navigation(&mut self, event: &Event) {
        match self.tab {
            Tab::General(page) => page.navigate(self, event),
            Tab::MaskCompare(page) => page.navigate(self, event),
        }
    }

    /// Handle input for the predicate UI
    pub fn handle_event<T>(
        &mut self,
        medium: &mut T,
        ila: &IlaConfig,
        event: &Event,
    ) -> PredicateEventResponse
    where
        T: Read + Write,
    {
        // Handle 'global' keybinds
        let event_consumed = match event {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                return PredicateEventResponse::QuitProgram;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                return PredicateEventResponse::MainMenu(("Cancelled changes".into(), false));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
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

                let compare: Vec<Signal> = self
                    .compare_state
                    .iter()
                    .zip(ila.signals.iter())
                    .filter_map(|(input_state, signal)| {
                        NumericState::immediate_parse(&input_state.input)
                            .ok()
                            .map(|n| (n, signal))
                    })
                    .filter(|(n, signal)| *n < BigUint::new(vec![2]).pow(signal.width as u32))
                    .map(biguint_to_signal)
                    .collect();
                let mask: Vec<Signal> = self
                    .mask_state
                    .iter()
                    .zip(ila.signals.iter())
                    .filter_map(|(input_state, signal)| {
                        NumericState::immediate_parse(&input_state.input)
                            .ok()
                            .map(|n| (n, signal))
                    })
                    .filter(|(n, signal)| *n < BigUint::new(vec![2]).pow(signal.width as u32))
                    .map(biguint_to_signal)
                    .collect();

                // Make sure our inputs are valid
                if compare.len() != self.compare_state.len() || mask.len() != self.mask_state.len()
                {
                    return PredicateEventResponse::Nothing;
                }

                let mut selected = 0;
                for mark in self.predicate_select_ui_state.get_all_marked().iter().rev() {
                    selected = (selected << 1) | (*mark as u32);
                }

                let Ok(operation) =
                    PredicateOperation::try_from(self.predicate_op_ui_state.get_selected() as u32)
                else {
                    return PredicateEventResponse::Nothing;
                };

                let predicate = IlaPredicate {
                    target: self.target,
                    operation,
                    predicate_select: selected,
                    mask: SignalCluster {
                        cluster: mask,
                        timestamp: Duration::ZERO,
                    },
                    compare: SignalCluster {
                        cluster: compare,
                        timestamp: Duration::ZERO,
                    },
                };

                let response = match predicate.update_ila(medium, ila) {
                    Ok(_) => ("Succesfully updated predicate state".into(), true),
                    Err(err) => (format!("Failed to update ILA with error {err}"), false),
                };

                return PredicateEventResponse::MainMenu(response);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Left,
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                self.prev_tab();
                true
            }
            Event::Key(KeyEvent {
                code: KeyCode::Right,
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                self.next_tab();
                true
            }
            _ => false,
        };

        if event_consumed {
            return PredicateEventResponse::Nothing;
        }

        // Give the event to the underlying tab
        self.manage_page_navigation(event);

        PredicateEventResponse::Nothing
    }
}


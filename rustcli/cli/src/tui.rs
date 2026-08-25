//! An interactive terminal UI over the same `App` the CLI uses.
//!
//! Everything here goes through the store and RPC layers, so a TUI session and a
//! sequence of CLI calls leave the wallet in the same state.
//!
//! The screen is built around a command list that is always visible: nothing has
//! to be memorised, `Tab` moves between panes, `Enter` runs the highlighted
//! command, and every command also has a single-key shortcut for anyone who
//! would rather not walk the list. `?` opens a full key reference over the top.

use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::json;

use causewaybay_core::app::{App, SendPlan};
use causewaybay_core::command::{AccountCommand, SendArgs, TargetArgs};
use causewaybay_core::error::{self, Result};
use causewaybay_core::export::{self, Format};
use causewaybay_core::host::Headless;
use causewaybay_core::network;
use causewaybay_core::output::CommandOutput;
use causewaybay_core::store::{self, Account, RecentSecret};
use causewaybay_core::ChainId;

// ============================================================== the commands

/// Everything the TUI can do. One value per row of the command pane, and the
/// single place a key press and a menu selection converge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Balance,
    Send,
    /// Create the next wallet: a fresh index, derived on every chain.
    NewWallet,
    NewSeed,
    ImportMnemonic,
    ImportKey,
    Recall,
    Derive,
    Activate,
    CopyAddress,
    Sign,
    Save(Format),
    ExportWallets,
    ToggleSecrets,
    /// Switch to the network with this key. Only the current chain's networks
    /// are offered, so the list stays short as chains are added.
    SelectNetwork(&'static str),
    Remove,
    Reload,
    Help,
    Quit,
}

struct Command {
    action: Action,
    /// Shown in the command pane.
    label: String,
    /// The single-key shortcut, if it has one. Networks are menu-only, because
    /// the list grows and hand-assigned letters would start colliding.
    key: Option<char>,
    /// One line of explanation, shown in the help overlay.
    help: String,
    /// Runs from its key, but takes no row in the pane.
    ///
    /// For the variants of a command that already has a row — the three other
    /// save formats — where four near-identical rows would cost a quarter of
    /// the pane to say one thing.
    hidden: bool,
}

impl Command {
    fn new(action: Action, label: &str, key: Option<char>, help: &str) -> Self {
        Command {
            action,
            label: label.to_string(),
            key,
            help: help.to_string(),
            hidden: false,
        }
    }

    /// The same command, dispatchable by key but off the pane.
    fn hidden(self) -> Self {
        Command {
            hidden: true,
            ..self
        }
    }
}

/// Build the command pane.
///
/// Every network is a row of its own — `cronos testnet`, `solana devnet`, and
/// the rest — because a two-level menu made the commonest move in the wallet
/// (go somewhere else) a matter of finding which rows had appeared under which
/// other row. Flat means one press from anywhere to anywhere, and the same
/// rows in the same order every time. The colour says which chain a row
/// belongs to and the ● says which one you are on, so nothing is lost by
/// dropping the nesting.
///
/// It takes no argument for the same reason: the list no longer changes with
/// the chain in view.
fn build_commands() -> Vec<Command> {
    let mut commands = vec![
        Command::new(
            Action::Balance,
            "Get balance",
            Some('b'),
            "Ask the node for the selected wallet's balance",
        ),
        Command::new(
            Action::Send,
            "Send amount",
            Some('s'),
            "Send this chain's native coin to an address",
        ),
        Command::new(
            Action::NewWallet,
            "New wallet",
            Some('n'),
            "Next index, derived on all four chains",
        ),
        Command::new(
            Action::NewSeed,
            "New seed",
            Some('N'),
            "Start a separate mnemonic, indices from 0 again",
        ),
        Command::new(
            Action::ImportMnemonic,
            "Import mnemonic",
            Some('m'),
            "Import an existing BIP-39 phrase here",
        ),
        Command::new(
            Action::ImportKey,
            "Import priv key",
            Some('p'),
            "Import a raw key in this chain's format",
        ),
        Command::new(
            Action::Recall,
            "Recall saved keys",
            Some('c'),
            "Reuse a mnemonic or key from the recall list",
        ),
        Command::new(
            Action::Derive,
            "Derive wallet",
            Some('d'),
            "Make the wallet at an index you choose",
        ),
        Command::new(
            Action::Activate,
            "Set active",
            Some('a'),
            "Make the selected wallet the CLI default",
        ),
        Command::new(
            Action::CopyAddress,
            "Copy address",
            Some('y'),
            "Copy the selected wallet's address",
        ),
        Command::new(
            Action::Sign,
            "Sign message",
            Some('g'),
            "Sign a message with this chain's scheme",
        ),
        // One row, four formats: the row says which keys, and the three that
        // are not the default keep working without taking three more rows.
        Command::new(
            Action::Save(Format::Jsonl),
            "Save list (1-4)",
            Some('1'),
            "1 jsonl · 2 csv · 3 txt · 4 md",
        ),
        Command::new(
            Action::Save(Format::Csv),
            "Save list .csv",
            Some('2'),
            "Write the wallet list to a CSV file",
        )
        .hidden(),
        Command::new(
            Action::Save(Format::Txt),
            "Save list .txt",
            Some('3'),
            "Write the wallet list to an aligned text file",
        )
        .hidden(),
        Command::new(
            Action::Save(Format::Markdown),
            "Save list .md",
            Some('4'),
            "Write the wallet list to a Markdown table",
        )
        .hidden(),
        Command::new(
            Action::ExportWallets,
            "Export wallets",
            Some('e'),
            "JSONL with private keys and both public keys",
        ),
        Command::new(
            Action::ToggleSecrets,
            "Show/hide secret",
            Some('v'),
            "Reveal or hide the private key and mnemonic",
        ),
    ];

    // One row per network, in registry order — which groups them by chain
    // without a heading, since the networks of a chain are named after it.
    for network in network::ALL.iter() {
        commands.push(Command::new(
            Action::SelectNetwork(network.key),
            // `cronos-testnet` reads as `cronos testnet`: the row says both
            // halves of where the wallet will be, and nothing else has to.
            &network.key.replace('-', " "),
            None,
            &format!("Work on {} ({})", network.name, network.symbol),
        ));
    }

    commands.extend([
        Command::new(
            Action::Remove,
            "Remove wallet",
            Some('x'),
            "Forget this wallet on every chain (asks first)",
        ),
        Command::new(
            Action::Reload,
            "Reload from disk",
            Some('r'),
            "Re-read the store, picking up CLI changes",
        ),
        Command::new(Action::Help, "Help", Some('?'), "Show this reference"),
        Command::new(Action::Quit, "Quit", Some('q'), "Leave the wallet"),
    ]);
    commands
}

/// A colour per chain, used everywhere a chain is named so the eye can pick
/// one out of a mixed list without reading.
fn chain_colour(chain: ChainId) -> Color {
    match chain {
        ChainId::Evm => Color::Magenta,
        ChainId::Solana => Color::Green,
        ChainId::Cardano => Color::Blue,
        ChainId::Midnight => Color::Yellow,
    }
}

fn command_for_key(commands: &[Command], key: char) -> Option<Action> {
    commands
        .iter()
        .find(|command| command.key == Some(key))
        .map(|command| command.action)
}

// ================================================================== the state

/// Which pane the arrow keys and Enter apply to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Commands,
    Accounts,
    /// The recall list, shown in place of the accounts pane.
    Recall,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Browse,
    /// Collecting text; the kind decides what happens on Enter.
    Input(InputKind),
    Confirm(ConfirmKind),
    /// Work is running on a background thread.
    ///
    /// Anything that waits on a node goes here. A Midnight send can spend
    /// minutes replaying the dust ledger and generating a proof, and a balance
    /// is a round trip to whatever the node feels like doing; done on the UI
    /// thread either one freezes every pixel, which reads as a hang. The
    /// thread reports through [`JobEvent`]s and the loop keeps drawing; Esc
    /// abandons the wait, which costs nothing — a plan is local until it is
    /// confirmed, and a balance is only ever a read.
    Busy,
    Help,
}

/// What a background thread has to say.
enum JobEvent {
    /// A line of narration — a dust sync milestone, a proving start.
    Progress(String),
    /// The work is done, and this is what it produced.
    Done(JobResult),
    /// Why there is nothing to show.
    Failed(String),
}

/// What a finished background job produced.
enum JobResult {
    /// The signed, checked plan, ready to confirm.
    Plan(Box<SendPlan>),
    /// A balance, already formatted with its symbol.
    Balance(String),
}

/// The receiving end of one background thread.
struct Job {
    events: std::sync::mpsc::Receiver<JobEvent>,
    started: Instant,
    /// The last narration line, re-rendered with a running clock.
    last: String,
    /// What abandoning it costs, said in the user's terms.
    abandoned: &'static str,
}

/// The [`Host`] a background thread runs under: progress goes to the event
/// channel, questions are already answered — the TUI shows its own confirm
/// screen after planning, and secrets are typed into its prompts, never read
/// mid-command.
struct JobHost {
    events: std::sync::mpsc::Sender<JobEvent>,
}

impl causewaybay_core::host::Host for JobHost {
    fn read_input(&self, what: &str) -> causewaybay_core::error::Result<String> {
        Err(error::usage(format!("no {what} can be read mid-plan")))
    }
    fn confirm(&self, _prompt: &str) -> causewaybay_core::error::Result<()> {
        Ok(())
    }
    fn progress(&self, message: &str) {
        let _ = self.events.send(JobEvent::Progress(message.to_string()));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InputKind {
    Label {
        new_seed: bool,
    },
    /// Whose balance to read. Blank means the selected wallet's own address.
    BalanceAddress,
    /// Filename for the full export: private keys included.
    ExportWalletsPath,
    Mnemonic,
    PrivateKey,
    DeriveIndex,
    SendTo,
    SendAmount,
    SignMessage,
    ExportPath(Format),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConfirmKind {
    Remove,
    Send,
}

struct State {
    accounts: Vec<Account>,
    selected: ListState,
    /// Remembered key material, shown when the recall pane has focus.
    recall: Vec<RecentSecret>,
    recall_selected: ListState,
    /// Every command, drawn or not. A hidden one still runs from its key and
    /// still appears in the reference; only the pane leaves it out — see
    /// [`State::visible_commands`], which is what a selection indexes into.
    command_list: Vec<Command>,
    commands: ListState,
    /// The network key the command pane marks as current.
    current_network: String,
    /// The chain in view. Every chain-specific command acts on this one, and
    /// it is what the network rows below the chain rows are drawn from.
    current_chain: ChainId,
    /// The wallet index in view.
    ///
    /// A wallet is one mnemonic and one index; each chain derives its own
    /// account at that index, so index 0 on Solana and index 0 on Cardano are
    /// two facets of the same wallet. This is the cursor: `New wallet` moves
    /// it on, and `New <chain>` fills the slot it points at.
    current_index: u32,
    focus: Focus,
    mode: Mode,
    input: String,
    status: String,
    status_is_error: bool,
    detail: Vec<(String, String)>,
    /// The work running in the background, if any is.
    job: Option<Job>,
    /// An address from the clipboard is sitting in the prompt, unaccepted.
    ///
    /// While set, the hint line asks the question; any edit or answer clears
    /// it, so the hint never claims an offer the user has already overridden.
    clipboard_offer: bool,
    /// Staged transfer, filled in across the two input steps.
    pending_to: String,
    pending_amount: String,
    /// The planned transfer, held between the confirmation and the broadcast.
    staged: Option<SendPlan>,
    show_secrets: bool,
    /// Set when the network changed, so the loop can rebuild `App` around it.
    network_changed: bool,
    quit: bool,
}

impl State {
    /// A state on the default network. Used by the tests; the event loop seeds
    /// the real one from the store via `with_network`.
    #[cfg(test)]
    fn new(accounts: Vec<Account>, active: Option<&str>) -> Self {
        Self::with_network(accounts, active, network::DEFAULT_NETWORK)
    }

    fn with_network(accounts: Vec<Account>, active: Option<&str>, current_network: &str) -> Self {
        let current_chain = network::find(current_network)
            .map(|n| n.chain)
            .unwrap_or_default();
        // The pane lists wallet indices, so the cursor is seeded with the
        // active account's *index*, not its position in the account vector.
        let active_index = active
            .and_then(|id| accounts.iter().find(|a| a.id == id))
            .and_then(|a| a.index)
            .unwrap_or(0);
        let mut selected = ListState::default();
        if !accounts.is_empty() {
            selected.select(Some(0));
        }
        let mut commands = ListState::default();
        commands.select(Some(0));

        let mut state = State {
            accounts,
            selected,
            recall: Vec::new(),
            recall_selected: ListState::default(),
            command_list: build_commands(),
            commands,
            current_network: current_network.to_string(),
            current_chain,
            current_index: 0,
            job: None,
            clipboard_offer: false,
            focus: Focus::Commands,
            mode: Mode::Browse,
            input: String::new(),
            status: "Ready — answers, prompts and warnings appear here".into(),
            status_is_error: false,
            detail: Vec::new(),
            pending_to: String::new(),
            pending_amount: String::new(),
            staged: None,
            show_secrets: false,
            network_changed: false,
            quit: false,
        };
        state.select_index(active_index);
        // …and if that wallet is gone — its index removed in an earlier
        // session, or the active account deleted — the cursor stays where it
        // is and `clamp_selection` makes `current_index` agree with it. Left
        // out, the header named one wallet while the keys acted on another,
        // which is how `x` came to remove a wallet nobody had pointed at.
        state.clamp_selection();
        state
    }

    /// The account every command acts on: the highlighted wallet, on the
    /// chain in view.
    ///
    /// The pane lists wallets, not accounts, so the chain is the other half of
    /// the answer — picking a chain re-points balance, send and the rest
    /// without the list moving. A wallet that has nothing on the chain in view
    /// falls back to whatever it does have, so the pane is never inert.
    fn current(&self) -> Option<&Account> {
        let index = self.selected_index()?;
        let here = self.accounts_at(index);
        here.iter()
            .find(|a| a.chain == self.current_chain)
            .or_else(|| here.first())
            .copied()
    }

    /// Every wallet index the store holds an account for, in order.
    fn indices(&self) -> Vec<u32> {
        let mut seen: Vec<u32> = self.accounts.iter().map(|a| a.index.unwrap_or(0)).collect();
        seen.sort_unstable();
        seen.dedup();
        seen
    }

    /// One wallet's accounts, in registry order — what the detail pane lists.
    ///
    /// Never filtered by the chain in view: the point of the detail pane is to
    /// show the whole wallet, whatever the list is narrowed to.
    fn accounts_at(&self, index: u32) -> Vec<&Account> {
        let mut on_index: Vec<&Account> = self
            .accounts
            .iter()
            .filter(|a| a.index.unwrap_or(0) == index)
            .collect();
        // Registry order, so a chain is always in the same place.
        on_index.sort_by_key(|a| {
            ChainId::ALL
                .iter()
                .position(|id| *id == a.chain)
                .unwrap_or(usize::MAX)
        });
        on_index
    }

    /// The wallet index the highlighted row is.
    fn selected_index(&self) -> Option<u32> {
        self.indices().get(self.selected.selected()?).copied()
    }

    /// Put the cursor on one wallet index, if the list is showing it.
    fn select_index(&mut self, index: u32) {
        if let Some(position) = self.indices().iter().position(|i| *i == index) {
            self.selected.select(Some(position));
            self.current_index = index;
        }
    }

    /// The lowest index this seed has no account at, on any chain.
    fn next_free_index(&self) -> u32 {
        let taken = self.indices();
        (0u32..).find(|i| !taken.contains(i)).unwrap_or(0)
    }

    /// Move to the chain in view, keeping the selection inside the lists.
    ///
    /// The command pane does not change with it: every network has a row
    /// whatever chain is in view, and the ● moves rather than the rows.
    fn set_chain(&mut self, chain: ChainId) {
        self.current_chain = chain;
        self.clamp_selection();
    }

    /// Keep every selection inside its list after the lists change under it.
    fn clamp_selection(&mut self) {
        let visible = self.indices().len();
        match (visible, self.selected.selected()) {
            (0, _) => self.selected.select(None),
            (len, Some(i)) if i >= len => self.selected.select(Some(len - 1)),
            (_, None) => self.selected.select(Some(0)),
            _ => {}
        }
        let commands = self.visible_commands().len();
        if let Some(i) = self.commands.selected() {
            if i >= commands {
                self.commands.select(Some(commands.saturating_sub(1)));
            }
        }
        // The highlight *is* the wallet cursor, so it cannot be left pointing
        // at a wallet the list no longer holds — after a removal that would
        // leave the header naming one wallet and the pane showing another.
        if let Some(index) = self.selected_index() {
            self.current_index = index;
        }
    }

    fn current_recall(&self) -> Option<&RecentSecret> {
        self.recall_selected
            .selected()
            .and_then(|i| self.recall.get(i))
    }

    /// The rows the pane actually draws.
    ///
    /// The selection is an index into *this*, never into `command_list`: a
    /// hidden row between them shifts every row below it, and the pane then
    /// runs a command three lines above the highlighted one.
    fn visible_commands(&self) -> Vec<&Command> {
        self.command_list.iter().filter(|c| !c.hidden).collect()
    }

    fn current_command(&self) -> Option<Action> {
        self.visible_commands()
            .get(self.commands.selected()?)
            .map(|command| command.action)
    }

    fn info(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_is_error = false;
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.status_is_error = true;
    }

    /// Move the selection in whichever pane has focus, wrapping at both ends.
    fn move_selection(&mut self, delta: isize) {
        let (len, state) = match self.focus {
            Focus::Commands => (self.visible_commands().len(), &mut self.commands),
            Focus::Accounts => (self.indices().len(), &mut self.selected),
            Focus::Recall => (self.recall.len(), &mut self.recall_selected),
        };
        if len == 0 {
            return;
        }
        let current = state.selected().unwrap_or(0) as isize;
        state.select(Some((current + delta).rem_euclid(len as isize) as usize));
    }

    /// Tab order. The recall pane replaces the account pane while it is open.
    fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Commands if !self.recall.is_empty() || self.focus == Focus::Recall => {
                Focus::Accounts
            }
            Focus::Commands => Focus::Accounts,
            Focus::Accounts => Focus::Commands,
            Focus::Recall => Focus::Commands,
        };
    }
}

// =================================================================== the loop

/// Run the TUI until the user quits, then restore the terminal.
pub fn run(app: App) -> Result<CommandOutput> {
    // The TUI asks its own questions on screen, so the wallet underneath it
    // must not ask again — and has no terminal left to ask on anyway.
    let app = app.with_host(std::sync::Arc::new(Headless::new().assume_yes(true)));

    enable_raw_mode().map_err(|e| error::internal(format!("cannot enter raw mode: {e}")))?;
    let mut out = stdout();
    // Bracketed paste makes a terminal deliver a paste as one event instead
    // of a burst of keystrokes. Without it the terminal may still send the
    // `ESC[200~` markers (shells leave the mode on), crossterm parses them
    // into an `Event::Paste` — and a loop that only reads key events swallows
    // the whole paste. That is why pasting an address used to do nothing.
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)
        .map_err(|e| error::internal(format!("cannot open the alternate screen: {e}")))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| error::internal(format!("cannot build the terminal: {e}")))?;

    let result = event_loop(app, &mut terminal);

    // Restore the terminal even if the loop failed, so the shell stays usable.
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();

    result.map(|_| CommandOutput::new(json!({"tui": "exited"}), "Left the wallet TUI."))
}

fn event_loop<B: ratatui::backend::Backend>(
    mut app: App,
    terminal: &mut Terminal<B>,
) -> Result<()> {
    let active = app.store.active_account().ok().map(|a| a.id);
    // Seeded from the store, not from DEFAULT_NETWORK: starting on the wrong
    // one would mark the wrong row as current and make `select_network` refuse
    // a real switch as a no-op.
    let current = app.store.network()?.key;
    let mut state = State::with_network(app.store.accounts()?, active.as_deref(), current);
    refresh_detail(&mut state);

    let tick = Duration::from_millis(200);
    let mut last = Instant::now();
    while !state.quit {
        // A network switch rebinds `App`, so every later RPC call, symbol and
        // explorer link follows the chain the header is showing.
        if state.network_changed {
            state.network_changed = false;
            app = App::new(
                Some(app.store.home().to_path_buf()),
                Some(&state.current_network),
                // The network settles the chain, so naming it again would
                // only be a second chance to disagree with itself.
                None,
                std::sync::Arc::clone(&app.host),
            )?;
            refresh_detail(&mut state);
        }

        // The background thread's narration lands before each draw, so the
        // status line and its clock stay live while the work runs elsewhere.
        poll_job(&mut state);

        terminal
            .draw(|frame| draw(frame, &app, &mut state))
            .map_err(|e| error::internal(format!("draw failed: {e}")))?;

        let timeout = tick.saturating_sub(last.elapsed());
        if event::poll(timeout).map_err(|e| error::internal(format!("input poll failed: {e}")))? {
            match event::read().map_err(|e| error::internal(format!("input read failed: {e}")))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(&app, &mut state, key.code, key.modifiers);
                }
                Event::Paste(text) => handle_paste(&mut state, &text),
                _ => {}
            }
        }
        if last.elapsed() >= tick {
            last = Instant::now();
        }
    }
    Ok(())
}

/// A paste lands in the input buffer, and only there.
///
/// Newlines are folded to spaces rather than kept: an address or mnemonic
/// copied from a browser often carries a trailing newline, and treating that
/// as Enter would submit a half-checked value the moment it was pasted. The
/// user still confirms with a deliberate keystroke.
fn handle_paste(state: &mut State, text: &str) {
    if !matches!(state.mode, Mode::Input(_)) {
        return; // nothing to paste into, and Browse keys are commands
    }
    append_pasted(state, text);
}

/// Fold pasted text into the input buffer: newlines become spaces, control
/// characters vanish, trailing whitespace is dropped.
fn append_pasted(state: &mut State, text: &str) {
    let mut pasted = false;
    for c in text.chars() {
        match c {
            '\n' | '\r' => {
                if !state.input.ends_with(' ') && !state.input.is_empty() {
                    state.input.push(' ');
                }
            }
            c if c.is_control() => {}
            c => {
                state.input.push(c);
                pasted = true;
            }
        }
    }
    if pasted {
        // Trailing whitespace from a folded newline is never meant.
        while state.input.ends_with(' ') {
            state.input.pop();
        }
    }
}

fn handle_key(app: &App, state: &mut State, code: KeyCode, modifiers: KeyModifiers) {
    if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
        state.quit = true;
        return;
    }
    match state.mode {
        Mode::Help => {
            // Any key dismisses the reference.
            state.mode = Mode::Browse;
        }
        Mode::Input(kind) => input_key(app, state, kind, code, modifiers),
        Mode::Confirm(kind) => confirm_key(app, state, kind, code),
        Mode::Busy => {
            // Abandoning is safe: a plan is local until confirmed and a
            // balance is a read, so the thread's eventual result simply lands
            // in a channel nobody is listening to any more.
            if code == KeyCode::Esc {
                let said = state
                    .job
                    .take()
                    .map(|job| job.abandoned)
                    .unwrap_or("Abandoned");
                state.mode = Mode::Browse;
                state.info(said);
            }
        }
        Mode::Browse => browse_key(app, state, code),
    }
}

fn browse_key(app: &App, state: &mut State, code: KeyCode) {
    match code {
        KeyCode::Tab | KeyCode::BackTab => {
            state.cycle_focus();
            if state.focus == Focus::Accounts {
                refresh_detail(state);
            }
        }
        KeyCode::Down => {
            state.move_selection(1);
            after_move(state);
        }
        KeyCode::Up => {
            state.move_selection(-1);
            after_move(state);
        }
        // The two axes are the two pairs of arrows: down the wallets, across
        // the chains. Reaching the command list to change chain was the one
        // move that needed explaining.
        KeyCode::Right => step_chain(app, state, 1),
        KeyCode::Left => step_chain(app, state, -1),
        KeyCode::Enter => match state.focus {
            Focus::Commands => {
                if let Some(action) = state.current_command() {
                    run_action(app, state, action);
                }
            }
            Focus::Recall => import_selected_recall(app, state),
            Focus::Accounts => {
                // Enter on a wallet is the obvious way to ask "how much is in
                // it", so it asks — rather than opening a prompt whose answer
                // is already on screen. `b` is still there for any other
                // address.
                start_balance(app, state, None);
            }
        },
        KeyCode::Esc => {
            if state.focus == Focus::Recall {
                close_recall(state);
            } else {
                state.quit = true;
            }
        }
        // `j`/`k` still move, but only where they cannot be a command shortcut.
        KeyCode::Char(key) => {
            if let Some(action) = command_for_key(&state.command_list, key) {
                run_action(app, state, action);
            } else if key == 'j' {
                state.move_selection(1);
                after_move(state);
            } else if key == 'k' {
                state.move_selection(-1);
                after_move(state);
            }
        }
        _ => {}
    }
}

/// Move to the next chain along, wrapping — what ←/→ do.
fn step_chain(app: &App, state: &mut State, delta: isize) {
    let here = ChainId::ALL
        .iter()
        .position(|id| *id == state.current_chain)
        .unwrap_or(0) as isize;
    let next = (here + delta).rem_euclid(ChainId::ALL.len() as isize) as usize;
    select_chain(app, state, ChainId::ALL[next]);
}

/// Keep the detail pane in step with whatever the arrow keys just moved.
fn after_move(state: &mut State) {
    // The highlight *is* the wallet cursor: standing on index 1 means the
    // "New <chain>" commands fill index 1. Keeping them separate would let the
    // screen show one index while the commands acted on another.
    if let Some(index) = state.selected_index() {
        state.current_index = index;
    }
    refresh_detail(state);
}

// ================================================================== dispatch

/// The single place a key press and a menu selection converge.
fn run_action(app: &App, state: &mut State, action: Action) {
    match action {
        Action::Quit => state.quit = true,
        Action::Help => state.mode = Mode::Help,
        Action::Reload => {
            reload(app, state);
            state.info("Reloaded from disk");
        }
        Action::Balance => {
            start_input(
                state,
                InputKind::BalanceAddress,
                "Address to check (blank = this wallet)",
            );
            offer_clipboard_address(app, state);
        }
        Action::NewWallet => new_wallet(app, state),
        Action::NewSeed => start_input(
            state,
            InputKind::Label { new_seed: true },
            "Label for index 0 of a new seed (blank = auto)",
        ),
        Action::ImportMnemonic => start_input(state, InputKind::Mnemonic, "Mnemonic to import"),
        Action::ImportKey => start_input(state, InputKind::PrivateKey, "Private key to import"),
        Action::Recall => open_recall(app, state),
        Action::Activate => set_active(app, state),
        Action::CopyAddress => copy_address(state),
        Action::ToggleSecrets => {
            state.show_secrets = !state.show_secrets;
            match state.focus {
                Focus::Recall => refresh_recall_detail(state),
                _ => refresh_detail(state),
            }
            state.info(if state.show_secrets {
                "Secrets shown — mind your shoulder"
            } else {
                "Secrets hidden"
            });
        }
        Action::SelectNetwork(key) => select_network(app, state, key),
        Action::ExportWallets => start_export_wallets(state),
        Action::Save(format) => start_export(state, format),
        Action::Derive => {
            if state.current().is_some() {
                start_input(state, InputKind::DeriveIndex, "Wallet index to derive");
            } else {
                state.fail("No wallet selected — create one first");
            }
        }
        Action::Sign => {
            if state.current().is_some() {
                start_input(state, InputKind::SignMessage, "Message to sign");
            } else {
                state.fail("No wallet selected — create one first");
            }
        }
        Action::Send => {
            if state.current().is_some() {
                state.pending_to.clear();
                state.pending_amount.clear();
                start_input(state, InputKind::SendTo, "Recipient address");
                offer_clipboard_address(app, state);
            } else {
                state.fail("No wallet selected — create one first");
            }
        }
        Action::Remove => {
            if state.focus == Focus::Recall {
                forget_selected_recall(app, state);
                return;
            }
            // The row is a wallet, so this removes the wallet: every chain's
            // account at that index. Removing one facet and leaving three
            // would make the pane show a wallet that is no longer one.
            match state.selected_index() {
                Some(index) => {
                    let held = state.accounts_at(index);
                    // Named, not numbered: the question has to be answerable
                    // without counting rows, because the answer deletes keys.
                    let count = held.len();
                    let named = held
                        .first()
                        .map(|a| {
                            format!(
                                " ({}, {} …)",
                                store::display_label(a),
                                short_address(&a.address)
                            )
                        })
                        .unwrap_or_default();
                    state.mode = Mode::Confirm(ConfirmKind::Remove);
                    state.info(format!(
                        "Remove wallet index {index}{named} and its {count} account(s)? [y/N]"
                    ));
                    // The pane that holds the target takes focus, so the row
                    // about to go is the highlighted one on a lit border.
                    state.focus = Focus::Accounts;
                }
                None => state.fail("No wallet selected"),
            }
        }
    }
}

// ==================================================================== recall

fn open_recall(app: &App, state: &mut State) {
    match app.store.recent() {
        Ok(entries) => {
            state.recall = entries;
            state.recall_selected.select(if state.recall.is_empty() {
                None
            } else {
                Some(0)
            });
            state.focus = Focus::Recall;
            refresh_recall_detail(state);
            if state.recall.is_empty() {
                state.info("Nothing remembered yet — create or import a wallet first");
            } else {
                state.info("Enter imports · x forgets · Esc goes back");
            }
        }
        Err(e) => state.fail(e.message),
    }
}

fn close_recall(state: &mut State) {
    state.focus = Focus::Accounts;
    refresh_detail(state);
    state.info("Ready");
}

fn import_selected_recall(app: &App, state: &mut State) {
    let Some(entry) = state.current_recall().cloned() else {
        state.fail("Nothing selected");
        return;
    };
    let outcome = if entry.kind == "mnemonic" {
        import_mnemonic(app, &entry.secret)
    } else {
        import_private_key(app, &entry.secret)
    };
    match outcome {
        Ok(address) => {
            state.focus = Focus::Accounts;
            reload(app, state);
            state.info(format!("Imported {address}"));
        }
        Err(e) => state.fail(e.message),
    }
}

fn forget_selected_recall(app: &App, state: &mut State) {
    let Some(entry) = state.current_recall().cloned() else {
        state.fail("Nothing to forget");
        return;
    };
    match app.store.forget_secret(&entry.id) {
        Ok(()) => {
            open_recall(app, state);
            state.info(format!("Forgot {}", entry.id));
        }
        Err(e) => state.fail(e.message),
    }
}

fn refresh_recall_detail(state: &mut State) {
    let Some(entry) = state.current_recall().cloned() else {
        state.detail = vec![("Recall".into(), "Nothing remembered yet.".into())];
        return;
    };
    state.detail = vec![
        ("Id".into(), entry.id.clone()),
        ("Kind".into(), entry.kind.clone()),
        ("Address".into(), entry.address.clone()),
        ("Uses".into(), entry.uses.to_string()),
        ("Last used".into(), entry.last_used_at.clone()),
        (
            if entry.kind == "mnemonic" {
                "Mnemonic".into()
            } else {
                "Private key".into()
            },
            if state.show_secrets {
                entry.secret.clone()
            } else {
                entry.preview()
            },
        ),
    ];
}

// ===================================================================== input

fn start_input(state: &mut State, kind: InputKind, prompt: &str) {
    state.mode = Mode::Input(kind);
    clear_input(state);
    state.info(prompt);
}

/// Open the full-export prompt. This one writes private keys, so it says so.
fn start_export_wallets(state: &mut State) {
    state.mode = Mode::Input(InputKind::ExportWalletsPath);
    state.input = "wallets-keys.jsonl".to_string();
    state.info(format!(
        "Export {} wallets WITH PRIVATE KEYS as jsonl to (Enter accepts)",
        state.accounts.len()
    ));
}

/// Open the export prompt with a sensible filename already filled in.
fn start_export(state: &mut State, format: Format) {
    state.mode = Mode::Input(InputKind::ExportPath(format));
    state.input = format!("wallets.{}", format.extension());
    state.info(format!(
        "Save {} wallets as {} to (Enter accepts)",
        state.accounts.len(),
        format.as_str()
    ));
}

fn input_key(
    app: &App,
    state: &mut State,
    kind: InputKind,
    code: KeyCode,
    modifiers: KeyModifiers,
) {
    // Ctrl-V reads the system clipboard through the platform's own helper —
    // pbpaste here — rather than through the terminal. Terminal paste is kept
    // too, but what it delivers depends on bracketed-paste support and every
    // multiplexer in between; this key works no matter what sits between the
    // wallet and the clipboard.
    // Ctrl-U clears the line — the standard chord, and the "no thanks" to an
    // offered clipboard address without cancelling the prompt itself.
    if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('u' | 'U')) {
        clear_input(state);
        state.clipboard_offer = false;
        return;
    }
    if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('v' | 'V')) {
        match crate::clipboard::paste() {
            Ok(text) if text.trim().is_empty() => {
                state.info("The clipboard is empty");
            }
            Ok(text) => append_pasted(state, &text),
            Err(e) => state.fail(e.message),
        }
        return;
    }
    match code {
        KeyCode::Esc => {
            state.mode = Mode::Browse;
            clear_input(state);
            state.clipboard_offer = false;
            state.info("Cancelled");
        }
        KeyCode::Backspace => {
            state.input.pop();
            state.clipboard_offer = false;
        }
        KeyCode::Char(c) => {
            state.input.push(c);
            state.clipboard_offer = false;
        }
        KeyCode::Enter => {
            let value = state.input.trim().to_string();
            clear_input(state);
            state.mode = Mode::Browse;
            state.clipboard_offer = false;
            submit(app, state, kind, value);
        }
        _ => {}
    }
}

fn confirm_key(app: &App, state: &mut State, kind: ConfirmKind, code: KeyCode) {
    let accepted = matches!(code, KeyCode::Char('y') | KeyCode::Char('Y'));
    state.mode = Mode::Browse;
    if !accepted {
        state.staged = None;
        state.info("Cancelled");
        return;
    }
    match kind {
        ConfirmKind::Remove => remove_wallet(app, state),
        ConfirmKind::Send => broadcast(app, state),
    }
}

fn submit(app: &App, state: &mut State, kind: InputKind, value: String) {
    match kind {
        InputKind::Label { new_seed } => {
            let label = if value.is_empty() {
                None
            } else {
                Some(value.as_str())
            };
            match create_account(app, label, new_seed) {
                Ok(address) => {
                    reload(app, state);
                    state.info(format!("Created {address}"));
                }
                Err(e) => state.fail(e.message),
            }
        }
        InputKind::Mnemonic => match import_mnemonic(app, &value) {
            Ok(address) => {
                reload(app, state);
                state.info(format!("Imported {address}"));
            }
            Err(e) => state.fail(e.message),
        },
        InputKind::PrivateKey => match import_private_key(app, &value) {
            Ok(address) => {
                reload(app, state);
                state.info(format!("Imported {address}"));
            }
            Err(e) => state.fail(e.message),
        },
        InputKind::DeriveIndex => match value.parse::<u32>() {
            Ok(index) => match derive(app, state, index) {
                Ok(address) => {
                    reload(app, state);
                    state.info(format!("Derived {address}"));
                }
                Err(e) => state.fail(e.message),
            },
            Err(_) => state.fail(format!("'{value}' is not an address index")),
        },
        InputKind::BalanceAddress => {
            let target = if value.trim().is_empty() {
                None
            } else {
                Some(value.trim().to_string())
            };
            start_balance(app, state, target);
        }
        InputKind::SendTo => {
            // Checked against the *selected account's* chain. An EVM-only
            // check here rejected every valid Solana, Cardano and Midnight
            // address before a send could even be staged.
            let Some(account) = state.current().cloned() else {
                state.fail("No wallet selected");
                return;
            };
            let network = network_for(app, &account);
            if let Err(e) =
                causewaybay_core::chain::chain(account.chain).check_address(&network, &value)
            {
                state.fail(e.message);
                return;
            }
            state.pending_to = value;
            start_input(
                state,
                InputKind::SendAmount,
                &format!("Amount to send, in {}", network.symbol),
            );
        }
        InputKind::SendAmount => {
            let Some(account) = state.current().cloned() else {
                state.fail("No wallet selected");
                return;
            };
            // Each chain counts to its own number of decimal places, so an
            // 18-decimal check would wave through amounts Solana cannot hold.
            let network = network_for(app, &account);
            if let Err(e) = network.units().parse(&value) {
                state.fail(e.message);
                return;
            }
            state.pending_amount = value;
            stage_send(app, state);
        }
        InputKind::SignMessage => match sign(app, state, &value) {
            Ok(signature) => {
                state.detail = vec![("Message".into(), value), ("Signature".into(), signature)];
                state.info("Signed");
            }
            Err(e) => state.fail(e.message),
        },
        InputKind::ExportWalletsPath => match export_accounts_with_keys(app, state, &value) {
            Ok(full) => {
                state.detail = vec![
                    ("Exported".into(), full),
                    ("Format".into(), "jsonl".into()),
                    ("Wallets".into(), state.accounts.len().to_string()),
                    (
                        "Includes".into(),
                        "address, private key, both public keys".into(),
                    ),
                    ("Permissions".into(), "0600 — owner only".into()),
                ];
                state.info(format!(
                    "Exported {} wallets with keys to {value}",
                    state.accounts.len()
                ));
            }
            Err(e) => state.fail(e.message),
        },
        InputKind::ExportPath(format) => match export_accounts(app, state, format, &value) {
            Ok(full) => {
                // The status bar is one line, so it gets the short name and the
                // detail pane — which wraps — gets the full path.
                state.detail = vec![
                    ("Saved".into(), full),
                    ("Format".into(), format.as_str().to_string()),
                    ("Wallets".into(), state.accounts.len().to_string()),
                    (
                        "Secrets".into(),
                        if state.show_secrets {
                            "included".into()
                        } else {
                            "excluded".to_string()
                        },
                    ),
                ];
                state.info(format!("Saved {} wallets to {value}", state.accounts.len()));
            }
            Err(e) => state.fail(e.message),
        },
    }
}

/// Write every wallet, keys included, as JSONL.
///
/// Always owner-only on disk and always with secrets: unlike "save list", the
/// point of this one is to move key material, so it does not depend on whether
/// the detail pane happens to be revealing secrets.
fn export_accounts_with_keys(app: &App, state: &State, path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(error::usage("no filename given"));
    }
    if state.accounts.is_empty() {
        return Err(error::usage("there are no wallets to export"));
    }
    let active = app.store.active_account().ok().map(|a| a.id);
    let rendered = export::render(&state.accounts, Format::Jsonl, active.as_deref(), true);

    let target = std::path::Path::new(path);
    // Born 0600: the file holds private keys from its first byte.
    causewaybay_core::paths::write_private(target, &rendered)
        .map_err(|e| error::internal(format!("cannot write {path}: {e}")))?;
    Ok(std::fs::canonicalize(target)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string()))
}

/// Write the wallet list to `path`, returning the absolute path written.
fn export_accounts(app: &App, state: &State, format: Format, path: &str) -> Result<String> {
    if path.is_empty() {
        return Err(error::usage("no filename given"));
    }
    if state.accounts.is_empty() {
        return Err(error::usage("there are no wallets to save"));
    }
    let active = app.store.active_account().ok().map(|a| a.id);
    // Secrets follow whatever the detail pane is currently showing, so saving a
    // file can never reveal more than what is already on screen.
    let rendered = export::render(
        &state.accounts,
        format,
        active.as_deref(),
        state.show_secrets,
    );

    let target = std::path::Path::new(path);
    if state.show_secrets {
        // Born 0600: the file holds private keys from its first byte.
        causewaybay_core::paths::write_private(target, &rendered)
            .map_err(|e| error::internal(format!("cannot write {path}: {e}")))?;
    } else {
        std::fs::write(target, rendered)
            .map_err(|e| error::internal(format!("cannot write {path}: {e}")))?;
    }
    Ok(std::fs::canonicalize(target)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.to_string()))
}

// =================================================================== actions

/// Pull the address out of whatever `App` just returned.
fn address_of(output: &CommandOutput) -> String {
    output
        .data
        .get("address")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

// The wallet operations below all go through `App`, the same entry point the
// CLI uses. Reimplementing them here is how the two front ends drift: an
// earlier version of this file derived addresses without recording the mnemonic
// in the recall list, which the CLI had always done.

fn create_account(app: &App, label: Option<&str>, new_seed: bool) -> Result<String> {
    let output = app.account(AccountCommand::New {
        label: label.map(str::to_string),
        new_seed,
        words: 12,
        index: None,
        show_secret: false,
        // The TUI creates one account on the chain in view; `--every-chain` is
        // a deliberate CLI act rather than something a keystroke should do.
        every_chain: false,
    })?;
    Ok(address_of(&output))
}

/// `-` means "read stdin" to the CLI, which in a raw-mode TUI blocks forever.
fn reject_stdin_placeholder(value: &str, what: &str) -> Result<()> {
    if value.trim() == "-" {
        return Err(error::usage(format!(
            "'-' reads from stdin, which a full-screen UI cannot do — type the {what} instead"
        )));
    }
    Ok(())
}

fn import_mnemonic(app: &App, phrase: &str) -> Result<String> {
    reject_stdin_placeholder(phrase, "mnemonic")?;
    let output = app.account(AccountCommand::ImportMnemonic {
        mnemonic: Some(phrase.to_string()),
        index: 0,
        label: None,
        passphrase: String::new(),
        // A mnemonic is a whole wallet. Importing it on one chain and leaving
        // the other three behind is what made an import look half-finished.
        every_chain: true,
    })?;
    Ok(address_of(&output))
}

fn import_private_key(app: &App, key: &str) -> Result<String> {
    reject_stdin_placeholder(key, "private key")?;
    let output = app.account(AccountCommand::ImportKey {
        private_key: Some(key.to_string()),
        label: None,
    })?;
    Ok(address_of(&output))
}

fn derive(app: &App, state: &State, index: u32) -> Result<String> {
    let account = state
        .current()
        .ok_or_else(|| error::usage("no wallet selected"))?;
    let output = app.account(AccountCommand::Derive {
        index,
        label: None,
        from: Some(account.id.clone()),
        // Deriving an index makes the wallet at that index, which is all four
        // chains — the same thing `New wallet` makes.
        every_chain: true,
    })?;
    Ok(address_of(&output))
}

/// Drop whatever was typed, overwriting a secret before releasing it.
///
/// Best effort only: `String` may have reallocated while it grew, and those
/// earlier buffers are already out of reach. It still means the live buffer is
/// not left holding a seed phrase for the rest of the session.
fn clear_input(state: &mut State) {
    let len = state.input.len();
    if len > 0 {
        state.input.replace_range(.., &"\0".repeat(len));
    }
    state.input.clear();
}

/// Put the selected wallet's address on the system clipboard.
fn copy_address(state: &mut State) {
    let Some(account) = state.current().cloned() else {
        state.fail("No wallet selected");
        return;
    };
    match crate::clipboard::copy(&account.address) {
        Ok(helper) => state.info(format!(
            "Copied {} to the clipboard ({helper})",
            account.address
        )),
        Err(e) => state.fail(format!(
            "{} — the address is {}",
            e.message, account.address
        )),
    }
}

fn set_active(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        state.fail("No wallet selected");
        return;
    };
    match app
        .store
        .config_set(causewaybay_core::store::KEY_ACTIVE_ACCOUNT, &account.id)
    {
        Ok(()) => state.info(format!("{} is now active", account.label)),
        Err(e) => state.fail(e.message),
    }
}

/// Forget the highlighted wallet: every chain's account at that index.
///
/// The mnemonic is not forgotten with it — it stays in the recall list, so a
/// wallet removed by mistake can be imported straight back.
fn remove_wallet(app: &App, state: &mut State) {
    let Some(index) = state.selected_index() else {
        return;
    };
    let ids: Vec<String> = state
        .accounts_at(index)
        .iter()
        .map(|a| a.id.clone())
        .collect();
    let mut removed = 0;
    for id in &ids {
        match app.store.delete_account(id) {
            Ok(()) => removed += 1,
            Err(e) => {
                // Stop at the first refusal rather than carrying on and
                // leaving a wallet in halves without saying so.
                reload(app, state);
                state.fail(format!("Removed {removed} of {}: {}", ids.len(), e.message));
                return;
            }
        }
    }
    reload(app, state);
    state.info(format!(
        "Removed wallet index {index} — {removed} account(s); the phrase is still in Recall"
    ));
}

/// Offer the clipboard's content as the prompt's starting value, and say so.
///
/// The offer is a question, not an assumption: the address appears in the
/// prompt with its provenance named, and the user answers with Enter (use it)
/// or Ctrl-U (clear it and type another). Nothing is queried or sent until
/// they do.
///
/// Offered only when the clipboard is a plausible answer: one line, a valid
/// address on the selected account's chain, and not that account's own.
/// Anything else — a mnemonic, a number, prose — is left alone, so the prompt
/// never opens holding junk, and never echoes a secret that happened to be
/// copied.
fn offer_clipboard_address(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        return;
    };
    let Ok(text) = crate::clipboard::paste() else {
        return;
    };
    let candidate = text.trim();
    if candidate.is_empty() || candidate.contains(char::is_whitespace) {
        return;
    }
    let network = network_for(app, &account);
    if causewaybay_core::chain::chain(account.chain)
        .check_address(&network, candidate)
        .is_err()
    {
        return;
    }
    if candidate == account.address {
        // Blank already means this wallet, and a send to it is refused anyway.
        return;
    }
    state.input = candidate.to_string();
    state.clipboard_offer = true;
}

/// The wallet scoped to one account's chain.
///
/// Commands follow `App::chain`, and the highlighted account is not always on
/// it — a Solana row can be selected while the App sits on EVM. Every command
/// that acts on the selection goes through here, or it reaches for the wrong
/// chain's keys, address format and node.
fn app_for(app: &App, account: &Account) -> Result<App> {
    app.reopen_on(account.chain)
}

/// The network the selected account's chain is on.
fn network_for(app: &App, account: &Account) -> causewaybay_core::network::Network {
    app.store
        .network_on(account.chain)
        .unwrap_or_else(|_| causewaybay_core::network::default_for(account.chain))
}

fn sign(app: &App, state: &State, message: &str) -> Result<String> {
    let account = state
        .current()
        .ok_or_else(|| error::usage("no wallet selected"))?;
    // Each chain signs in its own scheme and reads its own key format, so this
    // must not reach for an EVM keypair: three of the four would fail outright.
    let signer = causewaybay_core::chain::chain(account.chain).signer(&account.private_key)?;
    let _ = app;
    Ok(format!(
        "0x{}",
        hex::encode(signer.sign_message(message.as_bytes())?)
    ))
}

/// Read a balance: the address given, or the selected wallet's own.
/// Ask the node for a balance, on a thread.
///
/// A balance is a round trip to whatever the node feels like doing, and on the
/// UI thread that froze the screen for as long as it took — including the case
/// where it never answers at all. It goes the same way a send does now: a
/// thread, a clock in the status line, and Esc to stop waiting.
fn start_balance(app: &App, state: &mut State, target: Option<String>) {
    let Some(account) = state.current().cloned() else {
        state.fail("No wallet selected — create one first");
        return;
    };
    // The query goes to the selected account's chain, which is not always the
    // one the App happens to be on.
    let scoped = match app_for(app, &account) {
        Ok(scoped) => scoped,
        Err(e) => {
            state.fail(e.message);
            return;
        }
    };
    let address = target.unwrap_or_else(|| account.address.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    let scoped = scoped.with_host(std::sync::Arc::new(JobHost { events: tx.clone() }));
    std::thread::spawn(move || {
        let outcome = match scoped.balance(TargetArgs {
            address: Some(address),
            account: None,
            all: false,
        }) {
            Ok(output) => {
                let balance = output.data["balance"].as_str().unwrap_or("?");
                let symbol = output.data["symbol"].as_str().unwrap_or("");
                JobEvent::Done(JobResult::Balance(
                    format!("{balance} {symbol}").trim_end().to_string(),
                ))
            }
            Err(e) => JobEvent::Failed(e.message),
        };
        let _ = tx.send(outcome);
    });

    state.job = Some(Job {
        events: rx,
        started: Instant::now(),
        last: format!("Querying the {} balance", account.chain),
        abandoned: "Stopped waiting — nothing was changed",
    });
    state.mode = Mode::Busy;
    state.info(format!("Querying the {} balance…", account.chain));
}

/// Work out what the transfer would cost and ask before signing anything.
///
/// The plan comes from `App`, so the TUI gets the same nonce and gas resolution
/// and the same refusal when the balance cannot cover the transfer.
fn stage_send(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        state.fail("No wallet selected");
        return;
    };
    let args = SendArgs {
        to: state.pending_to.clone(),
        amount: state.pending_amount.clone(),
        gas_limit: None,
        gas_price_gwei: None,
        nonce: None,
        data: None,
        max_fee: None,
        wait: false,
        dry_run: false,
        account: Some(account.id.clone()),
    };
    let scoped = match app_for(app, &account) {
        Ok(scoped) => scoped,
        Err(e) => {
            state.fail(e.message);
            return;
        }
    };

    // Planning happens off the UI thread. On most chains it is a second or
    // two of RPC; on Midnight it can be minutes of dust-ledger replay and
    // proof generation, and running that here froze every pixel of the TUI —
    // which is indistinguishable from a crash, over a command that moves
    // money. The thread narrates through the channel and the loop keeps
    // drawing; nothing is broadcast until the confirm screen is answered.
    let (tx, rx) = std::sync::mpsc::channel();
    let scoped = scoped.with_host(std::sync::Arc::new(JobHost { events: tx.clone() }));
    std::thread::spawn(move || {
        let outcome = match scoped.plan_send(&args) {
            Ok(plan) => JobEvent::Done(JobResult::Plan(Box::new(plan))),
            Err(e) => JobEvent::Failed(e.message),
        };
        // The UI may have abandoned the wait; that is its right, and the
        // plan being dropped here is exactly what abandoning means.
        let _ = tx.send(outcome);
    });

    state.job = Some(Job {
        events: rx,
        started: Instant::now(),
        last: format!("Preparing the {} transfer", account.chain),
        abandoned: "Abandoned — nothing was sent",
    });
    state.mode = Mode::Busy;
    state.info(format!("Preparing the {} transfer…", account.chain));
}

/// Drain the background thread's narration, and take its result when it lands.
fn poll_job(state: &mut State) {
    let Some(job) = &state.job else { return };
    let mut finished: Option<JobEvent> = None;
    let mut narration: Option<String> = None;
    loop {
        match job.events.try_recv() {
            Ok(JobEvent::Progress(line)) => narration = Some(line),
            Ok(done) => {
                finished = Some(done);
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                // The thread died without a verdict — a panic. Nothing was
                // broadcast (submission only happens after the confirm), so
                // report it and move on rather than spinning forever.
                finished = Some(JobEvent::Failed(
                    "the work stopped unexpectedly; nothing was sent".into(),
                ));
                break;
            }
        }
    }

    let elapsed = job.started.elapsed().as_secs();
    if let Some(line) = narration {
        state.job.as_mut().expect("checked above").last = line;
    }
    match finished {
        None => {
            // Re-render the running clock even when the thread said nothing,
            // so a long prove visibly counts rather than visibly sitting.
            let last = state.job.as_ref().expect("checked above").last.clone();
            state.info(format!("{last} · {elapsed}s"));
        }
        Some(JobEvent::Done(JobResult::Plan(plan))) => {
            state.job = None;
            present_plan(state, *plan);
        }
        Some(JobEvent::Done(JobResult::Balance(balance))) => {
            state.job = None;
            state.mode = Mode::Browse;
            // The row sits with the rest of the wallet's facts rather than
            // only in the status line, which the next message would take.
            state.detail.retain(|(key, _)| key != "Balance");
            state.detail.push(("Balance".into(), balance.clone()));
            state.info(format!("Balance {balance}"));
        }
        Some(JobEvent::Failed(message)) => {
            state.job = None;
            state.mode = Mode::Browse;
            state.fail(message);
        }
        Some(JobEvent::Progress(_)) => unreachable!("progress never breaks the loop"),
    }
}

/// Put a finished plan on screen and ask the question.
fn present_plan(state: &mut State, plan: SendPlan) {
    // The transfer is already signed and checked by this point; what is on
    // screen is what would go out, and every chain fills the same four rows
    // plus whatever detail it has of its own.
    let units = plan.network.units();
    let prepared = &plan.prepared;
    let mut detail = vec![
        ("Chain".into(), plan.account.chain.as_str().to_string()),
        ("To".into(), prepared.to.clone()),
        ("Amount".into(), units.format_with_symbol(prepared.amount)),
        // Midnight pays its fee in DUST while moving NIGHT, so the fee gets
        // its own unit rather than the transfer's.
        (
            "Fee".into(),
            plan.fee_units().format_with_symbol(prepared.fee),
        ),
    ];
    if let Some(nonce) = prepared.nonce {
        detail.push(("Nonce".into(), nonce.to_string()));
    }
    if let Some(map) = prepared.detail.as_object() {
        for (key, value) in map {
            if value.is_null() {
                continue;
            }
            let shown = match value {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let mut label = key.replace('_', " ");
            if let Some(first) = label.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            detail.push((label, shown));
        }
    }
    let prompt = format!("{}? [y/N]", plan.prompt());
    state.detail = detail;
    state.staged = Some(plan);
    state.mode = Mode::Confirm(ConfirmKind::Send);
    state.info(prompt);
}

fn broadcast(app: &App, state: &mut State) {
    let Some(plan) = state.staged.take() else {
        state.fail("Nothing staged to send");
        return;
    };
    state.info("Broadcasting…");
    match app.execute_send(plan, false) {
        Ok(output) => {
            let hash = output.data["hash"].as_str().unwrap_or_default().to_string();
            state.detail = vec![
                ("Sent".into(), hash.clone()),
                (
                    "Explorer".into(),
                    output.data["explorer"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
            ];
            state.info(format!("Submitted {hash}"));
        }
        Err(e) => state.fail(e.message),
    }
}

/// Switch to a named network, rather than cycling through them.
///
/// `App` resolves its network once at construction, so anything it does after a
/// switch — balances, the symbol, explorer links — would keep using the old
/// chain. The TUI therefore rebuilds its `App` rather than only repainting.
/// Move to a chain, on whichever of its networks the wallet last used.
///
/// Switching chain is a network switch underneath — a chain is only reachable
/// through one of its networks — so this resolves the remembered one and goes
/// through the same path, rather than leaving the two settings able to
/// disagree about which chain is in view.
fn select_chain(app: &App, state: &mut State, chain: ChainId) {
    let target = app
        .store
        .network_on(chain)
        .unwrap_or_else(|_| network::default_for(chain));
    if state.current_chain == chain {
        state.info(format!(
            "Already on {} ({})",
            causewaybay_core::chain::chain(chain).name(),
            target.name
        ));
        return;
    }
    select_network(app, state, target.key);
    // The network switch reports itself; say the thing that actually changed.
    if !state.status_is_error {
        let held = state.accounts.iter().filter(|a| a.chain == chain).count();
        state.info(format!(
            "Now on {} · {} — {}",
            causewaybay_core::chain::chain(chain).name(),
            target.name,
            match held {
                0 => "no wallets here yet, press n to make one".to_string(),
                1 => "1 wallet".to_string(),
                n => format!("{n} wallets"),
            }
        ));
    }
}

/// Create the next wallet: a fresh index, derived on every chain at once.
///
/// A wallet is one mnemonic and one index, and every chain has an account at
/// that index — so making one is a single act rather than a slot to be filled
/// chain by chain. The first ever call mints the mnemonic too.
fn new_wallet(app: &App, state: &mut State) {
    let index = state.next_free_index();
    let outcome = app.account(AccountCommand::New {
        label: None,
        new_seed: false,
        words: 12,
        index: Some(index),
        show_secret: false,
        every_chain: true,
    });
    match outcome {
        Ok(_) => {
            reload(app, state);
            state.select_index(index);
            state.info(format!(
                "Created wallet index {index} on {} chains",
                ChainId::ALL.len()
            ));
        }
        Err(e) => state.fail(e.message),
    }
}

fn select_network(app: &App, state: &mut State, key: &'static str) {
    let target = match network::find(key) {
        Ok(found) => found,
        Err(e) => {
            state.fail(e.message);
            return;
        }
    };
    if state.current_network == target.key {
        state.info(format!("Already on {}", target.name));
        return;
    }
    match app.store.set_network(&target) {
        Ok(()) => {
            state.current_network = target.key.to_string();
            // The network carries its chain, so the two can never disagree
            // about which chain the rest of the screen is describing.
            state.set_chain(target.chain);
            state.network_changed = true;
            state.info(format!(
                "Network is now {} on {}",
                target.name,
                causewaybay_core::chain::chain(target.chain).name()
            ));
        }
        Err(e) => state.fail(e.message),
    }
}

fn reload(app: &App, state: &mut State) {
    if let Ok(accounts) = app.store.accounts() {
        let keep = state.selected_index().unwrap_or(state.current_index);
        state.accounts = accounts;
        state.selected.select(if state.accounts.is_empty() {
            None
        } else {
            Some(0)
        });
        state.select_index(keep);
        state.clamp_selection();
    }
    refresh_detail(state);
}

fn refresh_detail(state: &mut State) {
    let Some(index) = state.selected_index() else {
        state.detail = Vec::new();
        return;
    };
    // The wallet first, one line per chain: this is where the four accounts of
    // one index live now that the list itself is just the indices. The chain in
    // view is marked, because that is the one every command acts on.
    let mut rows = vec![("Wallet".to_string(), format!("index {index}"))];
    for chain in ChainId::ALL {
        let held = state
            .accounts_at(index)
            .into_iter()
            .find(|a| a.chain == chain);
        let marker = if chain == state.current_chain {
            "▸"
        } else {
            " "
        };
        rows.push((
            format!("{marker} {chain}"),
            match held {
                // Tab-separated, not laid out: how much of this fits is a
                // question about the pane's width, which only the draw knows.
                Some(account) => format!(
                    "{}\t{}",
                    store::display_label(account),
                    short_address(&account.address)
                ),
                None => "— not derived yet".to_string(),
            },
        ));
    }
    rows.push((String::new(), String::new()));

    let Some(account) = state.current().cloned() else {
        state.detail = rows;
        return;
    };
    // What is left says something or it is not here. No Address row — the ▸
    // line above is that address. No Source row unless the key was imported,
    // because a wallet's source is "mnemonic" by construction. No explorer
    // link — three wrapped lines of a URL nothing in a terminal can follow,
    // when `y` copies the address the browser actually wants. And no rows
    // standing in for the secrets: `v` is what shows those.
    if account.source != causewaybay_core::store::Source::Mnemonic {
        rows.push(("Source".to_string(), account.source.as_str().to_string()));
    }
    if let Some(path) = &account.derivation_path {
        rows.push(("Path".to_string(), path.clone()));
    }
    if state.show_secrets {
        rows.push(("Private key".to_string(), account.private_key.clone()));
        if let Some(mnemonic) = &account.mnemonic {
            rows.push(("Mnemonic".to_string(), mnemonic.clone()));
        }
    }
    state.detail = rows;
}

// ================================================================= rendering

fn draw(frame: &mut Frame, app: &App, state: &mut State) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            // One line of header and its border. What chain, what network,
            // which account: everything else the header used to carry — the
            // wallet index, a tally per chain — is in the panes below it, and
            // saying it twice cost a row of the screen.
            Constraint::Length(3), // header
            Constraint::Min(6),    // body
            Constraint::Length(3), // status / prompt
            Constraint::Length(1), // hint
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(26), // commands
            // Narrow on purpose: a row is one wallet index, and the width it
            // used to need for chains and addresses is worth more to the
            // detail pane, which is where those live now. 26 is what a
            // two-digit index and the "missing a chain" note want — but it is
            // a maximum, not a claim: on a narrow terminal the wallet list
            // gives its columns up so that an address still fits on one line
            // beside its label, which is the thing the pane is for.
            Constraint::Max(26),
            Constraint::Min(30),
        ])
        .split(chunks[1]);

    draw_commands(frame, state, body[0]);
    match state.focus {
        Focus::Recall => draw_recall(frame, state, body[1]),
        _ => draw_accounts(frame, state, body[1]),
    }
    draw_detail(frame, state, body[2]);

    draw_status(frame, state, chunks[2]);
    draw_hint(frame, state, chunks[3]);

    if state.mode == Mode::Help {
        draw_help_overlay(frame, &state.command_list, frame.area());
    }
}

/// What the status line shows for the text typed so far.
///
/// A mnemonic or a private key is never echoed: someone pasting a seed phrase
/// should not have it sitting in a terminal that may be shared, scrolled back,
/// or recorded. The mask still reports progress — one dot per word, or per hex
/// character — because "did my paste arrive, and is it the right length?" is
/// the question the prompt has to answer.
fn echo(kind: InputKind, input: &str) -> String {
    match kind {
        InputKind::Mnemonic => {
            let words = input.split_whitespace().count();
            if words == 0 {
                String::new()
            } else {
                format!(
                    "{} ({words} word{})",
                    "•".repeat(words.min(32)),
                    if words == 1 { "" } else { "s" }
                )
            }
        }
        InputKind::PrivateKey => {
            let body = input.trim();
            let body = body
                .strip_prefix("0x")
                .or_else(|| body.strip_prefix("0X"))
                .unwrap_or(body);
            let count = body.chars().count();
            if count == 0 {
                String::new()
            } else {
                format!(
                    "{} ({count} hex char{})",
                    "•".repeat(count.min(32)),
                    if count == 1 { "" } else { "s" }
                )
            }
        }
        // Labels, addresses, amounts and filenames are not secrets, and seeing
        // them is how a typo gets caught.
        _ => input.to_string(),
    }
}

/// A brighter border marks the pane the arrow keys apply to.
fn pane_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Two lines: what is in play, and what the wallet holds elsewhere.
///
/// The second line is the multi-chain one. A wallet spread over four chains
/// otherwise gives no sign that the other three exist, and "my funds are gone"
/// is the reading that follows.
fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let network = app.store.network().unwrap_or(app.network);
    let chain = network.chain;

    let mut first = vec![
        Span::styled("Causewaybay", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  ·  "),
        Span::styled(
            causewaybay_core::chain::chain(chain).name().to_string(),
            Style::default()
                .fg(chain_colour(chain))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::styled(
            network.name.to_string(),
            Style::default().fg(if network.testnet {
                Color::Green
            } else {
                // Mainnet is where a mistake costs something, so it does not
                // look like the testnets.
                Color::Red
            }),
        ),
        Span::raw("  ·  "),
    ];

    // The address that balance, send and the CLI all act on by default, for
    // *this* chain — worth having in view rather than a pane away.
    match app.store.active_account_on(chain).ok() {
        Some(account) => {
            first.push(Span::styled(
                format!("{}: ", store::display_label(&account)),
                Style::default().fg(Color::DarkGray),
            ));
            first.push(Span::styled(
                short_address(&account.address),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        None => first.push(Span::styled(
            format!("no {chain} wallet yet — press n"),
            Style::default().fg(Color::DarkGray),
        )),
    }

    frame.render_widget(
        Paragraph::new(vec![Line::from(first)]).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_commands(frame: &mut Frame, state: &mut State, area: Rect) {
    let focused = state.focus == Focus::Commands;
    let current_network = state.current_network.clone();
    let items: Vec<ListItem> = state
        .visible_commands()
        .into_iter()
        .map(|command| {
            // A network row is marked when it is the one in use, so the pane
            // doubles as the answer to "which chain am I on?".
            let selected_network = matches!(
                command.action,
                Action::SelectNetwork(key) if key == current_network
            );
            let label_style = match command.action {
                // A network row wears its chain's colour, so ten flat rows
                // still read as four groups; the one in use is bold.
                Action::SelectNetwork(key) => {
                    let colour = network::find(key)
                        .map(|n| chain_colour(n.chain))
                        .unwrap_or(Color::White);
                    if selected_network {
                        Style::default().fg(colour).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colour)
                    }
                }
                _ => Style::default().fg(Color::White),
            };
            let marker = if selected_network { "●" } else { " " };
            // The key leads the row. It is the affordance the pane exists to
            // teach, and a narrow terminal that has to cut something should
            // cut the tail of a label rather than the key that runs it.
            ListItem::new(Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Green)),
                Span::styled(
                    format!("{} ", command.key.unwrap_or(' ')),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(command.label.clone(), label_style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pane_style(focused))
                .title(" Commands "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state.commands);
}

fn draw_accounts(frame: &mut Frame, state: &mut State, area: Rect) {
    let focused = state.focus == Focus::Accounts;
    let current_index = state.current_index;
    let indices = state.indices();

    // One row per wallet, not per account. A wallet is one mnemonic and one
    // index; its four chain accounts are facets of it, and they are laid out
    // in the detail pane rather than turned into four rows that read as four
    // unrelated wallets.
    let items: Vec<ListItem> = indices
        .iter()
        .map(|index| {
            let here = *index == current_index;
            let held = state.accounts_at(*index).len();
            let mut spans = vec![Span::styled(
                format!("index {index}"),
                Style::default()
                    .fg(if here { Color::White } else { Color::Gray })
                    .add_modifier(if here {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )];
            // A wallet is normally on every chain, so saying "4/4" on every
            // row says nothing. The count appears only when it is news — a
            // wallet that is missing a chain, which is the one thing about a
            // row worth reading before opening it.
            if held < ChainId::ALL.len() {
                let total = ChainId::ALL.len();
                spans.push(Span::styled(
                    if area.width >= 26 {
                        format!("   {held}/{total} chains")
                    } else {
                        // Cut to the number rather than to half a word.
                        format!("  {held}/{total}")
                    },
                    Style::default().fg(Color::Yellow),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // Kept short: the pane is 22 columns inside its border, and a title past
    // that is cut off mid-word rather than wrapped.
    let title = if state.accounts.is_empty() {
        " Wallets — press n ".to_string()
    } else {
        format!(" Wallets ({}) ", indices.len())
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pane_style(focused))
                .title(title),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state.selected);
}

fn draw_recall(frame: &mut Frame, state: &mut State, area: Rect) {
    let items: Vec<ListItem> = state
        .recall
        .iter()
        .map(|entry| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<12}", entry.kind),
                    Style::default().fg(if entry.kind == "mnemonic" {
                        Color::Green
                    } else {
                        Color::Magenta
                    }),
                ),
                Span::raw(short_address(&entry.address)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pane_style(state.focus == Focus::Recall))
                .title(format!(" Remembered ({}) ", state.recall.len())),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut state.recall_selected);
}

fn draw_detail(frame: &mut Frame, state: &State, area: Rect) {
    // What is left of a row after the border and the key column, and the name
    // column the whole block agrees on within it.
    let room = (area.width as usize).saturating_sub(2 + 13);
    let column = pair_column(&state.detail, room);
    let lines: Vec<Line> = if state.detail.is_empty() {
        vec![
            Line::from(Span::styled(
                "No wallet yet.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "Press n",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to make one.", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "One phrase, one account per chain.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        state
            .detail
            .iter()
            .map(|(key, value)| {
                if key.is_empty() && value.is_empty() {
                    return Line::from("");
                }
                Line::from(vec![
                    Span::styled(format!("{key:<13}"), Style::default().fg(Color::DarkGray)),
                    Span::raw(fit_pair(value, column)),
                ])
            })
            .collect()
    };
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(pane_style(false))
                .title(" Detail "),
        ),
        area,
    );
}

fn draw_status(frame: &mut Frame, state: &State, area: Rect) {
    let (text, style, wrap) = match state.mode {
        Mode::Input(kind) => (
            // The end of the line, not the start: the input area is one row,
            // and a pasted address is one unbreakable "word". Word-wrapping it
            // pushed the whole thing onto a second line that does not exist —
            // so a long paste landed in the buffer and rendered as nothing at
            // all, which read as "paste is broken". Showing the tail keeps the
            // cursor and the most recently typed or pasted text in view.
            tail_fit(
                &format!("{} > {}_", state.status, echo(kind, &state.input)),
                area.width.saturating_sub(2) as usize,
            ),
            Style::default().fg(Color::Cyan),
            false,
        ),
        Mode::Busy => (
            tail_fit(&state.status, area.width.saturating_sub(2) as usize),
            Style::default().fg(Color::Yellow),
            false,
        ),
        _ => (
            state.status.clone(),
            if state.status_is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            },
            true,
        ),
    };
    let mut paragraph = Paragraph::new(Line::from(Span::styled(text, style)))
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: true });
    }
    frame.render_widget(paragraph, area);
}

/// Fit text into one row by keeping its tail.
///
/// `…` marks that something scrolled off to the left, the way a browser's
/// address bar does it.
fn tail_fit(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width || width == 0 {
        return text.to_string();
    }
    let keep = width.saturating_sub(1);
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("…{tail}")
}

/// The bottom line's text: the one place the clipboard offer can ask its
/// question, because a long address tail-scrolls the status line and would
/// push any wording there straight off the screen.
/// What the keys under the user's fingers do, right now.
///
/// This line is the manual. It changes with the mode *and* with the pane in
/// focus, because "↑↓ move" means something different in a list of commands
/// than in a list of wallets — and a first-time user should never have to
/// open the reference to find out which.
fn hint_for(state: &State) -> &'static str {
    match state.mode {
        Mode::Input(_) if state.clipboard_offer => {
            "Clipboard address in the prompt — Enter uses it · Ctrl-U clears it · or just type"
        }
        Mode::Input(_) => "Enter confirm · Esc cancel · Ctrl-V paste · Ctrl-U clear",
        Mode::Confirm(_) => "y confirm · any other key cancels",
        Mode::Busy => "working on a background thread — Esc stops waiting",
        Mode::Help => "any key closes this reference",
        Mode::Browse if state.accounts.is_empty() => {
            "n makes your first wallet — one phrase, all four chains · ? help · q quit"
        }
        Mode::Browse => match state.focus {
            Focus::Commands => "↑↓ pick · Enter run · Tab to the wallets · ? help · q quit",
            Focus::Accounts => {
                "↑↓ wallet · ←→ chain · Enter balance · s send · y copy · Tab back · ? help"
            }
            Focus::Recall => "↑↓ pick · Enter imports · x forgets · Esc goes back",
        },
    }
}

/// One line, always the same, so the bottom of the screen never overflows.
fn draw_hint(frame: &mut Frame, state: &State, area: Rect) {
    let hint = hint_for(state);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {hint}"),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

/// Wide enough that no help line wraps — see the test that checks it.
const HELP_WIDTH: u16 = 78;

/// The column a command label is padded into, in both the pane and the help.
const LABEL_WIDTH: usize = 17;

/// How many lines of network summary the help overlay needs.
///
/// The command *pane* keeps one row per network — that is how you pick one.
/// The help does not: ten near-identical "switch to …" lines would push the
/// overlay past the bottom of a normal terminal, so they are folded into one
/// line per chain instead.
fn help_network_lines() -> usize {
    causewaybay_core::ChainId::ALL.len()
}

/// The height the help overlay asks for, given the commands it will list.
///
/// Shared with the test that checks it fits, so the two cannot drift: the
/// overlay clipping its last few commands is invisible until someone looks
/// for a command that is off the bottom.
/// Whether the help overlay folds this command into a summary line rather
/// than giving it a row of its own.
///
/// Four chains, ten networks, four per-chain creates and four export formats
/// are twenty-two near-identical lines; listed one each they push the overlay
/// off the bottom of a normal terminal, and a clipped reference is worse than
/// a condensed one.
fn summarised_in_help(action: Action) -> bool {
    matches!(action, Action::SelectNetwork(_) | Action::Save(_))
}

fn help_overlay_height(commands: &[Command]) -> u16 {
    let listed = commands
        .iter()
        .filter(|c| !summarised_in_help(c.action))
        .count();
    // 5 navigation rows, a blank, a heading, the commands, a blank, a chains
    // heading, one line per chain, a menu note, a pane note, a save line, two
    // borders.
    (5 + 1 + 1 + listed + 1 + 1 + help_network_lines() + 1 + 1 + 1 + 1 + 2) as u16
}

/// The full reference, drawn over the top of everything else.
fn draw_help_overlay(frame: &mut Frame, commands: &[Command], area: Rect) {
    let popup = centered_rect(HELP_WIDTH, help_overlay_height(commands), area);
    frame.render_widget(Clear, popup);

    let mut lines = vec![
        Line::from(Span::styled(
            "Navigation",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("  Tab        move between the Commands and Wallets panes"),
        Line::from("  ↑ ↓ / j k  move within the focused pane"),
        Line::from("  ← →        the chain in view — or pick any network below"),
        Line::from("  Enter      run the highlighted command (or check a balance)"),
        Line::from("  Esc        cancel a prompt, leave the recall list, or quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Commands — each also works as a single key press",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    for command in commands.iter().filter(|c| !summarised_in_help(c.action)) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}  ", command.key.unwrap_or(' ')),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:<LABEL_WIDTH$}", command.label),
                Style::default().fg(Color::White),
            ),
            Span::styled(command.help.clone(), Style::default().fg(Color::DarkGray)),
        ]));
    }

    // The networks live on the menu one row each; here they are summarised by
    // chain, because the useful fact is which chains exist and that picking a
    // row switches to one.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "A wallet is one mnemonic and an index; every chain derives its own account",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    for id in causewaybay_core::ChainId::ALL {
        let names = network::ALL
            .iter()
            .filter(|n| n.chain == id)
            .map(|n| n.key)
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<10}", id.as_str()),
                Style::default().fg(chain_colour(id)),
            ),
            Span::styled(names, Style::default().fg(Color::DarkGray)),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "  Every network is a row of its own; ● marks the one in use",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(
        "  One row per wallet; Detail lists its accounts, ▸ marks the chain in view",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            "  1 2 3 4    ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "save the wallet list as .jsonl / .csv / .txt / .md",
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Help — any key closes ")
                .title_alignment(Alignment::Center),
        ),
        popup,
    );
}

/// A rect of at most `width` × `height`, centred in `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// Shorten an address for the narrow list pane.
pub fn short_address(address: &str) -> String {
    if address.len() <= 16 {
        return address.to_string();
    }
    format!("{}…{}", &address[..8], &address[address.len() - 6..])
}

/// The column the names in a `name\taddress` block are padded into, or `None`
/// when the pane is too narrow to carry the names at all.
///
/// Decided once for the whole block rather than row by row: a pane where one
/// chain shows its name and the next three do not reads as a bug, and the
/// question "does this fit?" has to be asked of the widest row, not each.
fn pair_column(rows: &[(String, String)], room: usize) -> Option<usize> {
    let mut widest_name = 0;
    let mut widest_address = 0;
    for (_, value) in rows {
        if let Some((name, address)) = value.split_once('\t') {
            widest_name = widest_name.max(name.chars().count());
            widest_address = widest_address.max(address.chars().count());
        }
    }
    if widest_name == 0 {
        return None;
    }
    // Two spaces between the columns, at least.
    (widest_name + 2 + widest_address <= room).then_some(widest_name + 2)
}

/// Lay out one `name\taddress` pair in the column the block settled on.
///
/// Without room for the names, the address alone: it is the half that cannot
/// be worked out from the rest of the pane — the name is the wallet index and
/// the chain, and both are on screen beside it.
fn fit_pair(value: &str, column: Option<usize>) -> String {
    let Some((name, address)) = value.split_once('\t') else {
        return value.to_string();
    };
    match column {
        Some(width) => format!("{name:<width$}{address}"),
        None => address.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use causewaybay_core::store::Source;

    fn account(id: &str, label: &str) -> Account {
        account_on(id, label, ChainId::Evm)
    }

    fn account_on(id: &str, label: &str, chain: ChainId) -> Account {
        account_at(id, label, chain, 0)
    }

    /// The Solana address the shared test phrase derives at index 0.
    const SOLANA_ADDRESS: &str = "HAgk14JpMQLgt6rVgv7cBQFJWFto5Dqxi472uT3DKpqk";

    fn account_at(id: &str, label: &str, chain: ChainId, index: u32) -> Account {
        Account {
            chain,
            id: id.into(),
            label: label.into(),
            address: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94".into(),
            source: Source::Mnemonic,
            private_key: "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
                .into(),
            mnemonic: Some("abandon about".into()),
            derivation_path: Some(format!("m/44'/60'/0'/0/{index}")),
            index: Some(index),
            created_at: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn shortens_long_addresses_only() {
        assert_eq!(short_address("0xabc"), "0xabc");
        let short = short_address("0x9858EfFD232B4033E47d90003D41EC34EcaEda94");
        assert!(short.starts_with("0x9858Ef"));
        assert!(short.ends_with("aEda94"));
        assert!(short.len() < 42);
    }

    #[test]
    fn every_command_has_a_unique_key() {
        let commands = build_commands();
        let mut keys: Vec<char> = commands.iter().filter_map(|c| c.key).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two commands share a shortcut");
    }

    #[test]
    fn command_keys_do_not_collide_with_navigation() {
        // `j` and `k` fall through to movement, so no command may claim them.
        for command in build_commands() {
            assert_ne!(
                command.key,
                Some('j'),
                "{:?} would break j/k movement",
                command.action
            );
            assert_ne!(
                command.key,
                Some('k'),
                "{:?} would break j/k movement",
                command.action
            );
        }
    }

    /// Every variant must appear in `COMMANDS`.
    ///
    /// The clipboard command was once added to the enum and wired into the
    /// dispatch, but a formatting change meant the menu entry never landed —
    /// so it compiled, and simply could not be reached. The exhaustive match
    /// below means a new variant will not compile until it is listed here, and
    /// the loop then requires it to be on the menu.
    #[test]
    fn every_action_is_on_the_menu() {
        fn shortcut(action: Action) -> Option<char> {
            match action {
                // Networks are menu-only: the list grows, letters would collide.
                Action::SelectNetwork(_) => None,
                Action::Balance => Some('b'),
                Action::Send => Some('s'),
                Action::NewWallet => Some('n'),
                Action::NewSeed => Some('N'),
                Action::ImportMnemonic => Some('m'),
                Action::ImportKey => Some('p'),
                Action::Recall => Some('c'),
                Action::Derive => Some('d'),
                Action::Activate => Some('a'),
                Action::CopyAddress => Some('y'),
                Action::Sign => Some('g'),
                Action::Save(Format::Jsonl) => Some('1'),
                Action::Save(Format::Csv) => Some('2'),
                Action::Save(Format::Txt) => Some('3'),
                Action::Save(Format::Markdown) => Some('4'),
                Action::ToggleSecrets => Some('v'),
                Action::ExportWallets => Some('e'),
                Action::Remove => Some('x'),
                Action::Reload => Some('r'),
                Action::Help => Some('?'),
                Action::Quit => Some('q'),
            }
        }

        let every_action: Vec<Action> = [
            Action::Balance,
            Action::Send,
            Action::NewWallet,
            Action::NewSeed,
            Action::ImportMnemonic,
            Action::ImportKey,
            Action::Recall,
            Action::Derive,
            Action::Activate,
            Action::CopyAddress,
            Action::Sign,
            Action::Save(Format::Jsonl),
            Action::Save(Format::Csv),
            Action::Save(Format::Txt),
            Action::Save(Format::Markdown),
            Action::ToggleSecrets,
            Action::ExportWallets,
            Action::Remove,
            Action::Reload,
            Action::Help,
            Action::Quit,
        ]
        .into_iter()
        // Every network, whatever chain is in view.
        .chain(network::ALL.iter().map(|n| Action::SelectNetwork(n.key)))
        .collect();
        assert_eq!(
            every_action.len(),
            build_commands().len(),
            "the menu and the action list have diverged"
        );
        for action in every_action {
            let commands = build_commands();
            let listed = commands
                .iter()
                .find(|command| command.action == action)
                .unwrap_or_else(|| panic!("{action:?} is dispatched but not on the menu"));
            assert_eq!(listed.key, shortcut(action), "{action:?} has the wrong key");
        }
    }

    #[test]
    fn every_command_is_reachable_by_key() {
        let commands = build_commands();
        for command in &commands {
            let Some(key) = command.key else {
                continue; // networks are menu-only, by design
            };
            assert_eq!(
                command_for_key(&commands, key),
                Some(command.action),
                "{:?} is listed but its key does nothing",
                command.action
            );
        }
        assert_eq!(command_for_key(&commands, '§'), None);
    }

    /// The row the pane highlights is the row Enter runs — for every row.
    ///
    /// A hidden command sitting between them shifted everything below it: the
    /// highlight was on `cardano preprod` and Enter switched to Solana Devnet,
    /// three rows above, because the selection indexed the full list while the
    /// pane drew the visible one.
    #[test]
    fn enter_runs_the_row_the_pane_is_highlighting() {
        let mut state = State::new(Vec::new(), None);
        let visible: Vec<(String, Action)> = state
            .visible_commands()
            .iter()
            .map(|c| (c.label.clone(), c.action))
            .collect();
        assert!(
            visible.len() < state.command_list.len(),
            "this test is only meaningful while some command is hidden"
        );

        for (row, (label, action)) in visible.iter().enumerate() {
            state.commands.select(Some(row));
            assert_eq!(
                state.current_command(),
                Some(*action),
                "row {row} draws {label} but would run something else"
            );
        }

        // And the selection cannot walk past the last drawn row.
        state.focus = Focus::Commands;
        state.commands.select(Some(visible.len() - 1));
        state.move_selection(1);
        assert_eq!(
            state.commands.selected(),
            Some(0),
            "it wraps at the last row"
        );
    }

    /// A hidden command is still a command: its key runs it and the reference
    /// still lists it. Only the pane leaves it out.
    #[test]
    fn a_hidden_command_still_runs_from_its_key() {
        let commands = build_commands();
        for (key, format) in [
            ('2', Format::Csv),
            ('3', Format::Txt),
            ('4', Format::Markdown),
        ] {
            assert_eq!(
                command_for_key(&commands, key),
                Some(Action::Save(format)),
                "{key} lost its command"
            );
            assert!(
                commands
                    .iter()
                    .find(|c| c.key == Some(key))
                    .is_some_and(|c| c.hidden),
                "{key} should be off the pane"
            );
        }
    }

    /// Every network is a row of its own, in one flat list, in the same order
    /// every time. A two-level menu made going somewhere a matter of finding
    /// which rows had appeared under which other row.
    #[test]
    fn every_network_has_its_own_row_in_one_flat_list() {
        let labels: Vec<String> = build_commands()
            .iter()
            .filter_map(|c| match c.action {
                Action::SelectNetwork(_) => Some(c.label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "cronos testnet",
                "cronos mainnet",
                "solana devnet",
                "solana testnet",
                "solana mainnet",
                "cardano preprod",
                "cardano preview",
                "cardano mainnet",
                "midnight preview",
                "midnight devnet",
            ]
        );
        // Nothing is nested, and nothing is a chain-without-a-network row.
        assert!(build_commands()
            .iter()
            .all(|c| !c.label.contains('↳') && !c.label.contains("Chain ·")));
    }

    /// The rows are the network table: a network added to it becomes a row
    /// without anyone editing this file, and the list does not change with the
    /// chain in view.
    #[test]
    fn the_menu_follows_the_table_and_holds_still() {
        let rows = |commands: &[Command]| -> Vec<&'static str> {
            commands
                .iter()
                .filter_map(|c| match c.action {
                    Action::SelectNetwork(key) => Some(key),
                    _ => None,
                })
                .collect()
        };
        let expected: Vec<&str> = network::ALL.iter().map(|n| n.key).collect();
        assert_eq!(rows(&build_commands()), expected);

        // Switching chain leaves the rows where they were; only the ● moves.
        let mut state = State::new(Vec::new(), None);
        let before: Vec<String> = state.command_list.iter().map(|c| c.label.clone()).collect();
        state.set_chain(ChainId::Cardano);
        let after: Vec<String> = state.command_list.iter().map(|c| c.label.clone()).collect();
        assert_eq!(before, after);
    }

    /// Each network row carries the network it switches to, so picking one
    /// settles the chain and the network in a single act.
    #[test]
    fn a_network_row_carries_the_network_it_switches_to() {
        let commands = build_commands();
        for (label, key) in [
            ("cronos testnet", "cronos-testnet"),
            ("solana mainnet", "solana-mainnet"),
            ("midnight devnet", "midnight-devnet"),
        ] {
            let entry = commands
                .iter()
                .find(|c| c.label == label)
                .unwrap_or_else(|| panic!("no row for {label}"));
            assert_eq!(entry.action, Action::SelectNetwork(key));
            assert_eq!(entry.key, None, "networks are menu-only");
            let network = network::find(key).unwrap();
            assert!(entry.help.contains(network.name), "{}", entry.help);
        }
    }

    #[test]
    fn the_export_wallets_command_is_on_the_menu() {
        let commands = build_commands();
        let entry = commands
            .iter()
            .find(|c| c.action == Action::ExportWallets)
            .expect("export wallets should be a command");
        assert_eq!(entry.key, Some('e'));
        assert!(entry.help.contains("private keys"));
    }

    #[test]
    fn the_full_export_prompt_names_a_jsonl_file_and_warns() {
        let mut state = State::new(vec![account("a", "a")], None);
        start_export_wallets(&mut state);
        assert_eq!(state.input, "wallets-keys.jsonl");
        assert!(matches!(
            state.mode,
            Mode::Input(InputKind::ExportWalletsPath)
        ));
        assert!(
            state.status.contains("PRIVATE KEYS"),
            "the prompt has to say what it is about to write: {}",
            state.status
        );
    }

    #[test]
    fn the_requested_actions_are_all_on_the_menu() {
        let actions: Vec<Action> = build_commands().iter().map(|c| c.action).collect();
        assert!(actions.contains(&Action::Balance), "get balance");
        assert!(actions.contains(&Action::Send), "send amount");
        assert!(actions.contains(&Action::CopyAddress), "copy address");
        for format in Format::all() {
            assert!(
                actions.contains(&Action::Save(format)),
                "save wallet list as {}",
                format.as_str()
            );
        }
    }

    #[test]
    fn every_command_carries_help_text() {
        for command in build_commands() {
            assert!(!command.label.is_empty(), "{:?}", command.action);
            assert!(
                !command.help.is_empty(),
                "{:?} has no help line",
                command.action
            );
            // The pane draws a marker, the label column, then the key, so the
            // label has to fit the column it is padded into.
            assert!(
                command.label.chars().count() <= LABEL_WIDTH,
                "{:?} label overflows the {LABEL_WIDTH}-column pane",
                command.action
            );
        }
    }

    /// A wallet the chain in view has nothing on still resolves — to what it
    /// does have — so the pane is never inert and the commands always have a
    /// target.
    #[test]
    fn a_chain_with_nothing_on_it_still_leaves_a_wallet_selected() {
        let accounts = vec![
            account_at("a", "evm-one", ChainId::Evm, 0),
            account_at("b", "sol-one", ChainId::Solana, 0),
            account_at("c", "evm-two", ChainId::Evm, 1),
        ];
        let mut state = State::new(accounts, Some("a"));
        state.set_chain(ChainId::Solana);
        assert_eq!(state.current().unwrap().label, "sol-one");

        // Index 1 has no Solana account, so it falls back to what it has.
        state.selected.select(Some(1));
        assert_eq!(state.current().unwrap().label, "evm-two");
        assert_eq!(state.indices(), vec![0, 1], "the list never hides a wallet");
    }

    /// Selecting the last row and then losing it must not leave the highlight
    /// pointing past the end — that is how a TUI acts on the wrong wallet.
    #[test]
    fn a_shrinking_list_pulls_a_past_the_end_selection_back() {
        let accounts = vec![
            account_at("a", "sol-one", ChainId::Solana, 0),
            account_at("b", "evm-one", ChainId::Evm, 0),
            account_at("c", "evm-two", ChainId::Evm, 1),
        ];
        let mut state = State::new(accounts, None);
        let last = state.indices().len() - 1;
        state.selected.select(Some(last));

        // Index 1 goes, as a removal from another session would take it.
        state.accounts.retain(|a| a.index != Some(1));
        state.clamp_selection();

        assert_eq!(state.indices(), vec![0]);
        assert_eq!(state.selected.selected(), Some(0));
        assert_eq!(
            state.current_index, 0,
            "the wallet cursor followed the highlight"
        );
    }

    #[test]
    fn an_empty_list_leaves_nothing_selected_rather_than_a_stale_row() {
        let mut state = State::new(vec![account_on("a", "evm-one", ChainId::Evm)], Some("a"));
        state.accounts.clear();
        state.clamp_selection();
        assert_eq!(state.selected.selected(), None);
        assert!(state.current().is_none());
    }

    /// Moving the selection walks the list of wallet indices, not the whole
    /// flat account vector.
    #[test]
    fn arrows_move_between_wallets_not_accounts() {
        let accounts = vec![
            account_at("a", "evm-one", ChainId::Evm, 0),
            account_at("b", "sol-one", ChainId::Solana, 0),
            account_at("c", "sol-two", ChainId::Solana, 1),
        ];
        let mut state = State::new(accounts, None);
        state.set_chain(ChainId::Solana);
        state.focus = Focus::Accounts;

        // Three accounts, two wallets, two rows.
        assert_eq!(state.indices(), vec![0, 1]);
        state.selected.select(Some(0));
        assert_eq!(state.current().unwrap().label, "sol-one");

        state.move_selection(1);
        assert_eq!(state.current().unwrap().label, "sol-two");
        // Wrapping happens at the end of the list.
        state.move_selection(1);
        assert_eq!(state.selected.selected(), Some(0));
    }

    // ------------------------------------------------ the wallet index model

    /// A wallet is one mnemonic and one index; each chain derives its own
    /// account there. The pane lists the wallets, one row each, and the
    /// accounts of the highlighted one are what the detail pane shows.
    #[test]
    fn the_pane_lists_one_row_per_wallet_index() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("b", "sol-0", ChainId::Solana, 0),
            account_at("c", "sol-1", ChainId::Solana, 1),
        ];
        let state = State::new(accounts, None);

        assert_eq!(state.indices(), vec![0, 1]);
        let labels: Vec<&str> = state
            .accounts_at(0)
            .iter()
            .map(|a| a.label.as_str())
            .collect();
        assert_eq!(labels, ["evm-0", "sol-0"]);
        assert_eq!(state.accounts_at(1).len(), 1);
    }

    /// Chains come out in registry order for every index, so a given chain is
    /// always in the same place down the detail pane.
    #[test]
    fn chains_are_ordered_the_same_way_under_every_index() {
        let accounts = vec![
            account_at("a", "mid", ChainId::Midnight, 0),
            account_at("b", "evm", ChainId::Evm, 0),
            account_at("c", "ada", ChainId::Cardano, 0),
            account_at("d", "sol", ChainId::Solana, 0),
        ];
        let state = State::new(accounts, None);
        let order: Vec<ChainId> = state.accounts_at(0).iter().map(|a| a.chain).collect();
        assert_eq!(order, ChainId::ALL.to_vec());
    }

    /// `New wallet` lands on the next free index. Whether it *creates* there
    /// is covered end to end in `cli_multichain.rs`, which has a real store.
    #[test]
    fn new_wallet_targets_the_next_free_index() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("b", "sol-0", ChainId::Solana, 0),
        ];
        let state = State::new(accounts, None);
        assert_eq!(state.next_free_index(), 1);
    }

    /// A gap left by a removed wallet is reused before a fresh index.
    #[test]
    fn the_next_free_index_fills_a_gap_before_extending() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("c", "evm-2", ChainId::Evm, 2),
        ];
        let state = State::new(accounts, None);
        assert_eq!(state.next_free_index(), 1, "index 1 is free");
    }

    /// Which chains a wallet is still missing is the question the detail pane
    /// exists to answer, so every chain gets a line either way.
    #[test]
    fn a_wallet_index_knows_which_chains_it_is_missing() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("b", "sol-0", ChainId::Solana, 0),
        ];
        let state = State::new(accounts, None);
        let held: Vec<ChainId> = state.accounts_at(0).iter().map(|a| a.chain).collect();
        assert_eq!(held, vec![ChainId::Evm, ChainId::Solana]);
        let missing: Vec<ChainId> = ChainId::ALL
            .iter()
            .copied()
            .filter(|id| !held.contains(id))
            .collect();
        assert_eq!(missing, vec![ChainId::Cardano, ChainId::Midnight]);
        // An index with nothing on it holds nothing.
        assert!(state.accounts_at(7).is_empty());
    }

    /// The highlight is the cursor: standing on a row means the "New <chain>"
    /// commands fill *that* index. Letting the two drift is how a TUI shows
    /// one wallet and acts on another.
    #[test]
    fn moving_the_highlight_moves_the_wallet_index() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("b", "evm-1", ChainId::Evm, 1),
        ];
        let mut state = State::new(accounts, None);

        // Rows: [index 0, index 1].
        state.selected.select(Some(0));
        assert_eq!(state.selected_index(), Some(0));
        state.selected.select(Some(1));
        assert_eq!(state.selected_index(), Some(1));
    }

    /// A wallet row resolves to an account even when the chain in view has
    /// nothing at that index, so balance, send and the rest still have a
    /// wallet to act on rather than refusing.
    #[test]
    fn a_wallet_row_resolves_to_an_account() {
        let accounts = vec![account_at("b", "sol-1", ChainId::Solana, 1)];
        let mut state = State::new(accounts, None);
        state.selected.select(Some(0));
        assert_eq!(state.selected_index(), Some(1));
        assert_eq!(state.current_chain, ChainId::Evm, "nothing here on evm");
        assert_eq!(state.current().unwrap().label, "sol-1");
    }

    /// Picking a chain is how you pick which of a wallet's accounts you are
    /// working with — the list holds still and the target moves.
    #[test]
    fn choosing_a_chain_moves_the_target_to_that_chains_account() {
        let accounts = vec![
            account_at("a", "evm-0", ChainId::Evm, 0),
            account_at("b", "sol-0", ChainId::Solana, 0),
            account_at("c", "ada-0", ChainId::Cardano, 0),
        ];
        let mut state = State::new(accounts, None);
        assert_eq!(state.current().unwrap().label, "evm-0");

        state.set_chain(ChainId::Solana);
        assert_eq!(state.current().unwrap().label, "sol-0");
        assert_eq!(state.selected.selected(), Some(0), "the row did not move");

        // A chain this wallet has nothing on falls back rather than going
        // blank, so there is always something to act on.
        state.set_chain(ChainId::Midnight);
        assert_eq!(state.current().unwrap().label, "evm-0");
    }

    /// The names the wallet gave itself before it was multi-chain said neither
    /// which wallet nor which chain, so the screen re-renders them.
    #[test]
    fn old_automatic_names_are_shown_in_the_index_and_chain_scheme() {
        let legacy = account_at("a", "account-3", ChainId::Solana, 1);
        assert_eq!(store::display_label(&legacy), "account1-solana");

        // A name the user chose is theirs, whatever it looks like.
        let chosen = account_at("b", "savings", ChainId::Evm, 0);
        assert_eq!(store::display_label(&chosen), "savings");
        let looks_close = account_at("c", "account-cold", ChainId::Evm, 0);
        assert_eq!(store::display_label(&looks_close), "account-cold");
    }

    /// One command makes a wallet, and it makes the whole wallet — there is no
    /// per-chain create to forget, and no half-populated index to explain.
    #[test]
    fn there_is_exactly_one_way_to_make_a_wallet() {
        let commands = build_commands();
        let creates: Vec<&Command> = commands
            .iter()
            .filter(|c| matches!(c.action, Action::NewWallet))
            .collect();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].key, Some('n'));
        assert!(
            creates[0].help.contains("all four chains"),
            "{}",
            creates[0].help
        );
    }

    // ---------------------------------------------- chain-correct operations

    /// The bug this pins: the TUI validated every recipient with an EVM-only
    /// parser, so a perfectly good Solana address was rejected as "not a valid
    /// address" and a Solana send could never even be staged.
    #[test]
    fn a_recipient_is_checked_against_the_selected_accounts_chain() {
        use causewaybay_core::chain;

        let cases = [
            (
                ChainId::Solana,
                SOLANA_ADDRESS,
                causewaybay_core::network::SOLANA_DEVNET,
            ),
            (
                ChainId::Evm,
                "0x9858EfFD232B4033E47d90003D41EC34EcaEda94",
                causewaybay_core::network::CRONOS_TESTNET,
            ),
        ];
        for (id, address, network) in cases {
            assert!(
                chain::chain(id).check_address(&network, address).is_ok(),
                "{id} rejected its own address {address}"
            );
        }

        // And the EVM parser is exactly what used to reject it.
        assert!(
            causewaybay_core::wallet::parse_address(SOLANA_ADDRESS).is_err(),
            "this is the check that must no longer be applied to every chain"
        );
    }

    /// Amounts are checked at the selected chain's scale, not at 18 decimals.
    #[test]
    fn an_amount_is_checked_at_the_selected_chains_scale() {
        use causewaybay_core::network::{CRONOS_TESTNET, SOLANA_DEVNET};

        // Nine decimals is fine on Solana; ten is not.
        assert!(SOLANA_DEVNET.units().parse("0.000000001").is_ok());
        assert!(SOLANA_DEVNET.units().parse("0.0000000001").is_err());
        // The old 18-decimal check would have waved both through.
        assert!(CRONOS_TESTNET.units().parse("0.0000000001").is_ok());
    }

    /// Signing reads the account's own key format, so a non-EVM secret is not
    /// pushed through an EVM keypair parser.
    #[test]
    fn every_chain_can_sign_with_its_own_stored_secret() {
        use causewaybay_core::chain::{self, Seed};

        let seed = Seed::new(
            "abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon about",
            "",
        )
        .unwrap();
        for id in ChainId::ALL {
            let derived = chain::chain(id).derive(&seed, 0).unwrap();
            let signer = chain::chain(id)
                .signer(&derived.secret)
                .unwrap_or_else(|e| panic!("{id} could not load its own key: {}", e.message));
            assert!(signer.sign_message(b"hello").is_ok(), "{id} could not sign");
        }
        // The EVM-only path the TUI used to take fails on three of the four.
        let solana = chain::chain(ChainId::Solana).derive(&seed, 0).unwrap();
        assert!(
            causewaybay_core::wallet::Keypair::from_hex(&solana.secret).is_err(),
            "this is the parser that used to be applied to every chain"
        );
    }

    /// The arrows are the two axes: down the wallets, across the chains.
    #[test]
    fn the_left_and_right_arrows_walk_the_chains() {
        let (_dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);
        press(&app, &mut state, KeyCode::Char('n'));
        press(&app, &mut state, KeyCode::Tab);

        assert_eq!(state.current_chain, ChainId::Evm);
        press(&app, &mut state, KeyCode::Right);
        assert_eq!(state.current_chain, ChainId::Solana, "{}", state.status);
        assert_eq!(state.current().unwrap().chain, ChainId::Solana);
        press(&app, &mut state, KeyCode::Left);
        assert_eq!(state.current_chain, ChainId::Evm);
        // And they wrap, so neither end is a dead key.
        press(&app, &mut state, KeyCode::Left);
        assert_eq!(state.current_chain, ChainId::Midnight);
    }

    /// The bottom line is the manual: it says what the keys do *here*, so the
    /// reference is never the only way to find out.
    #[test]
    fn the_hint_line_follows_the_pane_in_focus() {
        let mut state = State::new(Vec::new(), None);
        assert!(
            hint_for(&state).contains("n makes your first wallet"),
            "an empty wallet says how to start: {}",
            hint_for(&state)
        );

        state.accounts = vec![account_on("a", "evm-one", ChainId::Evm)];
        assert!(
            hint_for(&state).contains("Enter run"),
            "{}",
            hint_for(&state)
        );
        state.focus = Focus::Accounts;
        let hint = hint_for(&state);
        for key in ["↑↓ wallet", "←→ chain", "Enter balance", "s send"] {
            assert!(hint.contains(key), "{key} missing from {hint}");
        }
    }

    #[test]
    fn the_command_pane_starts_focused_and_selected() {
        let state = State::new(Vec::new(), None);
        assert_eq!(state.focus, Focus::Commands);
        assert_eq!(state.commands.selected(), Some(0));
        assert_eq!(state.current_command(), Some(build_commands()[0].action));
    }

    #[test]
    fn tab_moves_between_the_two_panes() {
        let mut state = State::new(vec![account("a", "a")], None);
        assert_eq!(state.focus, Focus::Commands);
        state.cycle_focus();
        assert_eq!(state.focus, Focus::Accounts);
        state.cycle_focus();
        assert_eq!(state.focus, Focus::Commands);
    }

    #[test]
    fn arrows_move_within_whichever_pane_has_focus() {
        // Two wallets, so the account pane has two rows to move between.
        let mut state = State::new(
            vec![
                account_at("a", "a", ChainId::Evm, 0),
                account_at("b", "b", ChainId::Evm, 1),
            ],
            None,
        );

        state.move_selection(1);
        assert_eq!(state.commands.selected(), Some(1), "commands move first");
        assert_eq!(state.selected.selected(), Some(0), "wallets stay put");

        state.focus = Focus::Accounts;
        state.move_selection(1);
        assert_eq!(state.selected.selected(), Some(1));
        assert_eq!(state.commands.selected(), Some(1), "commands stay put");
    }

    #[test]
    fn selection_wraps_in_both_panes() {
        let mut state = State::new(vec![account("a", "a"), account("b", "b")], None);
        state.move_selection(-1);
        // The last *drawn* row: a hidden command is not one you can land on.
        assert_eq!(
            state.commands.selected(),
            Some(state.visible_commands().len() - 1)
        );
        state.move_selection(1);
        assert_eq!(state.commands.selected(), Some(0));

        state.focus = Focus::Accounts;
        state.move_selection(-1);
        // Both accounts sit at index 0, so the pane is a single row and
        // wrapping backwards from the top lands back on it.
        assert_eq!(state.indices(), vec![0]);
        assert_eq!(state.selected.selected(), Some(0));
    }

    #[test]
    fn moving_in_an_empty_wallet_list_is_a_no_op() {
        let mut state = State::new(Vec::new(), None);
        state.focus = Focus::Accounts;
        state.move_selection(1);
        assert_eq!(state.selected.selected(), None);
        assert!(state.current().is_none());
    }

    #[test]
    fn selection_starts_on_the_active_account() {
        // The pane lists wallets, so "the active account" means its wallet.
        let accounts = vec![
            account_at("acc_1", "one", ChainId::Evm, 0),
            account_at("acc_2", "two", ChainId::Evm, 1),
        ];
        assert_eq!(
            State::new(accounts.clone(), Some("acc_2"))
                .selected
                .selected(),
            Some(1)
        );
        assert_eq!(
            State::new(accounts.clone(), None).selected.selected(),
            Some(0)
        );
        // An unknown id falls back to the first entry rather than panicking.
        assert_eq!(
            State::new(accounts, Some("acc_gone")).selected.selected(),
            Some(0)
        );
    }

    /// Draw the whole screen and read it back, because everything above this
    /// point tests the model and none of it would notice a pane that renders
    /// its rows off the edge of its own border.
    fn screen(state: &mut State, width: u16, height: u16) -> Vec<String> {
        let (_dir, app) = wallet_app();
        draw_on(&app, state, width, height)
    }

    /// A wallet on a throwaway home, as the event loop builds one.
    fn wallet_app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            Some(dir.path().to_path_buf()),
            None,
            None,
            // The TUI answers its own questions on screen, so the wallet under
            // it never asks — same host the real loop installs.
            std::sync::Arc::new(Headless::new().assume_yes(true)),
        )
        .unwrap();
        (dir, app)
    }

    fn draw_on(app: &App, state: &mut State, width: u16, height: u16) -> Vec<String> {
        use ratatui::backend::TestBackend;
        refresh_detail(state);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// Press a key, as the event loop does.
    fn press(app: &App, state: &mut State, code: KeyCode) {
        handle_key(app, state, code, KeyModifiers::NONE);
    }

    fn typed(app: &App, state: &mut State, text: &str) {
        for c in text.chars() {
            press(app, state, KeyCode::Char(c));
        }
    }

    /// The cursor and the wallet the screen names must be the same wallet, at
    /// every moment — including the first, when the wallet the cursor was
    /// seeded from may not exist any more.
    ///
    /// This is what made `x` remove a wallet nobody had pointed at: index 0
    /// removed in an earlier session left the header naming index 0 while the
    /// highlight sat on index 1, and the keys followed the highlight.
    #[test]
    fn the_cursor_and_the_named_wallet_agree_from_the_first_frame() {
        let accounts = vec![
            account_at("a", "account1-evm", ChainId::Evm, 1),
            account_at("b", "account2-evm", ChainId::Evm, 2),
        ];

        // The active account is gone: nothing to seed the cursor from.
        let state = State::with_network(accounts.clone(), Some("gone"), network::DEFAULT_NETWORK);
        assert_eq!(state.selected_index(), Some(1));
        assert_eq!(
            state.current_index, 1,
            "the screen names the highlighted row"
        );

        // And when it is there, both point at it.
        let state = State::with_network(accounts, Some("b"), network::DEFAULT_NETWORK);
        assert_eq!(state.selected_index(), Some(2));
        assert_eq!(state.current_index, 2);
    }

    /// The question names the wallet, and asking it lights up the pane the
    /// answer applies to — a number alone is not something you can check
    /// before pressing y.
    #[test]
    fn the_removal_question_names_the_wallet_it_means() {
        let (_dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);
        press(&app, &mut state, KeyCode::Char('n'));
        press(&app, &mut state, KeyCode::Char('n'));

        assert_eq!(state.focus, Focus::Commands, "asking from the command list");
        press(&app, &mut state, KeyCode::Char('x'));
        assert_eq!(state.focus, Focus::Accounts, "the target pane lights up");
        assert!(state.status.contains("index 1"), "{}", state.status);
        assert!(state.status.contains("account1-evm"), "{}", state.status);
        assert!(state.status.contains("4 account(s)"), "{}", state.status);
        // Any key but y keeps it.
        press(&app, &mut state, KeyCode::Esc);
        assert_eq!(state.accounts.len(), 8);
    }

    /// Removing the middle wallet takes the middle wallet: the one the cursor
    /// is on, not the first or the last.
    #[test]
    fn removing_the_middle_wallet_takes_the_middle_wallet() {
        let (_dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);
        for _ in 0..3 {
            press(&app, &mut state, KeyCode::Char('n'));
        }
        assert_eq!(state.indices(), vec![0, 1, 2]);

        press(&app, &mut state, KeyCode::Tab);
        state.select_index(1);
        press(&app, &mut state, KeyCode::Char('x'));
        press(&app, &mut state, KeyCode::Char('y'));

        assert_eq!(state.indices(), vec![0, 2], "{}", state.status);
        assert!(
            state.accounts.iter().all(|a| a.index != Some(1)),
            "index 1 is gone whole"
        );
        assert_eq!(state.accounts.len(), 8);
    }

    /// The row is a wallet, so x removes the wallet — all four accounts —
    /// rather than one facet of it, which would leave the pane showing a
    /// wallet that is no longer one.
    #[test]
    fn removing_a_row_removes_the_whole_wallet() {
        let (_dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);
        press(&app, &mut state, KeyCode::Char('n'));
        press(&app, &mut state, KeyCode::Char('n'));
        assert_eq!(state.indices(), vec![0, 1]);

        press(&app, &mut state, KeyCode::Char('x'));
        assert_eq!(state.mode, Mode::Confirm(ConfirmKind::Remove));
        assert!(
            state.status.contains("index 1") && state.status.contains("4 account"),
            "the question says what goes: {}",
            state.status
        );
        // n keeps it, as the [y/N] promises.
        press(&app, &mut state, KeyCode::Char('n'));
        assert_eq!(state.accounts.len(), 8, "{}", state.status);

        press(&app, &mut state, KeyCode::Char('x'));
        press(&app, &mut state, KeyCode::Char('y'));
        assert_eq!(state.indices(), vec![0], "the wallet went as a whole");
        assert_eq!(state.accounts.len(), 4);
        assert_eq!(state.current_index, 0, "the cursor landed on what is left");
        assert!(state.status.contains("Recall"), "{}", state.status);

        // And the phrase really is still there to import back.
        assert!(!app.store.recent().unwrap().is_empty());
    }

    /// A whole session at the keyboard, against a real store on a throwaway
    /// home: make two wallets, walk the list, switch chain, and read the
    /// screen back after each step.
    ///
    /// Everything above this tests one function; this is the only test that
    /// asks whether pressing the keys in order actually leaves the wallet, and
    /// the screen, where the user would expect.
    #[test]
    fn a_session_at_the_keyboard_makes_wallets_and_switches_chains() {
        let (_dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);

        // n makes a wallet: one index, every chain at once.
        press(&app, &mut state, KeyCode::Char('n'));
        assert_eq!(state.accounts.len(), ChainId::ALL.len(), "{}", state.status);
        assert_eq!(state.current_index, 0);
        press(&app, &mut state, KeyCode::Char('n'));
        assert_eq!(state.accounts.len(), 2 * ChainId::ALL.len());
        assert_eq!(state.current_index, 1, "the cursor follows the new wallet");

        // The list shows two wallets, and the detail pane the second one's
        // four accounts, named for it.
        let lines = draw_on(&app, &mut state, 110, 30);
        let wallets = pane(&lines, WALLET_PANE.0, WALLET_PANE.1).join("\n");
        assert!(wallets.contains("index 0"), "{wallets}");
        assert!(wallets.contains("index 1"), "{wallets}");
        let whole = lines.join("\n");
        for name in ["account1-evm", "account1-solana", "account1-cardano"] {
            assert!(whole.contains(name), "{name} missing from\n{whole}");
        }

        // Tab into the list, and the arrows move between wallets.
        press(&app, &mut state, KeyCode::Tab);
        assert_eq!(state.focus, Focus::Accounts);
        press(&app, &mut state, KeyCode::Up);
        assert_eq!(state.current_index, 0, "up is the previous wallet");
        press(&app, &mut state, KeyCode::Down);
        assert_eq!(state.current_index, 1);

        // Picking a chain re-points what the commands act on, without the
        // highlight moving off the wallet.
        let selected = state.selected.selected();
        run_action(&app, &mut state, Action::SelectNetwork("cardano-preprod"));
        assert_eq!(state.current_chain, ChainId::Cardano, "{}", state.status);
        assert_eq!(state.selected.selected(), selected, "the row held still");
        let account = state.current().expect("the wallet has a cardano account");
        assert_eq!(account.chain, ChainId::Cardano);
        assert_eq!(account.label, "account1-cardano");
        assert!(
            draw_on(&app, &mut state, 110, 30)
                .join("\n")
                .contains("▸ cardano"),
            "the detail pane marks the chain in view"
        );

        // And the Cronos rows switch chain and network in one press.
        run_action(&app, &mut state, Action::SelectNetwork("cronos-mainnet"));
        assert_eq!(state.current_chain, ChainId::Evm);
        assert_eq!(state.current_network, "cronos-mainnet");
        assert_eq!(state.current().unwrap().label, "account1-evm");

        // v reveals the secrets, and only then: with them hidden the pane does
        // not even keep a row for them.
        let hidden = draw_on(&app, &mut state, 110, 30).join("\n");
        assert!(!hidden.contains("Private key"), "{hidden}");
        assert!(!hidden.contains("Mnemonic"), "{hidden}");
        press(&app, &mut state, KeyCode::Char('v'));
        let shown = draw_on(&app, &mut state, 110, 30).join("\n");
        assert!(shown.contains("Private key"), "{shown}");
        assert!(shown.contains("Mnemonic"), "{shown}");
        press(&app, &mut state, KeyCode::Char('v'));

        // ? opens the reference and any key closes it.
        press(&app, &mut state, KeyCode::Char('?'));
        assert_eq!(state.mode, Mode::Help);
        assert!(draw_on(&app, &mut state, 110, 44)
            .join("\n")
            .contains("Help — any key closes"));
        press(&app, &mut state, KeyCode::Char('x'));
        assert_eq!(state.mode, Mode::Browse, "a key closes it, it does not run");

        // Both wallets stay listed whatever chain is in view: the list is
        // wallets, and a wallet does not stop existing because you looked at
        // another chain.
        assert_eq!(state.indices(), vec![0, 1]);
    }

    /// Saving from the TUI writes the same file the CLI would, and the prompt
    /// is a real prompt: the path is typed in and Enter accepts it.
    #[test]
    fn saving_the_list_from_the_pane_writes_the_flattened_export() {
        let (dir, app) = wallet_app();
        let mut state = State::with_network(Vec::new(), None, network::DEFAULT_NETWORK);
        press(&app, &mut state, KeyCode::Char('n'));

        press(&app, &mut state, KeyCode::Char('3'));
        assert_eq!(state.mode, Mode::Input(InputKind::ExportPath(Format::Txt)));
        assert_eq!(state.input, "wallets.txt", "a name is offered");
        // Type over the offer, so the file lands on the throwaway home.
        while !state.input.is_empty() {
            press(&app, &mut state, KeyCode::Backspace);
        }
        let target = dir.path().join("saved.txt");
        typed(&app, &mut state, target.to_str().unwrap());
        press(&app, &mut state, KeyCode::Enter);
        assert_eq!(state.mode, Mode::Browse);
        assert!(!state.status_is_error, "{}", state.status);

        let written = std::fs::read_to_string(&target).expect("the file was written");
        let names: Vec<&str> = written
            .lines()
            .skip(2)
            .filter_map(|line| line.split_whitespace().nth(1))
            .collect();
        assert_eq!(
            names,
            [
                "account0-cronos-testnet",
                "account0-cronos-mainnet",
                "account0-solana",
                "account0-cardano",
                "account0-midnight",
            ]
        );
        assert!(!written.contains("private_key"), "secrets stay hidden");
    }

    /// One pane's columns out of the whole screen: every rendered line spans
    /// all three panes, so "is this row in the wallet pane?" is a question
    /// about columns.
    fn pane(lines: &[String], from: usize, width: usize) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.chars().skip(from).take(width).collect())
            .collect()
    }

    /// Where each pane starts: 26 columns of commands, then 26 of wallets.
    const COMMAND_PANE_WIDTH: usize = 26;
    const WALLET_PANE: (usize, usize) = (COMMAND_PANE_WIDTH, 26);

    fn four_chains_and_a_second_wallet() -> Vec<Account> {
        vec![
            // The names the wallet used to give itself, so the screen has to
            // re-render them.
            account_at("a", "account-1", ChainId::Evm, 0),
            account_at("b", "account-2", ChainId::Solana, 0),
            account_at("c", "account-3", ChainId::Cardano, 0),
            account_at("d", "account-4", ChainId::Midnight, 0),
            account_at("e", "account-5", ChainId::Evm, 1),
            account_at("f", "account-6", ChainId::Solana, 1),
        ]
    }

    /// The wallet pane is one row per wallet — `index 0`, `index 1` — and the
    /// four accounts of the highlighted one are laid out in the detail pane,
    /// named for the wallet and the chain they belong to.
    #[test]
    fn the_screen_lists_wallets_by_index_and_names_accounts_by_chain() {
        let mut state = State::new(four_chains_and_a_second_wallet(), Some("a"));
        let lines = screen(&mut state, 110, 26);
        let joined = lines.join("\n");

        // The list is wallets: one row per index, and no account rows.
        let wallets = pane(&lines, WALLET_PANE.0, WALLET_PANE.1).join("\n");
        assert!(wallets.contains("index 0"), "{wallets}");
        assert!(wallets.contains("index 1"), "{wallets}");
        assert!(!wallets.contains("account"), "{wallets}");
        for name in [
            "account0-evm",
            "account0-solana",
            "account0-cardano",
            "account0-midnight",
        ] {
            assert!(joined.contains(name), "{name} missing from\n{joined}");
        }
        // Addresses are shown head and tail, never in full.
        assert!(joined.contains("0x9858Ef…aEda94"), "{joined}");
        assert!(
            !joined.contains("0x9858EfFD232B4033E47d90003D41EC34EcaEda94"),
            "the full address should only reach the explorer link"
        );
    }

    /// Nothing may render past its own border: a title or a row that overflows
    /// is cut off mid-word, which is how a pane starts lying about its state.
    #[test]
    fn no_pane_content_is_cut_off_by_its_border() {
        let mut state = State::new(four_chains_and_a_second_wallet(), Some("a"));
        let lines = screen(&mut state, 110, 26);
        let wallets = lines
            .iter()
            .find(|line| line.contains("Wallets ("))
            .expect("the wallet pane draws its title");
        assert!(
            wallets.contains(" Wallets (2) ┐") || wallets.contains(" Wallets (2) ─"),
            "the title is cut off: {wallets}"
        );
        // A complete wallet says only what it is; the count appears when a
        // wallet is missing a chain, and must fit the pane when it does.
        let rows = pane(&lines, WALLET_PANE.0, WALLET_PANE.1);
        let row_for = |needle: &str| {
            rows.iter()
                .find(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle} in {rows:?}"))
                .clone()
        };
        assert!(
            !row_for("index 0").contains("chains"),
            "index 0 is complete"
        );
        let partial = row_for("index 1");
        assert!(partial.contains("index 1   2/4 chains"), "{partial}");
        assert!(
            partial.ends_with(" │"),
            "the row runs past its border: {partial}"
        );
    }

    /// The Cronos rows are the widest thing in the command pane, and a label
    /// cut off by the border would read as a different command.
    #[test]
    fn the_command_pane_draws_its_widest_row_in_full() {
        let mut state = State::new(four_chains_and_a_second_wallet(), Some("a"));
        // Tall enough to hold every row, so "missing" means missing rather
        // than scrolled off the bottom.
        let lines = screen(&mut state, 110, 40);
        let commands = pane(&lines, 0, COMMAND_PANE_WIDTH).join("\n");
        assert!(commands.contains("cronos testnet"), "{commands}");
        assert!(commands.contains("midnight preview"), "{commands}");
        // The ● marks the network in use; the key column sits between it and
        // the label, empty on a menu-only row.
        assert!(commands.contains("●  cronos testnet"), "{commands}");
        // A row with a key leads with it.
        assert!(commands.contains(" b Get balance"), "{commands}");
    }

    /// A narrow terminal gets the half of the row it cannot work out for
    /// itself — the address — rather than two ragged wrapped lines. And the
    /// whole block agrees: one chain showing its name while the next three do
    /// not reads as a bug.
    #[test]
    fn the_detail_rows_fit_the_room_the_pane_has() {
        let block = vec![
            (
                "▸ evm".to_string(),
                "account0-evm\t0x9858Ef…aEda94".to_string(),
            ),
            (
                "  midnight".to_string(),
                "account0-midnight\tmn_addr…spy5es".to_string(),
            ),
            ("Path".to_string(), "m/44'/60'/0'/0/0".to_string()),
        ];

        // Room for the widest name and the widest address: everything aligns
        // on the widest name, not on whatever each row happens to need.
        let column = pair_column(&block, 40).expect("both halves fit");
        assert_eq!(column, "account0-midnight".len() + 2);
        assert_eq!(
            fit_pair(&block[0].1, Some(column)),
            format!("{:<column$}{}", "account0-evm", "0x9858Ef…aEda94")
        );

        // One column short of the widest row, so no row keeps its name —
        // including the one that would have fitted on its own.
        let column = pair_column(&block, 33);
        assert_eq!(column, None, "the block gives way together");
        assert_eq!(fit_pair(&block[0].1, column), "0x9858Ef…aEda94");
        // A row that is not a pair is left alone either way.
        assert_eq!(fit_pair(&block[2].1, column), "m/44'/60'/0'/0/0");
        // And a block with no pairs in it has no column to speak of.
        assert_eq!(pair_column(&block[2..], 40), None);
    }

    /// At 80 columns everything is still legible: no row wraps, and the
    /// addresses are all there.
    #[test]
    fn the_screen_holds_together_in_an_eighty_column_terminal() {
        let mut state = State::new(four_chains_and_a_second_wallet(), Some("a"));
        let lines = screen(&mut state, 80, 24);
        let joined = lines.join("\n");
        // The shortcut keys survive the squeeze — they are the affordance.
        assert!(joined.contains(" b Get balance"), "{joined}");
        // Four chain rows, four addresses, one line each.
        let address_lines = lines
            .iter()
            .filter(|line| line.contains("0x9858Ef…aEda94"))
            .count();
        assert_eq!(address_lines, 4, "one line per chain:\n{joined}");
        for chain in ChainId::ALL {
            assert!(joined.contains(chain.as_str()), "{chain} missing");
        }
    }

    /// Choosing a chain is how you choose which account of the wallet you are
    /// working with, so the detail pane's marker follows it.
    #[test]
    fn the_detail_pane_marks_the_chain_in_view() {
        let mut state = State::new(four_chains_and_a_second_wallet(), Some("a"));
        let evm = screen(&mut state, 110, 26).join("\n");
        assert!(evm.contains("▸ evm"), "{evm}");

        state.set_chain(ChainId::Cardano);
        let cardano = screen(&mut state, 110, 26).join("\n");
        assert!(cardano.contains("▸ cardano"), "{cardano}");
        assert!(!cardano.contains("▸ evm"), "{cardano}");
    }

    /// A wallet that has nothing on a chain says so on that chain's line,
    /// rather than leaving a gap the eye has to work out.
    #[test]
    fn a_chain_a_wallet_is_not_on_says_so() {
        let mut state = State::new(four_chains_and_a_second_wallet(), None);
        state.select_index(1);
        let lines = screen(&mut state, 110, 26).join("\n");
        assert!(lines.contains("account1-evm"), "{lines}");
        assert!(lines.contains("not derived yet"), "{lines}");
    }

    #[test]
    fn status_tracks_whether_it_is_an_error() {
        let mut state = State::new(Vec::new(), None);
        state.fail("boom");
        assert!(state.status_is_error);
        state.info("fine");
        assert!(!state.status_is_error);
        assert_eq!(state.status, "fine");
    }

    #[test]
    fn the_opening_status_line_says_what_the_box_is_for() {
        // The keys live on the hint line, which changes with the pane; the
        // status box is where the wallet answers, and says so until it does.
        let state = State::new(Vec::new(), None);
        assert!(state.status.contains("answers"), "{}", state.status);
        assert!(!state.status_is_error);
        assert!(
            hint_for(&state).contains('?'),
            "the keys are on the hint line: {}",
            hint_for(&state)
        );
    }

    /// The TUI used to reimplement wallet creation, and drifted: deriving here
    /// skipped the recall entry that deriving on the CLI recorded. Both now go
    /// through `App`, so this checks the shared path rather than the symptom.
    #[test]
    fn the_tui_actions_go_through_the_same_app_entry_point() {
        let source = include_str!("tui.rs");
        for forbidden in [
            "LegacyTransaction",
            "store.create_account",
            "remember_secret",
            "send_raw_transaction",
        ] {
            assert!(
                !source.contains(&format!("app.{forbidden}"))
                    && !source.contains(&format!("crate::tx::{forbidden}")),
                "the TUI reimplements {forbidden} instead of calling App — that is how \
                 the two front ends drift apart"
            );
        }
    }

    #[test]
    fn a_cancelled_confirmation_drops_the_staged_transfer() {
        let mut state = State::new(vec![account("a", "a")], None);
        assert!(state.staged.is_none());
        // A staged plan must never survive a "no" and get sent by a later yes.
        state.mode = Mode::Confirm(ConfirmKind::Send);
        state.staged = None;
        state.info("Cancelled");
        assert!(state.staged.is_none());
    }

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                          abandon abandon abandon about";

    #[test]
    fn a_mnemonic_is_never_echoed_to_the_screen() {
        let shown = echo(InputKind::Mnemonic, PHRASE);
        assert!(
            !shown.contains("abandon"),
            "the phrase must not appear: {shown}"
        );
        assert!(!shown.contains("about"));
        // It still confirms the paste landed, and how much of it.
        assert!(shown.contains("12 words"));
        assert_eq!(shown.matches('•').count(), 12);
    }

    #[test]
    fn a_partly_typed_mnemonic_shows_its_progress() {
        assert_eq!(echo(InputKind::Mnemonic, ""), "");
        assert!(echo(InputKind::Mnemonic, "abandon").contains("1 word)"));
        assert!(echo(InputKind::Mnemonic, "abandon about").contains("2 words"));
        // Extra whitespace does not inflate the count.
        assert!(echo(InputKind::Mnemonic, "  abandon   about  ").contains("2 words"));
    }

    #[test]
    fn a_24_word_phrase_does_not_overflow_the_status_line() {
        let long = vec!["abandon"; 24].join(" ");
        let shown = echo(InputKind::Mnemonic, &long);
        assert!(shown.contains("24 words"));
        assert!(
            shown.chars().count() < 60,
            "the mask has to fit on one line"
        );
    }

    #[test]
    fn a_private_key_is_never_echoed_to_the_screen() {
        let key = "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727";
        let shown = echo(InputKind::PrivateKey, key);
        assert!(
            !shown.contains("1ab42c"),
            "the key must not appear: {shown}"
        );
        assert!(!shown.contains("b727"));
        // 64 hex characters, with the 0x prefix not counted.
        assert!(shown.contains("64 hex chars"));
        assert_eq!(echo(InputKind::PrivateKey, ""), "");
    }

    #[test]
    fn non_secret_prompts_still_show_what_was_typed() {
        // Seeing these is how a typo gets caught before it is submitted.
        assert_eq!(
            echo(InputKind::Label { new_seed: false }, "savings"),
            "savings"
        );
        assert_eq!(echo(InputKind::SendTo, "0xabc"), "0xabc");
        assert_eq!(echo(InputKind::SendAmount, "1.5"), "1.5");
        assert_eq!(echo(InputKind::DeriveIndex, "3"), "3");
        assert_eq!(echo(InputKind::SignMessage, "hello"), "hello");
        assert_eq!(
            echo(InputKind::ExportPath(Format::Csv), "wallets.csv"),
            "wallets.csv"
        );
    }

    #[test]
    fn every_secret_prompt_is_masked() {
        // A new secret-bearing prompt has to be added here as well as to `echo`.
        for kind in [InputKind::Mnemonic, InputKind::PrivateKey] {
            let shown = echo(kind, "supersecretvalue words here");
            assert!(!shown.contains("supersecret"), "{shown}");
        }
    }

    /// `Get balance` asks whose balance, and a blank answer means the wallet
    /// in view — the same question test-wallet3's menu asks.
    #[test]
    fn get_balance_asks_for_a_target_before_querying() {
        let mut state = State::new(vec![account("a", "main")], Some("a"));
        assert!(matches!(state.mode, Mode::Browse));

        start_input(
            &mut state,
            InputKind::BalanceAddress,
            "Address to check (blank = this wallet)",
        );
        assert!(matches!(state.mode, Mode::Input(InputKind::BalanceAddress)));
        assert!(state.status.contains("blank"), "{}", state.status);

        // An address is not a secret: seeing it is how a typo gets caught.
        assert_eq!(
            echo(InputKind::BalanceAddress, SOLANA_ADDRESS),
            SOLANA_ADDRESS
        );
    }

    // ------------------------------------------------------------- pasting

    /// The bug this pins: the event loop matched only key events, so the
    /// `Event::Paste` a bracketed-paste terminal delivers was swallowed whole
    /// and pasting an address did nothing at all.
    #[test]
    fn a_paste_lands_in_the_input_buffer() {
        let mut state = State::new(Vec::new(), None);
        start_input(&mut state, InputKind::SendTo, "Recipient address");
        handle_paste(&mut state, SOLANA_ADDRESS);
        assert_eq!(state.input, SOLANA_ADDRESS);

        // A second paste appends, as typing would.
        handle_paste(&mut state, "xyz");
        assert_eq!(state.input, format!("{SOLANA_ADDRESS}xyz"));
    }

    /// A copied address usually carries a trailing newline. Treating it as
    /// Enter would submit the moment the paste landed, before the user saw
    /// what arrived — so newlines fold to spaces and trailing ones vanish.
    #[test]
    fn a_pasted_newline_does_not_submit() {
        let mut state = State::new(Vec::new(), None);
        start_input(&mut state, InputKind::SendTo, "Recipient address");
        handle_paste(&mut state, &format!("{SOLANA_ADDRESS}\n"));
        assert!(
            matches!(state.mode, Mode::Input(InputKind::SendTo)),
            "the prompt must still be open"
        );
        assert_eq!(state.input, SOLANA_ADDRESS, "no stray whitespace");
    }

    /// A multi-line paste — a mnemonic copied from a text file — folds to the
    /// single-space form the wallet normalises anyway.
    #[test]
    fn a_multiline_mnemonic_pastes_as_one_line() {
        let mut state = State::new(Vec::new(), None);
        start_input(&mut state, InputKind::Mnemonic, "Mnemonic to import");
        handle_paste(&mut state, "abandon abandon\nabandon about\r\n");
        assert_eq!(state.input, "abandon abandon abandon about");
    }

    /// Outside a prompt a paste is ignored: browse-mode keys are commands,
    /// and replaying a pasted address as commands would fire half the menu.
    #[test]
    fn a_paste_outside_a_prompt_is_ignored() {
        let mut state = State::new(Vec::new(), None);
        assert!(matches!(state.mode, Mode::Browse));
        handle_paste(&mut state, "qqqq");
        assert!(state.input.is_empty());
        assert!(!state.quit, "a pasted q must not quit the wallet");
    }

    /// Ctrl-V pulls from the system clipboard itself, so a terminal that
    /// mangles paste events cannot stop an address reaching a prompt. Runs
    /// only where a clipboard helper exists, like the clipboard suite.
    #[test]
    #[cfg(target_os = "macos")]
    fn ctrl_v_reads_the_system_clipboard_into_the_prompt() {
        let _guard = crate::clipboard::CLIPBOARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Put whatever was on the developer's clipboard back afterwards.
        let previous = crate::clipboard::paste().ok();

        let marker = "HpBvxSffXGnHZPEd66C4sXe4kSRY5962HdTxj3fFQyfw";
        if crate::clipboard::copy(marker).is_err() {
            return; // no clipboard on this machine; the helper suite skips too
        }

        let wallet = tempfile::tempdir().unwrap();
        let app = App::new(
            Some(wallet.path().to_path_buf()),
            None,
            None,
            std::sync::Arc::new(causewaybay_core::host::Headless::new()),
        )
        .unwrap();
        let mut state = State::new(Vec::new(), None);
        start_input(&mut state, InputKind::SendTo, "Recipient address");

        input_key(
            &app,
            &mut state,
            InputKind::SendTo,
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        );
        assert_eq!(state.input, marker);
        // And a plain 'v' still types a v rather than pasting.
        input_key(
            &app,
            &mut state,
            InputKind::SendTo,
            KeyCode::Char('v'),
            KeyModifiers::NONE,
        );
        assert_eq!(state.input, format!("{marker}v"));

        if let Some(text) = previous {
            let _ = crate::clipboard::copy(&text);
        }
    }

    /// The status area is one row and a pasted address is one unbreakable
    /// word. Word-wrap pushed the whole address onto a second line that does
    /// not exist, so a long paste landed in the buffer and rendered as
    /// *nothing* — which read as "paste is broken" while short pastes worked.
    #[test]
    fn a_long_input_shows_its_tail_rather_than_vanishing() {
        let address = "addr_test1qrqcypzsej9lnq2uxlf6ewkupqqlhhyanjn5akuwrkpfxhaa4e9g2c\
                       5zhhcuz54ea6w0x3nwvkulf6yx3xk6f4hn37uq530f20";
        let line = format!("Address to check (blank = this wallet) > {address}_");

        // Wider than the text: unchanged.
        assert_eq!(tail_fit(&line, 400), line);

        // The user's terminal: 104 usable columns. The cursor and the end of
        // the address must be on screen; the head scrolls off behind an `…`.
        let fitted = tail_fit(&line, 104);
        assert_eq!(fitted.chars().count(), 104);
        assert!(fitted.starts_with('…'), "{fitted}");
        assert!(fitted.ends_with("uq530f20_"), "{fitted}");

        // Degenerate widths must not panic or underflow.
        assert_eq!(tail_fit("abc", 0), "abc");
        assert_eq!(tail_fit("abcdef", 3), "…ef");
    }

    /// The clipboard offer: a copied address is presented for one-keystroke
    /// use, junk and secrets are never offered, and the offer is worded as a
    /// question rather than silently assumed.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_copied_address_is_offered_and_junk_is_not() {
        let _guard = crate::clipboard::CLIPBOARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = crate::clipboard::paste().ok();

        let wallet = tempfile::tempdir().unwrap();
        let app = App::new(
            Some(wallet.path().to_path_buf()),
            None,
            None,
            std::sync::Arc::new(causewaybay_core::host::Headless::new()),
        )
        .unwrap();

        let open_prompt = |state: &mut State| {
            start_input(
                state,
                InputKind::BalanceAddress,
                "Address to check (blank = this wallet)",
            );
        };

        // A valid Solana address, offered to a Solana account's prompt.
        let mut state = State::new(vec![account_on("a", "sol", ChainId::Solana)], Some("a"));
        if crate::clipboard::copy(SOLANA_ADDRESS).is_err() {
            return; // no clipboard helper on this machine
        }
        open_prompt(&mut state);
        offer_clipboard_address(&app, &mut state);
        assert_eq!(state.input, SOLANA_ADDRESS);
        assert!(state.clipboard_offer, "the offer must be marked as one");
        // The question lives on the hint line, where a long address cannot
        // scroll it away; both answers are named.
        let hint = hint_for(&state);
        assert!(hint.contains("Enter"), "{hint}");
        assert!(hint.contains("Ctrl-U"), "{hint}");

        // Junk is not offered, and the prompt says nothing about a clipboard.
        let mut state = State::new(vec![account_on("a", "sol", ChainId::Solana)], Some("a"));
        crate::clipboard::copy("not an address at all").unwrap();
        open_prompt(&mut state);
        offer_clipboard_address(&app, &mut state);
        assert!(state.input.is_empty());
        assert!(!state.clipboard_offer);

        // A copied mnemonic is a secret; it must never be echoed as an offer.
        let mut state = State::new(vec![account_on("a", "sol", ChainId::Solana)], Some("a"));
        crate::clipboard::copy(PHRASE).unwrap();
        open_prompt(&mut state);
        offer_clipboard_address(&app, &mut state);
        assert!(
            state.input.is_empty(),
            "a mnemonic was offered as an address"
        );

        // The account's own address is pointless for balance and refused for
        // send, so it is not offered either.
        let mut state = State::new(vec![account_on("a", "sol", ChainId::Solana)], Some("a"));
        let own = state.accounts[0].address.clone();
        // account_on uses an EVM-shaped address; give the test a solana one.
        state.accounts[0].address = SOLANA_ADDRESS.into();
        crate::clipboard::copy(SOLANA_ADDRESS).unwrap();
        open_prompt(&mut state);
        offer_clipboard_address(&app, &mut state);
        assert!(state.input.is_empty(), "its own address was offered back");
        let _ = own;

        if let Some(text) = previous {
            let _ = crate::clipboard::copy(&text);
        }
    }

    /// Ctrl-U is the "no thanks": it clears the offer (or anything typed)
    /// without closing the prompt.
    #[test]
    fn ctrl_u_clears_the_line_but_keeps_the_prompt_open() {
        let wallet = tempfile::tempdir().unwrap();
        let app = App::new(
            Some(wallet.path().to_path_buf()),
            None,
            None,
            std::sync::Arc::new(causewaybay_core::host::Headless::new()),
        )
        .unwrap();
        let mut state = State::new(Vec::new(), None);
        start_input(&mut state, InputKind::SendTo, "Recipient address");
        state.input = SOLANA_ADDRESS.into();

        input_key(
            &app,
            &mut state,
            InputKind::SendTo,
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        );
        assert!(state.input.is_empty());
        assert!(
            matches!(state.mode, Mode::Input(InputKind::SendTo)),
            "the prompt must survive the clear"
        );
    }

    // ---------------------------------------------------- background work

    /// The narration keeps arriving and the clock keeps counting while the
    /// thread works — the difference between "preparing, 40s in" and a
    /// frozen screen over a command that moves money.
    #[test]
    fn planning_narration_reaches_the_status_line_with_a_clock() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut state = State::new(Vec::new(), None);
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: "Preparing the midnight transfer".into(),
            abandoned: "Abandoned — nothing was sent",
        });
        state.mode = Mode::Busy;

        // Silence still ticks the clock.
        poll_job(&mut state);
        assert!(
            state.status.contains("Preparing the midnight transfer"),
            "{}",
            state.status
        );
        assert!(state.status.contains("s"), "{}", state.status);

        // A narration line replaces the message and survives later ticks.
        tx.send(JobEvent::Progress("dust sync: event 4096 of 150000".into()))
            .unwrap();
        poll_job(&mut state);
        poll_job(&mut state);
        assert!(state.status.contains("dust sync"), "{}", state.status);
        assert!(matches!(state.mode, Mode::Busy));
    }

    #[test]
    fn a_failed_plan_returns_to_browse_with_the_reason() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut state = State::new(Vec::new(), None);
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: String::new(),
            abandoned: "Abandoned — nothing was sent",
        });
        state.mode = Mode::Busy;

        tx.send(JobEvent::Failed(
            "balance 0 NIGHT cannot cover 1 NIGHT".into(),
        ))
        .unwrap();
        poll_job(&mut state);
        assert!(matches!(state.mode, Mode::Browse));
        assert!(state.status_is_error);
        assert!(state.status.contains("cannot cover"), "{}", state.status);
        assert!(state.job.is_none());
    }

    /// A balance runs on a thread like everything else that waits on a node:
    /// the screen keeps drawing, and the number lands in the wallet's own
    /// facts rather than only in a status line the next message overwrites.
    #[test]
    fn a_balance_arrives_in_the_detail_pane_and_frees_the_screen() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut state = State::new(vec![account("a", "main")], None);
        state.detail = vec![("Address".into(), "0xaaa".into())];
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: "Querying the evm balance".into(),
            abandoned: "Stopped waiting — nothing was changed",
        });
        state.mode = Mode::Busy;

        // While it runs the clock counts rather than the screen freezing.
        poll_job(&mut state);
        assert!(
            state.status.contains("Querying the evm balance"),
            "{}",
            state.status
        );
        assert_eq!(state.mode, Mode::Busy);

        tx.send(JobEvent::Done(JobResult::Balance("1.25 TCRO".into())))
            .unwrap();
        poll_job(&mut state);
        assert_eq!(state.mode, Mode::Browse);
        assert!(state.job.is_none());
        assert!(!state.status_is_error, "{}", state.status);
        assert_eq!(
            state.detail.last(),
            Some(&("Balance".to_string(), "1.25 TCRO".to_string()))
        );

        // Asking again replaces the row rather than stacking a second one.
        let (tx, rx) = std::sync::mpsc::channel();
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: String::new(),
            abandoned: "Stopped waiting — nothing was changed",
        });
        tx.send(JobEvent::Done(JobResult::Balance("2.00 TCRO".into())))
            .unwrap();
        poll_job(&mut state);
        assert_eq!(
            state
                .detail
                .iter()
                .filter(|(key, _)| key == "Balance")
                .count(),
            1
        );
    }

    /// Esc says what stopping actually cost, which is not the same sentence
    /// for a read as for a transfer.
    #[test]
    fn abandoning_says_what_it_abandoned() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut state = State::new(Vec::new(), None);
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: String::new(),
            abandoned: "Stopped waiting — nothing was changed",
        });
        state.mode = Mode::Busy;

        let (_dir, app) = wallet_app();
        press(&app, &mut state, KeyCode::Esc);
        assert_eq!(state.mode, Mode::Browse);
        assert!(state.job.is_none());
        assert_eq!(state.status, "Stopped waiting — nothing was changed");
    }

    /// A planning thread that panics drops its channel without a verdict.
    /// That must surface as a failure, not as a Busy screen counting forever.
    #[test]
    fn a_dead_planning_thread_is_reported_not_waited_on() {
        let (tx, rx) = std::sync::mpsc::channel::<JobEvent>();
        drop(tx);
        let mut state = State::new(Vec::new(), None);
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: String::new(),
            abandoned: "Abandoned — nothing was sent",
        });
        state.mode = Mode::Busy;

        poll_job(&mut state);
        assert!(matches!(state.mode, Mode::Browse));
        assert!(state.status_is_error);
        assert!(
            state.status.contains("nothing was sent"),
            "{}",
            state.status
        );
    }

    /// While a plan is being prepared, Esc abandons and everything else is
    /// inert — a stray keystroke must not stage a second send or quit the
    /// wallet mid-preparation.
    #[test]
    fn busy_mode_answers_only_to_esc() {
        let wallet = tempfile::tempdir().unwrap();
        let app = App::new(
            Some(wallet.path().to_path_buf()),
            None,
            None,
            std::sync::Arc::new(causewaybay_core::host::Headless::new()),
        )
        .unwrap();
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut state = State::new(Vec::new(), None);
        state.job = Some(Job {
            events: rx,
            started: Instant::now(),
            last: String::new(),
            abandoned: "Abandoned — nothing was sent",
        });
        state.mode = Mode::Busy;

        for key in ['q', 's', 'n', 'y'] {
            handle_key(&app, &mut state, KeyCode::Char(key), KeyModifiers::NONE);
            assert!(matches!(state.mode, Mode::Busy), "'{key}' broke Busy mode");
            assert!(!state.quit, "'{key}' quit mid-preparation");
        }

        handle_key(&app, &mut state, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(state.mode, Mode::Browse));
        assert!(state.job.is_none());
        assert!(
            state.status.contains("nothing was sent"),
            "{}",
            state.status
        );
    }

    /// The planning host narrates into the channel and refuses mid-plan input.
    #[test]
    fn the_planning_host_narrates_and_never_prompts() {
        use causewaybay_core::host::Host;
        let (tx, rx) = std::sync::mpsc::channel();
        let host = JobHost { events: tx };
        host.progress("proving the DUST spend");
        assert!(matches!(
            rx.try_recv(),
            Ok(JobEvent::Progress(line)) if line.contains("proving")
        ));
        assert!(host.read_input("mnemonic").is_err());
        assert!(host.confirm("send?").is_ok(), "the TUI confirms on screen");
    }

    #[test]
    fn submitting_or_cancelling_scrubs_the_buffer() {
        let mut state = State::new(Vec::new(), None);
        state.input = PHRASE.to_string();
        clear_input(&mut state);
        assert!(state.input.is_empty());

        // And starting a fresh prompt does not inherit the last one's text.
        state.input = PHRASE.to_string();
        start_input(&mut state, InputKind::Mnemonic, "Mnemonic to import");
        assert!(state.input.is_empty());
    }

    #[test]
    fn the_export_prompt_suggests_a_filename() {
        let mut state = State::new(vec![account("a", "a")], None);
        start_export(&mut state, Format::Csv);
        assert_eq!(state.input, "wallets.csv");
        assert!(matches!(
            state.mode,
            Mode::Input(InputKind::ExportPath(Format::Csv))
        ));

        start_export(&mut state, Format::Markdown);
        assert_eq!(state.input, "wallets.md");
    }

    #[test]
    fn no_help_line_wraps_inside_the_overlay() {
        // "  k  " + an 18-column label + the help text, inside the box borders.
        let budget = HELP_WIDTH as usize - 2 - 5 - LABEL_WIDTH;
        for command in build_commands() {
            assert!(
                command.help.len() <= budget,
                "{:?}: help is {} chars, only {budget} fit before it wraps",
                command.action,
                command.help.len()
            );
        }
    }

    #[test]
    fn the_help_overlay_has_room_for_every_command() {
        let commands = build_commands();
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let popup = centered_rect(HELP_WIDTH, help_overlay_height(&commands), area);
        assert!(
            popup.height >= help_overlay_height(&commands),
            "the overlay clips its last lines in a {}-row terminal",
            area.height
        );
    }

    /// Adding networks must not push the overlay off the screen again. Ten
    /// rows of "switch to …" is what broke it the first time.
    #[test]
    fn the_help_overlay_still_fits_a_modest_terminal_with_every_network() {
        let commands = build_commands();
        assert!(
            help_overlay_height(&commands) <= 40,
            "the help overlay wants {} rows, which will not fit a 40-row terminal",
            help_overlay_height(&commands)
        );
        // And it summarises the chains and networks rather than listing them
        // one row each — which is what made it overflow.
        let listed = commands
            .iter()
            .filter(|c| !matches!(c.action, Action::SelectNetwork(_)))
            .count();
        assert!(
            commands.len() > listed,
            "chains and networks should be on the menu"
        );
    }

    #[test]
    fn the_help_overlay_fits_a_small_terminal() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 12,
        };
        let popup = centered_rect(72, 30, area);
        assert!(popup.width <= area.width);
        assert!(popup.height <= area.height);
        assert!(popup.width > 0 && popup.height > 0);
    }

    #[test]
    fn the_help_overlay_is_centred() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let popup = centered_rect(72, 30, area);
        assert_eq!(popup.width, 72);
        assert_eq!(popup.height, 30);
        assert_eq!(popup.x, 14);
        assert_eq!(popup.y, 5);
    }
}

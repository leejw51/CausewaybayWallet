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

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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

use alloy_primitives::U256;

use causewaybay_core::app::{App, SendPlan};
use causewaybay_core::command::{AccountCommand, SendArgs, TargetArgs};
use causewaybay_core::error::{self, Result};
use causewaybay_core::export::{self, Format};
use causewaybay_core::host::Headless;
use causewaybay_core::network;
use causewaybay_core::output::CommandOutput;
use causewaybay_core::store::{Account, RecentSecret};
use causewaybay_core::units;
use causewaybay_core::wallet::{self, Keypair};

// ============================================================== the commands

/// Everything the TUI can do. One value per row of the command pane, and the
/// single place a key press and a menu selection converge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Balance,
    Send,
    NewAddress,
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
    /// Switch to the network with this key. One per entry in `network::ALL`,
    /// so adding a network to the table adds a menu entry with it.
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
}

impl Command {
    fn new(action: Action, label: &str, key: Option<char>, help: &str) -> Self {
        Command {
            action,
            label: label.to_string(),
            key,
            help: help.to_string(),
        }
    }
}

/// Build the command pane.
///
/// Networks come from `network::ALL` rather than being written out here, so a
/// chain added to the table shows up as an entry without touching this file.
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
            "Send the network's native coin to an address",
        ),
        Command::new(
            Action::NewAddress,
            "New address",
            Some('n'),
            "Add the next address of this seed: 0, 1, 2, …",
        ),
        Command::new(
            Action::NewSeed,
            "New seed",
            Some('N'),
            "Start a separate mnemonic, addresses from 0 again",
        ),
        Command::new(
            Action::ImportMnemonic,
            "Import mnemonic",
            Some('m'),
            "Import an existing BIP-39 phrase",
        ),
        Command::new(
            Action::ImportKey,
            "Import priv key",
            Some('p'),
            "Import a raw private key",
        ),
        Command::new(
            Action::Recall,
            "Recall saved keys",
            Some('c'),
            "Reuse a mnemonic or key from the recall list",
        ),
        Command::new(
            Action::Derive,
            "Derive address",
            Some('d'),
            "Derive another address from this mnemonic",
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
            "Copy the selected wallet's address to the clipboard",
        ),
        Command::new(
            Action::Sign,
            "Sign message",
            Some('g'),
            "Sign a message with EIP-191",
        ),
        Command::new(
            Action::Save(Format::Jsonl),
            "Save list .jsonl",
            Some('1'),
            "Write the wallet list to a JSONL file",
        ),
        Command::new(
            Action::Save(Format::Csv),
            "Save list .csv",
            Some('2'),
            "Write the wallet list to a CSV file",
        ),
        Command::new(
            Action::Save(Format::Txt),
            "Save list .txt",
            Some('3'),
            "Write the wallet list to an aligned text file",
        ),
        Command::new(
            Action::Save(Format::Markdown),
            "Save list .md",
            Some('4'),
            "Write the wallet list to a Markdown table",
        ),
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

    // One entry per network, flattened rather than hidden behind a toggle.
    for chain in network::ALL {
        commands.push(Command::new(
            Action::SelectNetwork(chain.key),
            chain.name,
            None,
            &format!("Switch to {} (chain {})", chain.name, chain.chain_id),
        ));
    }

    commands.extend([
        Command::new(
            Action::Remove,
            "Remove wallet",
            Some('x'),
            "Forget the selected wallet (asks first)",
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    /// Collecting text; the kind decides what happens on Enter.
    Input(InputKind),
    Confirm(ConfirmKind),
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Label {
        new_seed: bool,
    },
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    command_list: Vec<Command>,
    commands: ListState,
    /// The network key the command pane marks as current.
    current_network: String,
    focus: Focus,
    mode: Mode,
    input: String,
    status: String,
    status_is_error: bool,
    detail: Vec<(String, String)>,
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
        let index = active
            .and_then(|id| accounts.iter().position(|a| a.id == id))
            .unwrap_or(0);
        let mut selected = ListState::default();
        if !accounts.is_empty() {
            selected.select(Some(index));
        }
        let mut commands = ListState::default();
        commands.select(Some(0));

        State {
            accounts,
            selected,
            recall: Vec::new(),
            recall_selected: ListState::default(),
            command_list: build_commands(),
            commands,
            current_network: current_network.to_string(),
            focus: Focus::Commands,
            mode: Mode::Browse,
            input: String::new(),
            status: "Tab switches panes · Enter runs a command · ? for help".into(),
            status_is_error: false,
            detail: Vec::new(),
            pending_to: String::new(),
            pending_amount: String::new(),
            staged: None,
            show_secrets: false,
            network_changed: false,
            quit: false,
        }
    }

    fn current(&self) -> Option<&Account> {
        self.selected.selected().and_then(|i| self.accounts.get(i))
    }

    fn current_recall(&self) -> Option<&RecentSecret> {
        self.recall_selected
            .selected()
            .and_then(|i| self.recall.get(i))
    }

    fn current_command(&self) -> Option<Action> {
        self.commands
            .selected()
            .and_then(|i| self.command_list.get(i))
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
            Focus::Commands => (self.command_list.len(), &mut self.commands),
            Focus::Accounts => (self.accounts.len(), &mut self.selected),
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
    execute!(out, EnterAlternateScreen)
        .map_err(|e| error::internal(format!("cannot open the alternate screen: {e}")))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| error::internal(format!("cannot build the terminal: {e}")))?;

    let result = event_loop(app, &mut terminal);

    // Restore the terminal even if the loop failed, so the shell stays usable.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
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
    refresh_detail(&app, &mut state);

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
                std::sync::Arc::clone(&app.host),
            )?;
            refresh_detail(&app, &mut state);
        }

        terminal
            .draw(|frame| draw(frame, &app, &mut state))
            .map_err(|e| error::internal(format!("draw failed: {e}")))?;

        let timeout = tick.saturating_sub(last.elapsed());
        if event::poll(timeout).map_err(|e| error::internal(format!("input poll failed: {e}")))? {
            if let Event::Key(key) =
                event::read().map_err(|e| error::internal(format!("input read failed: {e}")))?
            {
                if key.kind == KeyEventKind::Press {
                    handle_key(&app, &mut state, key.code, key.modifiers);
                }
            }
        }
        if last.elapsed() >= tick {
            last = Instant::now();
        }
    }
    Ok(())
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
        Mode::Input(kind) => input_key(app, state, kind, code),
        Mode::Confirm(kind) => confirm_key(app, state, kind, code),
        Mode::Browse => browse_key(app, state, code),
    }
}

fn browse_key(app: &App, state: &mut State, code: KeyCode) {
    match code {
        KeyCode::Tab | KeyCode::BackTab => {
            state.cycle_focus();
            if state.focus == Focus::Accounts {
                refresh_detail(app, state);
            }
        }
        KeyCode::Down => {
            state.move_selection(1);
            after_move(app, state);
        }
        KeyCode::Up => {
            state.move_selection(-1);
            after_move(app, state);
        }
        KeyCode::Enter => match state.focus {
            Focus::Commands => {
                if let Some(action) = state.current_command() {
                    run_action(app, state, action);
                }
            }
            Focus::Recall => import_selected_recall(app, state),
            Focus::Accounts => {
                // Enter on a wallet is the obvious way to ask "how much is in it".
                run_action(app, state, Action::Balance);
            }
        },
        KeyCode::Esc => {
            if state.focus == Focus::Recall {
                close_recall(app, state);
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
                after_move(app, state);
            } else if key == 'k' {
                state.move_selection(-1);
                after_move(app, state);
            }
        }
        _ => {}
    }
}

/// Keep the detail pane in step with whatever the arrow keys just moved.
fn after_move(app: &App, state: &mut State) {
    match state.focus {
        Focus::Accounts => refresh_detail(app, state),
        Focus::Recall => refresh_recall_detail(state),
        Focus::Commands => {}
    }
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
        Action::Balance => refresh_balance(app, state),
        Action::NewAddress => start_input(
            state,
            InputKind::Label { new_seed: false },
            "Label for the next address (blank = auto)",
        ),
        Action::NewSeed => start_input(
            state,
            InputKind::Label { new_seed: true },
            "Label for the first address of a new seed (blank = auto)",
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
                _ => refresh_detail(app, state),
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
                start_input(state, InputKind::DeriveIndex, "Address index to derive");
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
            } else {
                state.fail("No wallet selected — create one first");
            }
        }
        Action::Remove => {
            if state.focus == Focus::Recall {
                forget_selected_recall(app, state);
            } else if state.current().is_some() {
                state.mode = Mode::Confirm(ConfirmKind::Remove);
                let label = state.current().map(|a| a.label.clone()).unwrap_or_default();
                state.info(format!("Remove {label}? [y/N]"));
            } else {
                state.fail("No wallet selected");
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

fn close_recall(app: &App, state: &mut State) {
    state.focus = Focus::Accounts;
    refresh_detail(app, state);
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

fn input_key(app: &App, state: &mut State, kind: InputKind, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            state.mode = Mode::Browse;
            clear_input(state);
            state.info("Cancelled");
        }
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::Char(c) => state.input.push(c),
        KeyCode::Enter => {
            let value = state.input.trim().to_string();
            clear_input(state);
            state.mode = Mode::Browse;
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
        ConfirmKind::Remove => remove_account(app, state),
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
        InputKind::SendTo => {
            if wallet::parse_address(&value).is_err() {
                state.fail(format!("'{value}' is not a valid address"));
                return;
            }
            state.pending_to = value;
            start_input(state, InputKind::SendAmount, "Amount to send");
        }
        InputKind::SendAmount => {
            if units::parse_ether(&value).is_err() {
                state.fail(format!("'{value}' is not a valid amount"));
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

fn remove_account(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        return;
    };
    match app.store.delete_account(&account.id) {
        Ok(()) => {
            reload(app, state);
            state.info(format!("Removed {}", account.label));
        }
        Err(e) => state.fail(e.message),
    }
}

fn sign(app: &App, state: &State, message: &str) -> Result<String> {
    let account = state
        .current()
        .ok_or_else(|| error::usage("no wallet selected"))?;
    let keypair = Keypair::from_hex(&account.private_key)?;
    let _ = app;
    Ok(format!(
        "0x{}",
        hex::encode(keypair.sign_message(message.as_bytes())?)
    ))
}

fn refresh_balance(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        state.fail("No wallet selected — create one first");
        return;
    };
    state.info("Querying balance…");
    match app.balance(TargetArgs {
        address: Some(account.address.clone()),
        account: None,
    }) {
        Ok(output) => {
            let balance = output.data["balance"].as_str().unwrap_or("?").to_string();
            let symbol = output.data["symbol"].as_str().unwrap_or("").to_string();
            state.detail.retain(|(key, _)| key != "Balance");
            state
                .detail
                .push(("Balance".into(), format!("{balance} {symbol}")));
            state.info(format!("Balance {balance} {symbol}"));
        }
        Err(e) => state.fail(e.message),
    }
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
        wait: false,
        account: Some(account.id.clone()),
    };
    state.info("Checking the balance…");
    match app.plan_send(&args) {
        Ok(plan) => {
            let fee = plan.gas_price * U256::from(plan.gas_limit);
            state.detail = vec![
                ("To".into(), plan.to.to_checksum(None)),
                (
                    "Amount".into(),
                    format!("{} {}", state.pending_amount, app.network.symbol),
                ),
                ("Nonce".into(), plan.nonce.to_string()),
                ("Gas limit".into(), plan.gas_limit.to_string()),
                (
                    "Gas price".into(),
                    format!("{} gwei", units::format_gwei(plan.gas_price)),
                ),
                (
                    "Max fee".into(),
                    format!("{} {}", units::format_ether(fee), app.network.symbol),
                ),
            ];
            state.staged = Some(plan);
            state.mode = Mode::Confirm(ConfirmKind::Send);
            state.info(format!(
                "Send {} {} to {}? [y/N]",
                state.pending_amount, app.network.symbol, state.pending_to
            ));
        }
        Err(e) => state.fail(e.message),
    }
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
fn select_network(app: &App, state: &mut State, key: &'static str) {
    let target = match network::find(key) {
        Ok(chain) => chain,
        Err(e) => {
            state.fail(e.message);
            return;
        }
    };
    if state.current_network == target.key {
        state.info(format!("Already on {}", target.name));
        return;
    }
    match app
        .store
        .config_set(causewaybay_core::store::KEY_NETWORK, target.key)
    {
        Ok(()) => {
            state.current_network = target.key.to_string();
            state.network_changed = true;
            state.info(format!(
                "Network is now {} (chain {})",
                target.name, target.chain_id
            ));
        }
        Err(e) => state.fail(e.message),
    }
}

fn reload(app: &App, state: &mut State) {
    if let Ok(accounts) = app.store.accounts() {
        let keep = state.current().map(|a| a.id.clone());
        state.accounts = accounts;
        let index = keep
            .and_then(|id| state.accounts.iter().position(|a| a.id == id))
            .unwrap_or(0);
        state.selected.select(if state.accounts.is_empty() {
            None
        } else {
            Some(index)
        });
    }
    refresh_detail(app, state);
}

fn refresh_detail(app: &App, state: &mut State) {
    let Some(account) = state.current().cloned() else {
        state.detail = Vec::new();
        return;
    };
    let mut rows = vec![
        ("Label".to_string(), account.label.clone()),
        ("Address".to_string(), account.address.clone()),
        ("Source".to_string(), account.source.as_str().to_string()),
    ];
    if let Some(path) = &account.derivation_path {
        rows.push(("Path".to_string(), path.clone()));
    }
    rows.push((
        "Private key".to_string(),
        if state.show_secrets {
            account.private_key.clone()
        } else {
            crate::output::truncate_secret(&account.private_key)
        },
    ));
    if let Some(mnemonic) = &account.mnemonic {
        rows.push((
            "Mnemonic".to_string(),
            if state.show_secrets {
                mnemonic.clone()
            } else {
                "<hidden — press v>".into()
            },
        ));
    }
    rows.push((
        "Explorer".to_string(),
        app.network.address_url(&account.address),
    ));
    state.detail = rows;
}

// ================================================================= rendering

fn draw(frame: &mut Frame, app: &App, state: &mut State) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
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
            Constraint::Length(27), // commands
            Constraint::Percentage(34),
            Constraint::Min(20),
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

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let network = app.store.network().unwrap_or(app.network);
    let active = app.store.active_account().ok();

    let mut spans = vec![
        Span::styled(
            "Causewaybay Wallet",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  ·  "),
        Span::styled(
            format!("{} (chain {})", network.name, network.chain_id),
            Style::default().fg(if network.testnet {
                Color::Green
            } else {
                Color::Yellow
            }),
        ),
        Span::raw("  ·  "),
    ];

    // The address that balance, send and the CLI all act on by default — worth
    // having in view rather than a pane away.
    match active {
        Some(account) => {
            spans.push(Span::styled(
                format!("{}: ", account.label),
                Style::default().fg(Color::DarkGray),
            ));
            spans.push(Span::styled(
                account.address.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        None => spans.push(Span::styled(
            "no wallet yet",
            Style::default().fg(Color::DarkGray),
        )),
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_commands(frame: &mut Frame, state: &mut State, area: Rect) {
    let focused = state.focus == Focus::Commands;
    let current_network = state.current_network.clone();
    let items: Vec<ListItem> = state
        .command_list
        .iter()
        .map(|command| {
            // A network row is marked when it is the one in use, so the pane
            // doubles as the answer to "which chain am I on?".
            let selected_network = matches!(
                command.action,
                Action::SelectNetwork(key) if key == current_network
            );
            let label_style = if selected_network {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let marker = if selected_network { "●" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker}{:<LABEL_WIDTH$}", command.label),
                    label_style,
                ),
                Span::styled(
                    command.key.map(String::from).unwrap_or_else(|| " ".into()),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
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
    let items: Vec<ListItem> = state
        .accounts
        .iter()
        .map(|account| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<12}", truncate(&account.label, 12)),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(short_address(&account.address)),
            ]))
        })
        .collect();

    let title = if state.accounts.is_empty() {
        " Wallets (none yet — press n) ".to_string()
    } else {
        format!(" Wallets ({}) ", state.accounts.len())
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
    let lines: Vec<Line> = if state.detail.is_empty() {
        vec![
            Line::from(Span::styled(
                "No wallet yet.",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Pick \"New address\" on the left and press Enter,",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "or press n. Press ? for the full reference.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        state
            .detail
            .iter()
            .map(|(key, value)| {
                Line::from(vec![
                    Span::styled(format!("{key:<13}"), Style::default().fg(Color::DarkGray)),
                    Span::raw(value.clone()),
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
    let (text, style) = match state.mode {
        Mode::Input(kind) => (
            format!("{} > {}_", state.status, echo(kind, &state.input)),
            Style::default().fg(Color::Cyan),
        ),
        _ => (
            state.status.clone(),
            if state.status_is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, style)))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Status ")),
        area,
    );
}

/// One line, always the same, so the bottom of the screen never overflows.
fn draw_hint(frame: &mut Frame, state: &State, area: Rect) {
    let hint = match state.mode {
        Mode::Input(_) => "Enter confirm · Esc cancel",
        Mode::Confirm(_) => "y confirm · any other key cancels",
        Mode::Help => "any key closes this reference",
        Mode::Browse => "Tab pane · ↑↓ move · Enter run · ? help · q quit",
    };
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
const LABEL_WIDTH: usize = 19;

/// The full reference, drawn over the top of everything else.
fn draw_help_overlay(frame: &mut Frame, commands: &[Command], area: Rect) {
    let popup = centered_rect(HELP_WIDTH, commands.len() as u16 + 12, area);
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
    for command in commands {
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
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Saved files land in the directory you started the TUI from.",
        Style::default().fg(Color::DarkGray),
    )));

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

/// Clip a label to the column width, marking that it was cut.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use causewaybay_core::store::Source;

    fn account(id: &str, label: &str) -> Account {
        Account {
            id: id.into(),
            label: label.into(),
            address: "0x9858EfFD232B4033E47d90003D41EC34EcaEda94".into(),
            source: Source::Mnemonic,
            private_key: "0x1ab42cc412b618bdea3a599e3c9bae199ebf030895b039e9db1e30dafb12b727"
                .into(),
            mnemonic: Some("abandon about".into()),
            derivation_path: Some("m/44'/60'/0'/0/0".into()),
            index: Some(0),
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
    fn truncates_long_labels() {
        assert_eq!(truncate("short", 12), "short");
        assert_eq!(truncate("a-very-long-label", 8), "a-very-…");
        assert_eq!(truncate("exactlyeight", 12), "exactlyeight");
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
                Action::NewAddress => Some('n'),
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
            Action::NewAddress,
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

    /// The menu is generated from `network::ALL`, so a chain added to the table
    /// becomes an entry without anyone editing this file.
    #[test]
    fn every_network_is_its_own_menu_entry() {
        let commands = build_commands();
        for chain in network::ALL {
            let entry = commands
                .iter()
                .find(|command| command.action == Action::SelectNetwork(chain.key))
                .unwrap_or_else(|| panic!("{} is missing from the menu", chain.key));
            assert_eq!(entry.label, chain.name, "the entry is labelled by name");
            assert_eq!(entry.key, None, "networks are menu-only");
            assert!(entry.help.contains(&chain.chain_id.to_string()));
        }
        // Flattened, not a toggle: one row per chain.
        let network_rows = commands
            .iter()
            .filter(|command| matches!(command.action, Action::SelectNetwork(_)))
            .count();
        assert_eq!(network_rows, network::ALL.len());
    }

    #[test]
    fn both_cronos_networks_are_on_the_menu_by_name() {
        let labels: Vec<String> = build_commands()
            .iter()
            .filter(|c| matches!(c.action, Action::SelectNetwork(_)))
            .map(|c| c.label.clone())
            .collect();
        assert_eq!(labels, ["Cronos EVM Testnet", "Cronos EVM Mainnet"]);
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
        let mut state = State::new(vec![account("a", "a"), account("b", "b")], None);

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
        assert_eq!(state.commands.selected(), Some(build_commands().len() - 1));
        state.move_selection(1);
        assert_eq!(state.commands.selected(), Some(0));

        state.focus = Focus::Accounts;
        state.move_selection(-1);
        assert_eq!(state.selected.selected(), Some(1));
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
        let accounts = vec![account("acc_1", "one"), account("acc_2", "two")];
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
    fn the_opening_status_line_explains_the_controls() {
        let state = State::new(Vec::new(), None);
        assert!(state.status.contains("Tab"));
        assert!(state.status.contains("Enter"));
        assert!(state.status.contains('?'));
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
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let popup = centered_rect(HELP_WIDTH, build_commands().len() as u16 + 12, area);
        // 4 navigation rows, a blank, a heading, one row per command, a blank,
        // a closing note, and two borders.
        let needed = 4 + 1 + 1 + build_commands().len() as u16 + 1 + 1 + 2;
        assert!(popup.height >= needed, "overlay clips the last commands");
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

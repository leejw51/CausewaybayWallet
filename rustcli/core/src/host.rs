//! The two things a command needs from the world outside the wallet.
//!
//! A command may have to read a secret that was passed as `-`, and it may have
//! to ask before spending money. Both are trivial for a terminal and impossible
//! for a shared library called from Lua, so neither is hardcoded: [`App`] holds
//! a `dyn Host` and asks it.
//!
//! [`Headless`] is the implementation core ships — it answers from values the
//! caller supplied up front, which is exactly what a GUI or an FFI caller has.
//! The `cwbwallet` binary substitutes one that talks to the terminal.
//!
//! [`App`]: crate::app::App

use crate::error::{self, Result};

pub trait Host: Send + Sync {
    /// Supply the text an argument given as `-` stands for.
    ///
    /// `what` names the thing being asked for ("mnemonic", "message") and
    /// belongs in the error when nothing is available.
    fn read_input(&self, what: &str) -> Result<String>;

    /// Ask before something irreversible. `Ok(())` is a yes.
    ///
    /// A no must be reported as [`Code::ConfirmationRequired`], which is the
    /// code every caller — including the CLI's exit status — branches on.
    ///
    /// [`Code::ConfirmationRequired`]: crate::error::Code::ConfirmationRequired
    fn confirm(&self, prompt: &str) -> Result<()>;

    /// Report that something slow is still going.
    ///
    /// Most commands never call this. The one that must is a Midnight send
    /// from an address already registered for DUST generation: it replays the
    /// chain's dust event stream and then generates a zero-knowledge proof,
    /// which is minutes of work with nothing on the screen. Silence there is
    /// indistinguishable from a hang, and a user who kills a wallet mid-send
    /// has every reason to think the funds are gone.
    ///
    /// It is a notification, not a question: there is no answer and no way to
    /// fail. A front end with nowhere to put the text ignores it, which is why
    /// the default does nothing rather than being required of every host.
    fn progress(&self, _message: &str) {}
}

/// A host with no terminal behind it: every answer is decided in advance.
///
/// This is what the C ABI builds from a request, and what a GUI wants — the
/// GUI has already shown its own dialog by the time it calls in, so it passes
/// `assume_yes` rather than having the wallet try to prompt from a library.
#[derive(Debug, Default, Clone)]
pub struct Headless {
    /// Answers every [`Host::confirm`] with yes.
    pub assume_yes: bool,
    /// What an argument of `-` reads as. `None` means "nothing was supplied".
    pub input: Option<String>,
}

impl Headless {
    /// A host that refuses every confirmation and has no input to give.
    pub fn new() -> Self {
        Headless::default()
    }

    pub fn assume_yes(mut self, yes: bool) -> Self {
        self.assume_yes = yes;
        self
    }

    pub fn with_input(mut self, input: Option<String>) -> Self {
        self.input = input;
        self
    }
}

impl Host for Headless {
    fn read_input(&self, what: &str) -> Result<String> {
        match &self.input {
            Some(text) => Ok(text.clone()),
            None => Err(error::usage(format!(
                "no {what} supplied; pass it in the request rather than as `-`"
            ))),
        }
    }

    fn confirm(&self, prompt: &str) -> Result<()> {
        if self.assume_yes {
            Ok(())
        } else {
            Err(error::confirmation_required(format!(
                "{prompt} — re-run with --yes to confirm"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;

    #[test]
    fn a_headless_host_refuses_by_default() {
        let host = Headless::new();
        let refused = host.confirm("send 1 CRO?").unwrap_err();
        assert_eq!(refused.code, Code::ConfirmationRequired);
        // The message has to say how to get past it, or the caller is stuck.
        assert!(refused.message.contains("--yes"));
    }

    #[test]
    fn assume_yes_answers_every_prompt() {
        assert!(Headless::new().assume_yes(true).confirm("really?").is_ok());
    }

    /// A host that cares about progress gets it; one that does not is unaffected.
    #[test]
    fn progress_defaults_to_doing_nothing_and_can_be_overridden() {
        use std::sync::Mutex;

        // The default: a host that never mentions progress still compiles and
        // silently swallows it.
        Headless::new().progress("syncing dust state");

        struct Recording(Mutex<Vec<String>>);
        impl Host for Recording {
            fn read_input(&self, _: &str) -> Result<String> {
                Ok(String::new())
            }
            fn confirm(&self, _: &str) -> Result<()> {
                Ok(())
            }
            fn progress(&self, message: &str) {
                self.0.lock().unwrap().push(message.to_string());
            }
        }
        let host = Recording(Mutex::new(Vec::new()));
        host.progress("replaying 150000 dust events");
        host.progress("proving");
        assert_eq!(host.0.lock().unwrap().len(), 2);
    }

    #[test]
    fn input_is_whatever_the_caller_supplied() {
        let host = Headless::new().with_input(Some("hello".into()));
        assert_eq!(host.read_input("message").unwrap(), "hello");

        // Without it, the error names what was missing.
        let missing = Headless::new().read_input("mnemonic").unwrap_err();
        assert_eq!(missing.code, Code::Usage);
        assert!(missing.message.contains("mnemonic"));
    }
}

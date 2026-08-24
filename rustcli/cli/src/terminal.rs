//! The `Host` a terminal can provide: a real stdin, and a real question.

use std::io::{IsTerminal, Read, Write};

use causewaybay_core::error::{self, Result};
use causewaybay_core::host::Host;

pub struct TerminalHost {
    /// `--yes`: every confirmation is already answered.
    assume_yes: bool,
    /// Whether prompting is allowed at all. `--json` clears it, because an
    /// envelope on stdout has no room for a question and no one to read it.
    may_prompt: bool,
}

impl TerminalHost {
    pub fn new(assume_yes: bool, may_prompt: bool) -> Self {
        TerminalHost {
            assume_yes,
            may_prompt,
        }
    }
}

impl Host for TerminalHost {
    /// Slow work narrates itself on stderr, the way test-wallet3 does it:
    /// stdout stays the single machine-readable channel, and a Midnight dust
    /// sync that takes minutes stops looking like a hang.
    fn progress(&self, message: &str) {
        eprintln!("  {message}");
    }

    fn read_input(&self, what: &str) -> Result<String> {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        if buffer.trim().is_empty() {
            return Err(error::usage(format!("no {what} supplied on stdin")));
        }
        Ok(buffer)
    }

    fn confirm(&self, prompt: &str) -> Result<()> {
        if self.assume_yes {
            return Ok(());
        }
        // Piped input is not a person: asking would either hang or consume
        // data meant for the command itself.
        if !self.may_prompt || !std::io::stdin().is_terminal() {
            return Err(error::confirmation_required(format!(
                "{prompt} — re-run with --yes to confirm"
            )));
        }
        // The question goes to stderr so `cwbwallet … > file` still works.
        eprint!("{prompt} [y/N]: ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            Ok(())
        } else {
            Err(error::confirmation_required("cancelled"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use causewaybay_core::error::Code;

    #[test]
    fn yes_short_circuits_every_prompt() {
        assert!(TerminalHost::new(true, true).confirm("really?").is_ok());
        // Even when prompting is off — `--yes --json` is a normal combination.
        assert!(TerminalHost::new(true, false).confirm("really?").is_ok());
    }

    #[test]
    fn json_mode_refuses_instead_of_asking() {
        let refused = TerminalHost::new(false, false)
            .confirm("send?")
            .unwrap_err();
        assert_eq!(refused.code, Code::ConfirmationRequired);
        assert!(refused.message.contains("--yes"));
    }
}

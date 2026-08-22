//! Copying text to the system clipboard.
//!
//! Done by piping to whatever the platform provides rather than by linking a
//! clipboard crate: on Linux those pull in X11/Wayland development libraries,
//! which is a heavy thing to require of a wallet that is otherwise pure Rust.
//! The trade-off is that a headless machine with no helper installed gets a
//! clear message instead of silent failure.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::error::{self, Result};

/// The candidates, in the order they are tried: (program, arguments).
fn candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        vec![("pbcopy", vec![])]
    } else if cfg!(target_os = "windows") {
        vec![("clip", vec![])]
    } else {
        vec![
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ]
    }
}

/// Copy `text` to the clipboard, returning the helper that accepted it.
pub fn copy(text: &str) -> Result<&'static str> {
    let mut tried = Vec::new();
    for (program, args) in candidates() {
        match write_to(program, &args, text) {
            Ok(()) => return Ok(program),
            Err(reason) => tried.push(format!("{program} ({reason})")),
        }
    }
    Err(error::internal(format!(
        "no clipboard helper worked — tried {}",
        tried.join(", ")
    )))
}

/// Spawn one helper and feed it the text on stdin.
fn write_to(program: &str, args: &[&str], text: &str) -> std::result::Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    // The pipe is dropped before waiting, so the helper sees EOF and can exit.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "no stdin pipe".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("exited with {status}"))
    }
}

/// True when some clipboard helper is on PATH.
///
/// Used to decide whether to offer the command at all, so a machine without one
/// does not advertise something that cannot work.
pub fn is_available() -> bool {
    candidates().into_iter().any(|(program, _)| {
        Command::new(program)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|mut child| {
                let _ = child.kill();
                let _ = child.wait();
                true
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clipboard is one shared resource for the whole machine, so the tests
    /// that write to it take turns rather than racing each other.
    static CLIPBOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn the_candidate_list_suits_the_platform() {
        let programs: Vec<&str> = candidates().into_iter().map(|(p, _)| p).collect();
        assert!(!programs.is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(programs, vec!["pbcopy"]);
        } else if cfg!(target_os = "linux") {
            // Wayland first, then the two X11 helpers.
            assert!(programs.contains(&"wl-copy"));
            assert!(programs.contains(&"xclip"));
        }
    }

    #[test]
    fn a_missing_helper_reports_what_it_tried() {
        let err = write_to("definitely-not-a-real-program", &[], "x").unwrap_err();
        assert!(!err.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn copies_and_reads_back_on_macos() {
        let _guard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());

        // Whatever the developer had on their clipboard is put back afterwards;
        // a test has no business keeping it.
        let previous = Command::new("pbpaste").output().ok().map(|o| o.stdout);

        let marker = "causewaybay-clipboard-test-0xabc123";
        let helper = copy(marker).expect("pbcopy should work on macOS");
        assert_eq!(helper, "pbcopy");

        let pasted = Command::new("pbpaste")
            .output()
            .expect("pbpaste should run");
        let round_tripped = String::from_utf8_lossy(&pasted.stdout).trim().to_string();

        if let Some(bytes) = previous {
            let _ = write_to("pbcopy", &[], &String::from_utf8_lossy(&bytes));
        }
        assert_eq!(round_tripped, marker);
    }

    #[test]
    fn copying_an_empty_string_is_not_an_error() {
        let _guard = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
        if is_available() {
            let previous = if cfg!(target_os = "macos") {
                Command::new("pbpaste").output().ok().map(|o| o.stdout)
            } else {
                None
            };
            assert!(copy("").is_ok());
            if let Some(bytes) = previous {
                let _ = write_to("pbcopy", &[], &String::from_utf8_lossy(&bytes));
            }
        }
    }
}

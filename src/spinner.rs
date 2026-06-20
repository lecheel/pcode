use std::io::{IsTerminal, Write};

/// A terminal spinner that animates in the background via a tokio task.
/// Automatically stops on drop.
pub struct Spinner {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Spinner {
    /// Start a spinner with the given label, e.g. "Thinking".
    /// If stderr is not a terminal (piped), this is a no-op.
    pub fn start(msg: &str) -> Self {
        if !std::io::stderr().is_terminal() {
            return Self { handle: None };
        }

        let msg = msg.to_string();
        let handle = tokio::spawn(async move {
            let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let mut i = 0;
            loop {
                eprint!("\r  {} {} ", frames[i], msg);
                let _ = std::io::stderr().flush();
                i = (i + 1) % frames.len();
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            }
        });
        Self {
            handle: Some(handle),
        }
    }

    /// Stop the spinner and clear the line.
    pub fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
            eprint!("\r{}\r", " ".repeat(60));
            let _ = std::io::stderr().flush();
        }
    }

    /// Change the spinner label (stops old, starts new).
    pub fn set_message(&mut self, msg: &str) {
        self.stop();
        *self = Spinner::start(msg);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

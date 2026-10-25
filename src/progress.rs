// Install progress: one line per stage on stderr. On a terminal the current
// stage shows an animated spinner and turns into a check mark when the next
// stage starts; when stderr is redirected (scripts, CI, tests) every stage is
// printed as a plain line, exactly as before.
use std::{
    fmt,
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CLEAR_LINE: &str = "\r\x1b[2K";

struct Spinner {
    message: String,
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

// Serialises every write so a warning never lands in the middle of a frame.
static OUTPUT: Mutex<()> = Mutex::new(());
static ACTIVE: Mutex<Option<Spinner>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn animated() -> bool {
    io::stderr().is_terminal() && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

fn write(text: &str) {
    let _guard = lock(&OUTPUT);
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.flush();
}

fn mark(symbol: &str, color: &str) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
        symbol.to_owned()
    } else {
        format!("\x1b[{color}m{symbol}\x1b[0m")
    }
}

// Begin a stage, finishing the previous one.
pub fn stage(message: &str) {
    if !animated() {
        eprintln!("{message}");
        return;
    }
    finish();
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let text = message.to_owned();
    let handle = thread::spawn(move || {
        let mut frame = 0;
        while !flag.load(Ordering::Relaxed) {
            write(&format!(
                "{CLEAR_LINE}{} {text}",
                mark(FRAMES[frame % FRAMES.len()], "36")
            ));
            frame += 1;
            thread::sleep(Duration::from_millis(80));
        }
    });
    *lock(&ACTIVE) = Some(Spinner {
        message: message.to_owned(),
        stop,
        handle,
    });
}

fn end(symbol: &str, color: &str) {
    let Some(spinner) = lock(&ACTIVE).take() else {
        return;
    };
    spinner.stop.store(true, Ordering::Relaxed);
    let _ = spinner.handle.join();
    write(&format!(
        "{CLEAR_LINE}{} {}\n",
        mark(symbol, color),
        spinner.message.trim_end_matches("...")
    ));
}

// Mark the running stage as done.
pub fn finish() {
    end("✔", "32");
}

// Mark the running stage as failed; call before printing the error.
pub fn fail() {
    end("✘", "31");
}

// Print a warning without corrupting the spinner line.
pub fn warning(message: fmt::Arguments) {
    if animated() && lock(&ACTIVE).is_some() {
        write(&format!("{CLEAR_LINE}{message}\n"));
    } else {
        eprintln!("{message}");
    }
}

macro_rules! notice {
    ($($arg:tt)*) => {
        $crate::progress::warning(format_args!($($arg)*))
    };
}
pub(crate) use notice;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirected_stderr_prints_plain_stages_without_a_spinner() {
        // Test harness stderr is captured, never a terminal.
        assert!(!animated());
        stage("Extracting package files...");
        assert!(lock(&ACTIVE).is_none());
        finish();
        fail();
    }
}

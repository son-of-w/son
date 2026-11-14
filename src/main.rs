use clap::Parser;
use son_of_w::{cli::Cli, error};

/// The main thread of a Windows program has a stack of 1 MB, where macOS
/// and Linux give 8 MB. Parsing the command line of a build that is not
/// optimized needs more than 1 MB, so the work is done on a thread whose
/// stack has the same size on every system.
const STACK_BYTES: usize = 8 * 1024 * 1024;

/// True when a panic is `println!` or `eprintln!` that met a closed pipe:
/// the reader of the output went away, as in `son recall x | head -1`.
/// The message is the one of the standard library; the second and third
/// forms are what Windows says for a pipe that is closed.
fn reader_went_away(message: &str) -> bool {
    (message.starts_with("failed printing to stdout")
        || message.starts_with("failed printing to stderr"))
        && (message.contains("Broken pipe")
            || message.contains("os error 232")
            || message.contains("os error 109"))
}

fn main() {
    // A reader that closes the pipe early is not a fault of son: end
    // quietly, as other command-line programs do, and print no panic.
    // What the command did to the store is done or taken back whole, as
    // SQLite keeps it; only the rest of the output is lost.
    let usual = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("");
        if reader_went_away(message) {
            std::process::exit(0);
        }
        usual(info);
    }));
    let work = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(run)
        .expect("cannot start the working thread");
    if work.join().is_err() {
        // The panic message is already printed.
        std::process::exit(101);
    }
}

fn run() {
    // Known before the arguments are parsed, so that a mistake in the
    // arguments is reported in the form the caller asked for.
    let json = std::env::args()
        .skip(1)
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // --help and --version are not errors; clap prints them itself.
        Err(err) if json && err.use_stderr() => {
            let message = err.to_string();
            let message = message
                .trim()
                .strip_prefix("error: ")
                .unwrap_or(message.trim());
            eprintln!("{}", error::json_of("invalid_argument", message));
            std::process::exit(2);
        }
        Err(err) => err.exit(),
    };
    if let Err(err) = son_of_w::app::run(cli) {
        if json {
            eprintln!("{}", error::to_json(&err));
        } else {
            eprintln!("error: {err:#}");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::reader_went_away;

    #[test]
    fn only_a_closed_pipe_ends_quietly() {
        assert!(reader_went_away(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(reader_went_away(
            "failed printing to stderr: Broken pipe (os error 32)"
        ));
        assert!(reader_went_away(
            "failed printing to stdout: The pipe is being closed. (os error 232)"
        ));
        // A full disk is a fault that the user must see.
        assert!(!reader_went_away(
            "failed printing to stdout: No space left on device (os error 28)"
        ));
        assert!(!reader_went_away("index out of bounds"));
    }
}

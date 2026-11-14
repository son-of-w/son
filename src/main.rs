use clap::Parser;
use son_of_w::{cli::Cli, error};

/// The main thread of a Windows program has a stack of 1 MB, where macOS
/// and Linux give 8 MB. Parsing the command line of a build that is not
/// optimized needs more than 1 MB, so the work is done on a thread whose
/// stack has the same size on every system.
const STACK_BYTES: usize = 8 * 1024 * 1024;

fn main() {
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

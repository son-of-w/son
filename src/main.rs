use clap::Parser;
use son_of_w::{cli::Cli, error};

fn main() {
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

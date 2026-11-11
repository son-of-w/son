use clap::Parser;

fn main() {
    let cli = son_of_w::cli::Cli::parse();
    if let Err(err) = son_of_w::app::run(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

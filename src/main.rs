//! The `relay` command line interface.

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let code = relay_rs::cli::run(arguments);
    std::process::exit(code);
}

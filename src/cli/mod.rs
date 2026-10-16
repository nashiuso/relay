//! The `relay` command line interface.
//!
//! The interface is intentionally small: a handful of commands, one flag per
//! idea, and help text at every level. Everything the CLI does is available
//! through the library as well.

pub mod args;
pub mod commands;
pub mod output;

use args::FlagSpec;
use commands::CliError;
use output::Style;

/// The exit code for a successful run.
pub const EXIT_OK: i32 = 0;
/// The exit code for a runtime error.
pub const EXIT_ERROR: i32 = 1;
/// The exit code for a usage error.
pub const EXIT_USAGE: i32 = 2;

/// The global options, accepted by every command.
pub const GLOBAL_FLAGS: [FlagSpec; 7] = [
    FlagSpec::value("config", "PATH", "Use a specific configuration file"),
    FlagSpec::boolean_short("verbose", 'v', "Print more diagnostics"),
    FlagSpec::boolean_short("quiet", 'q', "Print only results and errors"),
    FlagSpec::boolean("no-color", "Disable coloured output"),
    FlagSpec::boolean("json", "Print machine readable JSON"),
    FlagSpec::boolean("help", "Show help for a command"),
    FlagSpec::boolean("version", "Show the version"),
];

/// Runs the command line interface and returns the exit code.
pub fn run(arguments: Vec<String>) -> i32 {
    let mut arguments = arguments;
    arguments.remove(0);

    let (command, rest) = match split_command(&arguments) {
        Some(parts) => parts,
        None => {
            if arguments
                .iter()
                .any(|arg| arg == "--version" || arg == "-V")
            {
                println!("relay {}", crate::VERSION);
                return EXIT_OK;
            }
            print_help();
            return EXIT_OK;
        }
    };

    let (specs, help) = match command.as_str() {
        "init" => (
            GLOBAL_FLAGS
                .iter()
                .copied()
                .chain([
                    FlagSpec::value("path", "PATH", "Write the file to this path"),
                    FlagSpec::boolean("force", "Overwrite an existing file"),
                ])
                .collect::<Vec<_>>(),
            INIT_HELP,
        ),
        "providers" => (GLOBAL_FLAGS.to_vec(), PROVIDERS_HELP),
        "models" => (
            GLOBAL_FLAGS
                .iter()
                .copied()
                .chain([FlagSpec::boolean("refresh", "Ignore the cached model list")])
                .collect::<Vec<_>>(),
            MODELS_HELP,
        ),
        "chat" => (
            GLOBAL_FLAGS
                .iter()
                .copied()
                .chain([
                    FlagSpec::value_short("model", 'm', "MODEL", "Model to use, `provider/model`"),
                    FlagSpec::value_short("prompt", 'p', "TEXT", "The question to ask"),
                    FlagSpec::value("system", "TEXT", "A system message"),
                    FlagSpec::value("temperature", "VALUE", "Sampling temperature"),
                    FlagSpec::value("max-tokens", "N", "Maximum number of tokens"),
                    FlagSpec::boolean("interactive", "Keep a conversation open"),
                    FlagSpec::boolean("no-stream", "Wait for the complete answer"),
                ])
                .collect::<Vec<_>>(),
            CHAT_HELP,
        ),
        "doctor" => (GLOBAL_FLAGS.to_vec(), DOCTOR_HELP),
        "serve" => (
            GLOBAL_FLAGS
                .iter()
                .copied()
                .chain([
                    FlagSpec::value("host", "HOST", "Interface to bind to"),
                    FlagSpec::value("port", "PORT", "Port to listen on"),
                    FlagSpec::boolean("allow-remote", "Allow non local clients"),
                ])
                .collect::<Vec<_>>(),
            SERVE_HELP,
        ),
        "help" => {
            print_help();
            return EXIT_OK;
        }
        other => {
            eprintln!("error: unknown command `{other}`");
            eprintln!();
            eprintln!("Run `relay --help` to see the available commands.");
            return EXIT_USAGE;
        }
    };

    let parsed = match args::parse(&specs, &rest) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!();
            eprintln!("{}", help);
            return EXIT_USAGE;
        }
    };

    if parsed.is_set("help") {
        println!("{help}");
        return EXIT_OK;
    }
    if parsed.is_set("version") {
        println!("relay {}", crate::VERSION);
        return EXIT_OK;
    }

    let style = Style::detect(
        parsed.is_set("no-color"),
        parsed.is_set("quiet"),
        parsed.is_set("json"),
    );

    let result = match command.as_str() {
        "init" => commands::init(&parsed, &style),
        "providers" => commands::providers(&parsed, &style),
        "models" => commands::models(&parsed, &style),
        "chat" => commands::chat(&parsed, &style),
        "doctor" => commands::doctor(&parsed, &style),
        "serve" => commands::serve(&parsed, &style),
        _ => unreachable!("the command was validated above"),
    };

    match result {
        Ok(()) => EXIT_OK,
        Err(error) => {
            report(&error, &style);
            if error.usage { EXIT_USAGE } else { EXIT_ERROR }
        }
    }
}

fn report(error: &CliError, style: &Style) {
    output::error(style, &error.message);
    if let Some(hint) = &error.hint {
        output::note(style, &format!("hint: {hint}"));
    }
}

/// Splits the command from the remaining arguments.
///
/// Global options may appear before or after the command, so everything but
/// the command itself is handed to the command parser.
fn split_command(arguments: &[String]) -> Option<(String, Vec<String>)> {
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        if argument == "--" {
            return None;
        }
        if argument.starts_with('-') {
            // `--config` is the only global option that takes a value.
            if argument == "--config" {
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        let mut rest: Vec<String> = arguments[..index].to_vec();
        rest.extend(arguments[index + 1..].iter().cloned());
        return Some((argument.to_owned(), rest));
    }
    None
}

/// Prints the top level help.
pub fn print_help() {
    println!("{HELP}");
}

const HELP: &str = r#"relay — one interface. your models.

Usage:
  relay <command> [options]

Commands:
  init        Write a configuration file
  providers   List the configured providers
  models      List the models of a provider
  chat        Ask a model a question
  doctor      Check the local setup
  serve       Run the optional OpenAI compatible server

Options:
  -c, --config <PATH>  Use a specific configuration file
  -v, --verbose        Print more diagnostics
  -q, --quiet          Print only results and errors
      --no-color       Disable coloured output
      --json           Print machine readable JSON
  -h, --help           Show this help
  -V, --version        Show the version

Run `relay <command> --help` for the options of a command.
"#;

const INIT_HELP: &str = r#"relay init — write a configuration file

Usage:
  relay init [options]

Options:
      --path <PATH>  Write the file to this path
      --force        Overwrite an existing file
  -q, --quiet        Print only results and errors
"#;

const PROVIDERS_HELP: &str = r#"relay providers — list the configured providers

Usage:
  relay providers [options]

Options:
      --json         Print machine readable JSON
  -q, --quiet        Print only results and errors
"#;

const MODELS_HELP: &str = r#"relay models — list the models of a provider

Usage:
  relay models [provider] [options]

Arguments:
  provider            The provider to query; all providers by default

Options:
      --refresh        Ignore the cached model list
      --json          Print machine readable JSON
  -q, --quiet         Print only results and errors
"#;

const CHAT_HELP: &str = r#"relay chat — ask a model a question

Usage:
  relay chat -m provider/model -p "your question"
  relay chat --interactive

Options:
  -m, --model <MODEL>       Model to use, `provider/model`
  -p, --prompt <TEXT>       The question to ask
      --system <TEXT>       A system message
      --temperature <VALUE>  Sampling temperature
      --max-tokens <N>      Maximum number of tokens
      --interactive         Keep a conversation open
      --no-stream           Wait for the complete answer
  -q, --quiet               Print only results and errors
"#;

const DOCTOR_HELP: &str = r#"relay doctor — check the local setup

Usage:
  relay doctor [options]

Options:
      --json         Print machine readable JSON
  -q, --quiet        Print only results and errors
"#;

const SERVE_HELP: &str = r#"relay serve — run the optional OpenAI compatible server

Usage:
  relay serve [options]

Options:
      --host <HOST>         Interface to bind to (default 127.0.0.1)
      --port <PORT>         Port to listen on (default 8080)
      --allow-remote        Allow clients from other machines
  -q, --quiet              Print only results and errors
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_command() {
        let arguments = vec![
            "chat".to_owned(),
            "-m".to_owned(),
            "openai/gpt-4o".to_owned(),
        ];
        let (command, rest) = split_command(&arguments).unwrap();
        assert_eq!(command, "chat");
        assert_eq!(rest, ["-m", "openai/gpt-4o"]);
    }

    #[test]
    fn keeps_global_flags_in_the_command_arguments() {
        let arguments = vec![
            "--config".to_owned(),
            "/tmp/relay.toml".to_owned(),
            "providers".to_owned(),
            "--json".to_owned(),
        ];
        let (command, rest) = split_command(&arguments).unwrap();
        assert_eq!(command, "providers");
        assert_eq!(rest, ["--config", "/tmp/relay.toml", "--json"]);
    }

    #[test]
    fn reports_unknown_commands() {
        assert_eq!(run(vec!["relay".to_owned(), "nope".to_owned()]), EXIT_USAGE);
    }

    #[test]
    fn prints_the_version() {
        assert_eq!(
            run(vec!["relay".to_owned(), "--version".to_owned()]),
            EXIT_OK
        );
    }
}

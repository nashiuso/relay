//! A small command line argument parser.
//!
//! The parser supports long options (`--model gpt-4o`), long options with an
//! inline value (`--model=gpt-4o`), short options (`-m gpt-4o`), boolean
//! flags and positional arguments. It intentionally stays small: Relay only
//! needs a handful of options per command.

use std::collections::{BTreeMap, BTreeSet};

/// The definition of a command line option.
#[derive(Debug, Clone, Copy)]
pub struct FlagSpec {
    /// The long name of the option, without the leading dashes.
    pub long: &'static str,
    /// The optional short name of the option.
    pub short: Option<char>,
    /// The placeholder of the value, `None` for boolean flags.
    pub value: Option<&'static str>,
    /// A short description used in the help output.
    pub help: &'static str,
}

impl FlagSpec {
    /// Defines a boolean flag.
    pub const fn boolean(long: &'static str, help: &'static str) -> Self {
        Self {
            long,
            short: None,
            value: None,
            help,
        }
    }

    /// Defines a boolean flag with a short name.
    pub const fn boolean_short(long: &'static str, short: char, help: &'static str) -> Self {
        Self {
            long,
            short: Some(short),
            value: None,
            help,
        }
    }

    /// Defines an option that takes a value.
    pub const fn value(long: &'static str, placeholder: &'static str, help: &'static str) -> Self {
        Self {
            long,
            short: None,
            value: Some(placeholder),
            help,
        }
    }

    /// Defines an option that takes a value and has a short name.
    pub const fn value_short(
        long: &'static str,
        short: char,
        placeholder: &'static str,
        help: &'static str,
    ) -> Self {
        Self {
            long,
            short: Some(short),
            value: Some(placeholder),
            help,
        }
    }
}

/// The result of parsing a command line.
#[derive(Debug, Default, Clone)]
pub struct Parsed {
    flags: BTreeSet<String>,
    values: BTreeMap<String, Vec<String>>,
    positionals: Vec<String>,
}

impl Parsed {
    /// Returns `true` when the boolean flag was given.
    pub fn is_set(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    /// Returns the last value of an option.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    /// Returns every value of a repeated option.
    pub fn values(&self, name: &str) -> &[String] {
        self.values.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Returns the positional arguments.
    pub fn positionals(&self) -> &[String] {
        &self.positionals
    }
}

/// Parses arguments against a list of option definitions.
pub fn parse(specs: &[FlagSpec], args: &[String]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut index = 0usize;
    let mut only_positional = false;

    while index < args.len() {
        let argument = args[index].as_str();
        index += 1;

        if only_positional {
            parsed.positionals.push(argument.to_owned());
            continue;
        }
        if argument == "--" {
            only_positional = true;
            continue;
        }

        if let Some(long) = argument.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (long, None),
            };
            let spec = specs
                .iter()
                .find(|spec| spec.long == name)
                .ok_or_else(|| format!("unknown option `--{name}`"))?;
            match spec.value {
                None => {
                    if inline.is_some() {
                        return Err(format!("`--{name}` does not take a value"));
                    }
                    parsed.flags.insert(spec.long.to_owned());
                }
                Some(_) => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            let value = args
                                .get(index)
                                .cloned()
                                .ok_or_else(|| format!("`--{name}` needs a value"))?;
                            index += 1;
                            value
                        }
                    };
                    parsed
                        .values
                        .entry(spec.long.to_owned())
                        .or_default()
                        .push(value);
                }
            }
            continue;
        }

        if argument.len() > 1 && argument.starts_with('-') && !argument.starts_with("--") {
            let short = argument.chars().nth(1).expect("the argument is not empty");
            let spec = specs
                .iter()
                .find(|spec| spec.short == Some(short))
                .ok_or_else(|| format!("unknown option `-{short}`"))?;
            match spec.value {
                None => {
                    parsed.flags.insert(spec.long.to_owned());
                }
                Some(_) => {
                    let value = args
                        .get(index)
                        .cloned()
                        .ok_or_else(|| format!("`-{short}` needs a value"))?;
                    index += 1;
                    parsed
                        .values
                        .entry(spec.long.to_owned())
                        .or_default()
                        .push(value);
                }
            }
            continue;
        }

        parsed.positionals.push(argument.to_owned());
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs() -> Vec<FlagSpec> {
        vec![
            FlagSpec::boolean_short("verbose", 'v', "verbose"),
            FlagSpec::value_short("model", 'm', "MODEL", "model"),
            FlagSpec::boolean("json", "json"),
        ]
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_values() {
        let parsed = parse(&specs(), &args(&["--verbose", "-m", "gpt-4o"])).unwrap();
        assert!(parsed.is_set("verbose"));
        assert_eq!(parsed.value("model"), Some("gpt-4o"));
    }

    #[test]
    fn parses_inline_values() {
        let parsed = parse(&specs(), &args(&["--model=gpt-4o"])).unwrap();
        assert_eq!(parsed.value("model"), Some("gpt-4o"));
    }

    #[test]
    fn collects_positionals() {
        let parsed = parse(&specs(), &args(&["one", "--json", "two"])).unwrap();
        assert_eq!(parsed.positionals(), ["one", "two"]);
        assert!(parsed.is_set("json"));
    }

    #[test]
    fn rejects_unknown_options() {
        let error = parse(&specs(), &args(&["--nope"])).unwrap_err();
        assert!(error.contains("--nope"));
    }

    #[test]
    fn reports_missing_values() {
        let error = parse(&specs(), &args(&["--model"])).unwrap_err();
        assert!(error.contains("needs a value"));
    }

    #[test]
    fn stops_at_the_double_dash() {
        let parsed = parse(&specs(), &args(&["--", "--not-an-option"])).unwrap();
        assert_eq!(parsed.positionals(), ["--not-an-option"]);
    }
}

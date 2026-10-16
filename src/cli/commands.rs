//! The commands of the `relay` binary.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::cli::args::Parsed;
use crate::cli::output::Style;
use crate::config::Config;
use crate::error::RelayError;
use crate::json::{Map, Value};
use crate::model::ModelRef;
use crate::types::{ChatRequest, Message, Role};

/// An error that the command line interface reports to the user.
#[derive(Debug)]
pub struct CliError {
    /// The message shown to the user.
    pub message: String,
    /// An optional hint that explains how to fix the problem.
    pub hint: Option<String>,
    /// Whether the problem is a usage error (exit code 2) or a runtime error.
    pub usage: bool,
}

impl CliError {
    /// Creates a runtime error.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
            usage: false,
        }
    }

    /// Creates a usage error.
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
            usage: true,
        }
    }

    /// Adds a hint.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl From<RelayError> for CliError {
    fn from(value: RelayError) -> Self {
        CliError::error(value.to_string())
    }
}

/// Loads the configuration, honouring `--config` and the environment.
pub fn load_config(parsed: &Parsed) -> Result<Config, CliError> {
    let mut config = match parsed.value("config") {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.exists() {
                return Err(CliError::error(format!(
                    "the configuration file `{}` does not exist",
                    path.display()
                ))
                .with_hint("run `relay init` to create one"));
            }
            Config::from_path(&path)?
        }
        None => Config::load()?,
    };
    config.apply_env(|key| std::env::var(key).ok())?;
    Ok(config)
}

/// `relay init` – writes a configuration file.
pub fn init(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let path = match parsed.value("path") {
        Some(path) => PathBuf::from(path),
        None => Config::default_path(),
    };
    let force = parsed.is_set("force");

    if path.exists() && !force {
        return Err(
            CliError::error(format!("`{}` already exists", path.display()))
                .with_hint("use `--force` to overwrite it"),
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            CliError::error(format!("could not create `{}`: {err}", parent.display()))
        })?;
    }
    std::fs::write(&path, crate::config::TEMPLATE)
        .map_err(|err| CliError::error(format!("could not write `{}`: {err}", path.display())))?;
    restrict_permissions(&path)?;

    if !style.quiet && !style.json {
        println!("{}", style.success("created"),);
        println!("  {}", path.display());
        println!();
        println!("Next steps:");
        println!("  1. add a provider and an API key to the file");
        println!("  2. run `relay doctor` to check the setup");
        println!("  3. run `relay chat -p \"hello\"`");
    }
    Ok(())
}

/// `relay providers` – lists the configured providers.
pub fn providers(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let config = load_config(parsed)?;
    let client = crate::Client::from_settings(config)?;
    let providers = client.providers();

    if style.json {
        let mut list = Vec::new();
        for provider in &providers {
            let mut object = Map::new();
            object.insert("name", provider.name.clone());
            object.insert("type", provider.kind.as_str());
            object.insert("endpoint", provider.description.clone());
            object.insert(
                "default_model",
                provider
                    .default_model
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::Null),
            );
            object.insert("has_credentials", provider.has_credentials);
            list.push(Value::Object(object));
        }
        println!("{}", Value::Array(list));
        return Ok(());
    }

    if providers.is_empty() {
        println!("No providers are configured.");
        println!("Add one to the configuration file, for example:");
        println!();
        println!("  [providers.ollama]");
        println!("  type = \"ollama\"");
        println!("  base_url = \"http://127.0.0.1:11434\"");
        return Ok(());
    }

    let width = providers
        .iter()
        .map(|provider| provider.name.len())
        .max()
        .unwrap_or(0);
    for provider in &providers {
        let name = style.accent(&format!("{:<width$}", provider.name, width = width));
        let model = provider
            .default_model
            .as_deref()
            .map(|model| format!("  {}", style.dim(model)))
            .unwrap_or_default();
        let key = if provider.has_credentials {
            style.dim("key")
        } else {
            String::new()
        };
        println!(
            "{name}  {}{model}  {key}{}",
            style.dim(provider.kind.as_str()),
            style.dim(&provider.description)
        );
    }
    Ok(())
}

/// `relay models` – lists the models of the providers.
pub fn models(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let config = load_config(parsed)?;
    let client = crate::Client::from_settings(config)?;
    let requested = parsed.positionals().first().cloned();

    let names = match requested {
        Some(name) => vec![name],
        None => client.provider_names(),
    };
    if names.is_empty() {
        return Err(CliError::error("no providers are configured")
            .with_hint("run `relay init` to create a configuration file"));
    }

    let refresh = parsed.is_set("refresh");
    let mut results: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    let mut failures = 0usize;

    for name in names {
        let models = if refresh {
            client.refresh_models(&name)
        } else {
            client.models(&name)
        };
        match models {
            Ok(models) => {
                let ids = models
                    .iter()
                    .map(|model| model.id().to_owned())
                    .collect::<Vec<_>>();
                results.push((name, ids, None));
            }
            Err(err) => {
                failures += 1;
                results.push((name, Vec::new(), Some(err.to_string())));
            }
        }
    }

    if style.json {
        let mut list = Vec::new();
        for (name, models, error) in &results {
            let mut object = Map::new();
            object.insert("provider", name.clone());
            object.insert(
                "models",
                Value::Array(models.iter().map(|m| Value::string(m.clone())).collect()),
            );
            if let Some(error) = error {
                object.insert("error", Value::string(error.clone()));
            }
            list.push(Value::Object(object));
        }
        println!("{}", Value::Array(list));
    } else {
        for (name, models, error) in &results {
            match error {
                Some(error) => {
                    println!("{}  {}", style.accent(name), style.error(error));
                }
                None if models.is_empty() => {
                    println!(
                        "{}  {}",
                        style.accent(name),
                        style.dim("no models reported")
                    );
                }
                None => {
                    println!("{}", style.accent(name));
                    for model in models {
                        println!("  {model}");
                    }
                }
            }
        }
    }

    if failures > 0 && !style.json {
        return Err(CliError::error(format!(
            "{failures} provider(s) could not be queried"
        )));
    }
    Ok(())
}

/// `relay chat` – asks a model a question.
pub fn chat(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let config = load_config(parsed)?;
    let client = crate::Client::from_settings(config)?;

    let model = match parsed.value("model") {
        Some(model) => Some(ModelRef::parse(model)?),
        None => client.default_model().cloned(),
    };
    let model = model.ok_or_else(|| {
        CliError::usage("no model selected")
            .with_hint("pass `--model provider/model` or set `default_model` in the configuration")
    })?;

    let mut request = ChatRequest::new(model.clone());
    if let Some(system) = parsed.value("system") {
        request = request.system(system);
    }
    if let Some(temperature) = parsed.value("temperature") {
        let value: f32 = temperature.parse().map_err(|_| {
            CliError::usage(format!(
                "`--temperature` must be a number, got `{temperature}`"
            ))
        })?;
        request = request.temperature(value);
    }
    if let Some(max_tokens) = parsed.value("max-tokens") {
        let value: u32 = max_tokens.parse().map_err(|_| {
            CliError::usage(format!(
                "`--max-tokens` must be a number, got `{max_tokens}`"
            ))
        })?;
        request = request.max_tokens(value);
    }
    if parsed.is_set("fallback") {
        return Err(CliError::error(
            "fallback is configured in the configuration file (`[fallback] enabled = true`)",
        ));
    }

    let interactive = parsed.is_set("interactive");
    let prompt = parsed.value("prompt").map(str::to_owned);
    let stream_output = !parsed.is_set("no-stream");

    if !interactive && prompt.is_none() {
        return Err(CliError::usage("no prompt given")
            .with_hint("pass `--prompt \"your question\"` or use `--interactive`"));
    }

    if interactive {
        run_interactive(&client, request, style, stream_output)
    } else {
        let prompt = prompt.expect("the prompt was checked above");
        let request = request.user(prompt);
        run_once(&client, request, style, stream_output)
    }
}

fn run_once(
    client: &crate::Client,
    request: ChatRequest,
    style: &Style,
    stream_output: bool,
) -> Result<(), CliError> {
    if !stream_output {
        let response = client.complete_blocking(request)?;
        if !style.json {
            println!("{}", response.text());
        }
        return Ok(());
    }

    let mut stream = client.stream_blocking(request)?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let mut wrote_anything = false;
    for chunk in stream.by_ref() {
        match chunk? {
            crate::StreamChunk::Text(text) => {
                wrote_anything = true;
                let _ = handle.write_all(text.as_bytes());
                let _ = handle.flush();
            }
            crate::StreamChunk::Done { .. } => break,
        }
    }
    if wrote_anything {
        let _ = handle.write_all(b"\n");
        let _ = handle.flush();
    }
    Ok(())
}

fn run_interactive(
    client: &crate::Client,
    request: ChatRequest,
    style: &Style,
    stream_output: bool,
) -> Result<(), CliError> {
    // A terminal gets the question prompt; a pipe is read silently, which
    // makes the mode usable in a script.
    let terminal = std::io::stdin().is_terminal();
    if !style.quiet && terminal {
        println!(
            "{}",
            style.dim("Type a question and press enter. Press Ctrl-D to exit.")
        );
    }

    let mut history: Vec<Message> = request.messages().to_vec();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        if !style.quiet && terminal {
            print!("{} ", style.accent("›"));
            let _ = std::io::stdout().flush();
        }
        let Some(line) = lines.next() else { break };
        let line =
            line.map_err(|err| CliError::error(format!("could not read the input: {err}")))?;
        let question = line.trim();
        if question.is_empty() {
            continue;
        }
        if question == "/exit" || question == "/quit" {
            break;
        }

        history.push(Message::new(Role::User, question));
        let mut turn = ChatRequest::new(request.model().clone());
        for message in &history {
            turn = turn.message(message.role(), message.content());
        }

        let answer = if stream_output {
            let mut stream = client.stream_blocking(turn)?;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            let mut answer = String::new();
            for chunk in stream.by_ref() {
                if let crate::StreamChunk::Text(text) = chunk? {
                    answer.push_str(&text);
                    let _ = handle.write_all(text.as_bytes());
                    let _ = handle.flush();
                }
            }
            let _ = handle.write_all(b"\n");
            let _ = handle.flush();
            answer
        } else {
            let response = client.complete_blocking(turn)?;
            println!("{}", response.text());
            response.text().to_owned()
        };

        history.push(Message::new(Role::Assistant, answer));
    }
    Ok(())
}

/// `relay doctor` – checks the local setup.
pub fn doctor(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let config = load_config(parsed)?;
    let mut checks: Vec<(String, String, bool)> = Vec::new();

    match &config.path {
        Some(path) if path.exists() => {
            checks.push(("config".to_owned(), path.display().to_string(), true));
            if let Some(warning) = permissions_warning(path) {
                checks.push(("config permissions".to_owned(), warning, false));
            }
        }
        Some(path) => {
            checks.push((
                "config".to_owned(),
                format!("{} (missing)", path.display()),
                false,
            ));
        }
        None => {
            checks.push(("config".to_owned(), "not found".to_owned(), false));
        }
    }

    checks.push((
        "default model".to_owned(),
        config
            .default_model
            .clone()
            .unwrap_or_else(|| "not set".to_owned()),
        config.default_model.is_some(),
    ));
    checks.push((
        "providers".to_owned(),
        format!("{} configured", config.providers.len()),
        !config.providers.is_empty(),
    ));

    let client = crate::Client::from_settings(config)?;
    let mut provider_results = Vec::new();
    for provider in client.providers() {
        let outcome = match client.models(&provider.name) {
            Ok(models) => (true, format!("{} models available", models.len())),
            Err(err) => (false, err.to_string()),
        };
        provider_results.push((provider, outcome));
    }

    if style.json {
        let mut root = Map::new();
        let mut check_list = Vec::new();
        for (name, detail, ok) in &checks {
            let mut object = Map::new();
            object.insert("name", name.clone());
            object.insert("detail", detail.clone());
            object.insert("ok", *ok);
            check_list.push(Value::Object(object));
        }
        root.insert("checks", Value::Array(check_list));
        let mut provider_list = Vec::new();
        for (provider, (ok, detail)) in &provider_results {
            let mut object = Map::new();
            object.insert("name", provider.name.clone());
            object.insert("type", provider.kind.as_str());
            object.insert("endpoint", provider.description.clone());
            object.insert("ok", *ok);
            object.insert("detail", detail.clone());
            provider_list.push(Value::Object(object));
        }
        root.insert("providers", Value::Array(provider_list));
        println!("{}", Value::Object(root));
    } else {
        println!("{}", style.bold("Relay doctor"));
        println!();
        for (name, detail, ok) in &checks {
            let mark = if *ok {
                style.success("ok  ")
            } else {
                style.warning("warn")
            };
            println!("  {mark}  {name:<18} {}", style.dim(detail));
        }
        if !provider_results.is_empty() {
            println!();
            println!("{}", style.bold("Providers"));
            for (provider, (ok, detail)) in &provider_results {
                let mark = if *ok {
                    style.success("ok  ")
                } else {
                    style.error("fail")
                };
                println!(
                    "  {mark}  {:<18} {}",
                    provider.name,
                    style.dim(&provider.description)
                );
                if !ok {
                    println!("        {}", style.dim(detail));
                }
            }
        }
        println!();
    }

    let failed = provider_results.iter().any(|(_, (ok, _))| !ok);
    if failed {
        return Err(CliError::error("some providers could not be reached"));
    }
    Ok(())
}

/// `relay serve` – runs the optional OpenAI compatible server.
pub fn serve(parsed: &Parsed, style: &Style) -> Result<(), CliError> {
    let config = load_config(parsed)?;
    let mut server = config.server.clone();
    if let Some(host) = parsed.value("host") {
        server.host = host.to_owned();
    }
    if let Some(port) = parsed.value("port") {
        server.port = port
            .parse()
            .map_err(|_| CliError::usage(format!("`--port` must be a number, got `{port}`")))?;
    }
    if parsed.is_set("allow-remote") {
        server.allow_remote = true;
    }

    let client = crate::Client::from_settings(config)?;
    #[cfg(feature = "server")]
    {
        let _ = style;
        crate::server::serve(client, server)?;
        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (client, server, style);
        Err(
            CliError::error("this build does not include the HTTP server")
                .with_hint("rebuild with `cargo install --features server relay-rs`"),
        )
    }
}

/// Writes a hint about a configuration file that is readable by others.
fn permissions_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path).ok()?;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "{} is readable by other users (mode {:o}); run `chmod 600 {}`",
                path.display(),
                mode & 0o777,
                path.display()
            ));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Restricts a file to the owner.
fn restrict_permissions(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|err| {
            CliError::error(format!(
                "could not restrict the permissions of `{}`: {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

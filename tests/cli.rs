//! Tests for the `relay` command line interface.

use std::path::PathBuf;
use std::process::Command;

use relay_rs::testing::{MockResponse, MockServer};

fn relay(arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(arguments)
        .output()
        .expect("the relay binary could not be started");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn temporary_path(name: &str) -> PathBuf {
    let unique = format!(
        "relay-test-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_nanos())
            .unwrap_or_default()
    );
    std::env::temp_dir().join(unique)
}

#[test]
fn prints_the_help() {
    let (code, stdout, _) = relay(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("chat"));
    assert!(stdout.contains("doctor"));
}

#[test]
fn prints_the_version() {
    let (code, stdout, _) = relay(&["--version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains(relay_rs::VERSION));
}

#[test]
fn every_command_has_help() {
    for command in ["init", "providers", "models", "chat", "doctor", "serve"] {
        let (code, stdout, stderr) = relay(&[command, "--help"]);
        assert_eq!(code, 0, "`relay {command} --help` failed: {stderr}");
        assert!(stdout.contains("Usage:"));
    }
}

#[test]
fn rejects_unknown_commands() {
    let (code, _, stderr) = relay(&["nonsense"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown command"));
}

#[test]
fn rejects_unknown_options() {
    let (code, _, stderr) = relay(&["providers", "--nonsense"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown option"));
}

#[test]
fn initialises_a_configuration_file() {
    let path = temporary_path("init");
    let path_text = path.to_string_lossy().to_string();
    let (code, stdout, stderr) = relay(&["init", "--path", &path_text]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("created"));

    let content = std::fs::read_to_string(&path).expect("the file was not written");
    assert!(content.contains("[providers.ollama]"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the file is not private");
    }

    let (code, _, stderr) = relay(&["init", "--path", &path_text]);
    assert_eq!(code, 1);
    assert!(stderr.contains("already exists"));

    let (code, _, _) = relay(&["init", "--path", &path_text, "--force"]);
    assert_eq!(code, 0);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn lists_the_configured_providers_as_json() {
    let path = temporary_path("providers");
    let path_text = path.to_string_lossy().to_string();
    let (code, _, stderr) = relay(&["init", "--path", &path_text]);
    assert_eq!(code, 0, "{stderr}");

    let (code, stdout, stderr) = relay(&["--config", &path_text, "providers", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let value = relay_rs::json::parse(&stdout).expect("the output is not JSON");
    let providers = value.as_array().expect("the output is not an array");
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].get("name").unwrap().as_str(), Some("ollama"));
    assert_eq!(providers[0].get("type").unwrap().as_str(), Some("ollama"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn chat_needs_a_prompt() {
    let path = temporary_path("chat");
    let path_text = path.to_string_lossy().to_string();
    relay(&["init", "--path", &path_text]);

    let (code, _, stderr) = relay(&["--config", &path_text, "chat"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("no prompt given"));

    let _ = std::fs::remove_file(&path);
}

#[test]
fn models_are_listed_from_the_provider() {
    let server = MockServer::start(vec![MockResponse::json(
        r#"{"data":[{"id":"model-a"},{"id":"model-b"}]}"#,
    )]);

    let path = temporary_path("models");
    let config = format!(
        "default_model = \"mock/model-a\"\n\n[providers.mock]\ntype = \"openai-compatible\"\nbase_url = \"http://{}/v1\"\n",
        server.address()
    );
    std::fs::write(&path, config).expect("the configuration was not written");
    let path_text = path.to_string_lossy().to_string();

    let (code, stdout, stderr) = relay(&["--config", &path_text, "models"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("model-a"));
    assert!(stdout.contains("model-b"));

    let (code, stdout, stderr) = relay(&["--config", &path_text, "models", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let value = relay_rs::json::parse(&stdout).expect("the output is not JSON");
    let providers = value.as_array().unwrap();
    let models = providers[0].get("models").unwrap().as_array().unwrap();
    assert_eq!(models.len(), 2);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn doctor_reports_the_setup() {
    let path = temporary_path("doctor");
    let path_text = path.to_string_lossy().to_string();
    relay(&["init", "--path", &path_text]);

    let (code, stdout, stderr) = relay(&["--config", &path_text, "doctor", "--json"]);
    // The providers of the template are not reachable in the test environment,
    // so the command reports the problem instead of pretending everything is
    // fine.
    assert!(code == 0 || code == 1, "{stderr}");
    let value = relay_rs::json::parse(&stdout).expect("the output is not JSON");
    assert!(value.get("checks").is_some());
    assert!(value.get("providers").is_some());

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_configuration_file_is_reported() {
    let (code, _, stderr) = relay(&["--config", "/definitely/not/here.toml", "providers"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("does not exist"));
}

#[test]
fn the_interactive_mode_keeps_the_conversation() {
    let server = MockServer::start(vec![
        MockResponse::json(r#"{"choices":[{"message":{"content":"first"}}]}"#),
        MockResponse::json(r#"{"choices":[{"message":{"content":"second"}}]}"#),
    ]);

    let path = temporary_path("interactive");
    let config = format!(
        "default_model = \"mock/model-a\"\n\n[providers.mock]\ntype = \"openai-compatible\"\nbase_url = \"http://{}/v1\"\n",
        server.address()
    );
    std::fs::write(&path, config).expect("the configuration was not written");
    let path_text = path.to_string_lossy().to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_relay"))
        .args([
            "--config",
            &path_text,
            "chat",
            "-m",
            "mock/model-a",
            "--interactive",
            "--no-stream",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the relay binary could not be started");
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().expect("the input is not piped");
        stdin
            .write_all(b"first question\nsecond question\n")
            .unwrap();
    }
    let output = child
        .wait_with_output()
        .expect("the process did not finish");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    assert!(output.status.success());
    assert!(stdout.contains("first"));
    assert!(stdout.contains("second"));
    assert_eq!(server.request_count(), 2);

    // The second question carries the history of the first one.
    let second = server.requests()[1].body_json().unwrap();
    let messages = second.get("messages").unwrap().as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[0].get("content").unwrap().as_str(),
        Some("first question")
    );
    assert_eq!(
        messages[2].get("content").unwrap().as_str(),
        Some("second question")
    );

    let _ = std::fs::remove_file(&path);
}

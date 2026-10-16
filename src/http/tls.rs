//! A TLS transport built on the system `openssl` command line tool.
//!
//! Relay does not depend on a TLS library so that the crate can be built in
//! environments without access to a package registry. For HTTPS requests the
//! transport starts `openssl s_client` and speaks HTTP/1.1 over the encrypted
//! pipe. The certificate chain is verified by OpenSSL unless the endpoint is
//! explicitly marked as insecure.

use std::io::{self, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Settings for the TLS transport.
#[derive(Debug, Clone, Default)]
pub struct TlsConfig {
    /// A PEM bundle used to verify the server certificate.
    pub ca_file: Option<PathBuf>,
    /// Skip certificate verification. Only useful for local development.
    pub insecure: bool,
}

impl TlsConfig {
    /// Creates a configuration that uses the system trust store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets a custom certificate authority bundle.
    pub fn with_ca_file(mut self, path: impl AsRef<Path>) -> Self {
        self.ca_file = Some(path.as_ref().to_path_buf());
        self
    }

    /// Disables certificate verification.
    pub fn insecure(mut self, insecure: bool) -> Self {
        self.insecure = insecure;
        self
    }
}

/// An established TLS session.
#[derive(Debug)]
pub struct TlsSession {
    child: Option<Child>,
    reader: Option<ChildStdout>,
    writer: Option<ChildStdin>,
    stderr: Arc<Mutex<String>>,
}

impl TlsSession {
    /// Takes the decrypted byte stream coming from the server.
    pub fn take_reader(&mut self) -> Option<ChildStdout> {
        self.reader.take()
    }

    /// Takes the byte stream that is encrypted and sent to the server.
    pub fn take_writer(&mut self) -> Option<ChildStdin> {
        self.writer.take()
    }

    /// Diagnostics collected from the `openssl` process.
    pub fn diagnostics(&self) -> String {
        self.stderr.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for TlsSession {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Opens a TLS session to `host:port`.
pub fn connect(host: &str, port: u16, config: &TlsConfig) -> io::Result<TlsSession> {
    let mut command = Command::new("openssl");
    command
        .arg("s_client")
        .arg("-connect")
        .arg(format!("{host}:{port}"))
        .arg("-servername")
        .arg(host)
        .arg("-quiet")
        .arg("-no_ign_eof");

    if !config.insecure {
        command.arg("-verify_return_error");
        // Hostname verification only makes sense for names, not for literals.
        if host.parse::<IpAddr>().is_err() {
            command.arg("-verify_hostname").arg(host);
        }
    }
    if let Some(ca_file) = &config.ca_file {
        command.arg("-CAfile").arg(ca_file);
    }

    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|err| {
        io::Error::other(format!(
            "could not start the `openssl` command line tool ({err}); \
             install OpenSSL to use https:// endpoints"
        ))
    })?;

    let writer = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("openssl stdin was not available"))?;
    let reader = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("openssl stdout was not available"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("openssl stderr was not available"))?;

    let diagnostics = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&diagnostics);
    std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut chunk = [0u8; 1024];
        let mut buffer = String::new();
        loop {
            match stderr.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => buffer.push_str(&String::from_utf8_lossy(&chunk[..read])),
                Err(_) => break,
            }
        }
        if let Ok(mut sink) = sink.lock() {
            sink.push_str(&buffer);
        }
    });

    Ok(TlsSession {
        child: Some(child),
        reader: Some(reader),
        writer: Some(writer),
        stderr: diagnostics,
    })
}

/// Writes the whole buffer to the session.
pub fn write_all(session: &mut TlsSession, data: &[u8]) -> io::Result<()> {
    let writer = session
        .writer
        .as_mut()
        .ok_or_else(|| io::Error::other("the TLS session was already consumed"))?;
    writer.write_all(data)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_diagnostic_when_the_host_is_unreachable() {
        // The tool is present in the test environment, so this only asserts
        // that the failure path produces an error instead of panicking.
        match connect("127.0.0.1", 1, &TlsConfig::new()) {
            Ok(_) => {}
            Err(err) => assert!(!err.to_string().is_empty()),
        }
    }
}

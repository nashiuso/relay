//! A small, dependency free TOML reader.
//!
//! Relay reads its configuration from a TOML file, so the crate ships a
//! focused parser instead of pulling in a registry dependency. The parser
//! understands the parts of TOML that matter for configuration files:
//!
//! * comments, bare and quoted keys, dotted keys
//! * tables (`[table]`) and nested tables (`[table.sub]`)
//! * arrays of tables (`[[table]]`)
//! * basic, literal and multi-line strings
//! * integers, floats, booleans, arrays and inline tables
//!
//! Date and time values are returned as strings; Relay does not use them.
//!
//! Values are represented with [`crate::json::Value`], the same dynamic value
//! type used by the JSON layer.

use std::fmt;

use crate::json::{Map, Value};

/// A TOML parse failure with the position where it occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TomlError {
    message: String,
    line: usize,
    column: usize,
}

impl TomlError {
    /// The human readable description of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The one based line the failure occurred on.
    pub fn line(&self) -> usize {
        self.line
    }

    /// The one based column the failure occurred on.
    pub fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for TomlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid TOML at line {} column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for TomlError {}

/// Parses a TOML document into a JSON value.
///
/// The document must contain a table at the root, which is the common case
/// for configuration files.
pub fn parse(input: &str) -> Result<Value, TomlError> {
    let mut parser = Parser {
        input,
        bytes: input.as_bytes(),
        pos: 0,
    };
    parser.parse_document()
}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

/// The path of the table that key/value pairs are currently written to.
type Path = Vec<String>;

impl<'a> Parser<'a> {
    fn error(&self, message: impl Into<String>) -> TomlError {
        let (line, column) = self.line_column();
        TomlError {
            message: message.into(),
            line,
            column,
        }
    }

    fn line_column(&self) -> (usize, usize) {
        let mut line = 1usize;
        let mut column = 1usize;
        for &byte in &self.bytes[..self.pos.min(self.bytes.len())] {
            if byte == b'\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        (line, column)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, text: &str) -> bool {
        self.input[self.pos..].starts_with(text)
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ') | Some(b'\t')) {
            self.pos += 1;
        }
    }

    fn skip_comment(&mut self) {
        if self.peek() == Some(b'#') {
            while let Some(byte) = self.peek() {
                if byte == b'\n' {
                    break;
                }
                self.pos += 1;
            }
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            self.skip_spaces();
            match self.peek() {
                Some(b'#') => self.skip_comment(),
                Some(b'\n') | Some(b'\r') => self.pos += 1,
                _ => break,
            }
        }
    }

    fn parse_document(&mut self) -> Result<Value, TomlError> {
        let mut root = Map::new();
        let mut current: Path = Vec::new();

        loop {
            self.skip_trivia();
            match self.peek() {
                None => break,
                Some(b'[') => {
                    let (path, is_array) = self.parse_table_header()?;
                    if is_array {
                        push_array_table(&mut root, &path).map_err(|m| self.error(m))?;
                        current = path;
                    } else {
                        ensure_table(&mut root, &path).map_err(|m| self.error(m))?;
                        current = path;
                    }
                }
                _ => {
                    let path = self.parse_key_path()?;
                    self.skip_spaces();
                    if self.peek() != Some(b'=') {
                        return Err(self.error("expected `=` after key"));
                    }
                    self.pos += 1;
                    self.skip_spaces();
                    let value = self.parse_value()?;
                    self.finish_line()?;
                    let mut full = current.clone();
                    full.extend(path);
                    insert_value(&mut root, &full, value).map_err(|message| self.error(message))?;
                }
            }
        }

        Ok(Value::Object(root))
    }

    fn finish_line(&mut self) -> Result<(), TomlError> {
        self.skip_spaces();
        self.skip_comment();
        match self.peek() {
            None => Ok(()),
            Some(b'\n') | Some(b'\r') => {
                self.pos += 1;
                Ok(())
            }
            Some(_) => Err(self.error("unexpected characters after value")),
        }
    }

    fn parse_table_header(&mut self) -> Result<(Path, bool), TomlError> {
        self.pos += 1; // consume '['
        let is_array = self.peek() == Some(b'[');
        if is_array {
            self.pos += 1;
        }
        self.skip_spaces();
        let path = self.parse_key_path()?;
        self.skip_spaces();
        if is_array {
            if !self.starts_with("]]") {
                return Err(self.error("expected `]]`"));
            }
            self.pos += 2;
        } else {
            if self.peek() != Some(b']') {
                return Err(self.error("expected `]`"));
            }
            self.pos += 1;
        }
        self.finish_line()?;
        Ok((path, is_array))
    }

    fn parse_key_path(&mut self) -> Result<Path, TomlError> {
        let mut path = Vec::new();
        loop {
            self.skip_spaces();
            let key = self.parse_key()?;
            path.push(key);
            self.skip_spaces();
            if self.peek() == Some(b'.') {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(path)
    }

    fn parse_key(&mut self) -> Result<String, TomlError> {
        match self.peek() {
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') => self.parse_literal_string(),
            _ => {
                let start = self.pos;
                while let Some(byte) = self.peek() {
                    if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                if start == self.pos {
                    return Err(self.error("expected a key"));
                }
                Ok(self.input[start..self.pos].to_owned())
            }
        }
    }

    fn parse_value(&mut self) -> Result<Value, TomlError> {
        match self.peek() {
            None => Err(self.error("expected a value")),
            Some(b'"') => {
                if self.starts_with("\"\"\"") {
                    self.parse_multiline_basic_string().map(Value::String)
                } else {
                    self.parse_basic_string().map(Value::String)
                }
            }
            Some(b'\'') => {
                if self.starts_with("'''") {
                    self.parse_multiline_literal_string().map(Value::String)
                } else {
                    self.parse_literal_string().map(Value::String)
                }
            }
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_inline_table(),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'+') | Some(b'-') | Some(b'0'..=b'9') => self.parse_number_or_datetime(),
            Some(_) => Err(self.error("expected a value")),
        }
    }

    fn parse_bool(&mut self) -> Result<Value, TomlError> {
        if self.starts_with("true") {
            self.pos += 4;
            Ok(Value::Bool(true))
        } else if self.starts_with("false") {
            self.pos += 5;
            Ok(Value::Bool(false))
        } else {
            Err(self.error("expected `true` or `false`"))
        }
    }

    fn parse_array(&mut self) -> Result<Value, TomlError> {
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.error("unterminated array")),
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => {
                    let value = self.parse_value()?;
                    items.push(value);
                    self.skip_trivia();
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Value::Array(items));
                        }
                        _ => return Err(self.error("expected `,` or `]`")),
                    }
                }
            }
        }
    }

    fn parse_inline_table(&mut self) -> Result<Value, TomlError> {
        self.pos += 1; // consume '{'
        let mut map = Map::new();
        loop {
            self.skip_spaces();
            match self.peek() {
                None => return Err(self.error("unterminated inline table")),
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(map));
                }
                _ => {
                    let path = self.parse_key_path()?;
                    self.skip_spaces();
                    if self.peek() != Some(b'=') {
                        return Err(self.error("expected `=` after key"));
                    }
                    self.pos += 1;
                    self.skip_spaces();
                    let value = self.parse_value()?;
                    insert_value(&mut map, &path, value).map_err(|message| self.error(message))?;
                    self.skip_spaces();
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                        }
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Value::Object(map));
                        }
                        _ => return Err(self.error("expected `,` or `}`")),
                    }
                }
            }
        }
    }

    fn parse_number_or_datetime(&mut self) -> Result<Value, TomlError> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.' | b':' | b'_') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let raw = &self.input[start..self.pos];
        if raw.is_empty() {
            return Err(self.error("expected a value"));
        }
        if let Some(value) = parse_integer(raw) {
            return Ok(Value::Number(value));
        }
        if let Some(value) = parse_float(raw) {
            return Ok(Value::Number(value));
        }
        if looks_like_datetime(raw) {
            return Ok(Value::String(raw.to_owned()));
        }
        Err(self.error(format!("invalid value `{raw}`")))
    }

    fn parse_basic_string(&mut self) -> Result<String, TomlError> {
        self.pos += 1; // consume '"'
        let mut out = String::new();
        loop {
            let byte = match self.peek() {
                Some(b) => b,
                None => return Err(self.error("unterminated string")),
            };
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\n' => return Err(self.error("unterminated string")),
                b'\\' => {
                    self.pos += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' | b'U' => {
                            let width = if escape == b'u' { 4 } else { 8 };
                            let code = self.parse_hex(width)?;
                            let ch = char::from_u32(code)
                                .ok_or_else(|| self.error("invalid unicode escape"))?;
                            out.push(ch);
                        }
                        other => {
                            return Err(self.error(format!("invalid escape `\\{}`", other as char)));
                        }
                    }
                }
                _ => {
                    let ch = self.next_char()?;
                    out.push(ch);
                }
            }
        }
    }

    fn parse_multiline_basic_string(&mut self) -> Result<String, TomlError> {
        self.pos += 3;
        if self.peek() == Some(b'\n') {
            self.pos += 1;
        } else if self.starts_with("\r\n") {
            self.pos += 2;
        }
        let mut out = String::new();
        loop {
            if self.starts_with("\"\"\"") {
                self.pos += 3;
                return Ok(out);
            }
            if self.peek().is_none() {
                return Err(self.error("unterminated multi-line string"));
            }
            if self.peek() == Some(b'\\') {
                // A backslash at the end of a line trims the following whitespace.
                let mut look = self.pos + 1;
                while matches!(self.bytes.get(look), Some(b' ') | Some(b'\t') | Some(b'\r')) {
                    look += 1;
                }
                if self.bytes.get(look) == Some(&b'\n') {
                    self.pos = look + 1;
                    while matches!(
                        self.peek(),
                        Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
                    ) {
                        self.pos += 1;
                    }
                    continue;
                }
            }
            if self.peek() == Some(b'\\') {
                self.pos += 1;
                let escape = self
                    .peek()
                    .ok_or_else(|| self.error("unterminated escape"))?;
                self.pos += 1;
                match escape {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' | b'U' => {
                        let width = if escape == b'u' { 4 } else { 8 };
                        let code = self.parse_hex(width)?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| self.error("invalid unicode escape"))?;
                        out.push(ch);
                    }
                    other => {
                        return Err(self.error(format!("invalid escape `\\{}`", other as char)));
                    }
                }
                continue;
            }
            let ch = self.next_char()?;
            out.push(ch);
        }
    }

    fn parse_literal_string(&mut self) -> Result<String, TomlError> {
        self.pos += 1; // consume '\''
        let start = self.pos;
        loop {
            match self.peek() {
                None | Some(b'\n') => return Err(self.error("unterminated literal string")),
                Some(b'\'') => {
                    let text = self.input[start..self.pos].to_owned();
                    self.pos += 1;
                    return Ok(text);
                }
                _ => {
                    self.next_char()?;
                }
            }
        }
    }

    fn parse_multiline_literal_string(&mut self) -> Result<String, TomlError> {
        self.pos += 3;
        if self.peek() == Some(b'\n') {
            self.pos += 1;
        } else if self.starts_with("\r\n") {
            self.pos += 2;
        }
        let start = self.pos;
        loop {
            if self.starts_with("'''") {
                let text = self.input[start..self.pos].to_owned();
                self.pos += 3;
                return Ok(text);
            }
            if self.peek().is_none() {
                return Err(self.error("unterminated multi-line literal string"));
            }
            self.next_char()?;
        }
    }

    fn next_char(&mut self) -> Result<char, TomlError> {
        let rest = &self.input[self.pos..];
        let ch = rest
            .chars()
            .next()
            .ok_or_else(|| self.error("unexpected end of input"))?;
        self.pos += ch.len_utf8();
        Ok(ch)
    }

    fn parse_hex(&mut self, width: usize) -> Result<u32, TomlError> {
        if self.pos + width > self.bytes.len() {
            return Err(self.error("truncated unicode escape"));
        }
        let text = &self.input[self.pos..self.pos + width];
        let value =
            u32::from_str_radix(text, 16).map_err(|_| self.error("invalid unicode escape"))?;
        self.pos += width;
        Ok(value)
    }
}

fn parse_integer(raw: &str) -> Option<crate::json::Number> {
    let cleaned = raw.replace('_', "");
    let (sign, digits) = match cleaned.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, cleaned.strip_prefix('+').unwrap_or(&cleaned)),
    };
    if let Some(hex) = digits.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16)
            .ok()
            .map(|v| crate::json::Number::Int(sign * v));
    }
    if let Some(oct) = digits.strip_prefix("0o") {
        return i64::from_str_radix(oct, 8)
            .ok()
            .map(|v| crate::json::Number::Int(sign * v));
    }
    if let Some(bin) = digits.strip_prefix("0b") {
        return i64::from_str_radix(bin, 2)
            .ok()
            .map(|v| crate::json::Number::Int(sign * v));
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<i64>()
        .ok()
        .map(|v| crate::json::Number::Int(sign * v))
}

fn parse_float(raw: &str) -> Option<crate::json::Number> {
    let cleaned = raw.replace('_', "");
    if cleaned.contains(':') || cleaned.matches('-').count() > 1 {
        return None;
    }
    if !cleaned.contains('.') && !cleaned.contains('e') && !cleaned.contains('E') {
        return None;
    }
    cleaned.parse::<f64>().ok().map(crate::json::Number::Float)
}

fn looks_like_datetime(raw: &str) -> bool {
    // Dates look like 2024-01-31, times like 07:30:00.
    let bytes = raw.as_bytes();
    if bytes.len() >= 10 && bytes[4] == b'-' && bytes[7] == b'-' {
        return bytes[..4].iter().all(u8::is_ascii_digit);
    }
    raw.contains(':')
}

/// Creates the table at `path`, if it does not exist yet.
fn ensure_table(root: &mut Map, path: &[String]) -> Result<(), String> {
    let Some((segment, rest)) = path.split_first() else {
        return Ok(());
    };
    if !root.contains_key(segment) {
        root.insert(segment.clone(), Value::Object(Map::new()));
    }
    match root.get_mut(segment) {
        Some(Value::Object(map)) => ensure_table(map, rest),
        Some(Value::Array(items)) => match items.last_mut() {
            Some(Value::Object(map)) => ensure_table(map, rest),
            _ => Err(format!("cannot redefine `{segment}` as a table")),
        },
        _ => Err(format!("cannot redefine `{segment}` as a table")),
    }
}

/// Appends a new table to the array of tables at `path`.
fn push_array_table(root: &mut Map, path: &[String]) -> Result<(), String> {
    let Some((last, parents)) = path.split_last() else {
        return Err("empty table name".to_owned());
    };
    ensure_table(root, parents)?;
    let parent = descend(root, parents)?;
    if !parent.contains_key(last) {
        parent.insert(last.clone(), Value::Array(Vec::new()));
    }
    match parent.get_mut(last) {
        Some(Value::Array(items)) => {
            items.push(Value::Object(Map::new()));
            Ok(())
        }
        _ => Err(format!("cannot redefine `{last}` as an array of tables")),
    }
}

/// Stores `value` at `path`, creating intermediate tables on the way.
fn insert_value(root: &mut Map, path: &[String], value: Value) -> Result<(), String> {
    let Some((last, parents)) = path.split_last() else {
        return Err("empty key".to_owned());
    };
    let parent = descend(root, parents)?;
    parent.insert(last.clone(), value);
    Ok(())
}

/// Returns the table addressed by `path`, creating missing tables.
fn descend<'a>(root: &'a mut Map, path: &[String]) -> Result<&'a mut Map, String> {
    let Some((segment, rest)) = path.split_first() else {
        return Ok(root);
    };
    if !root.contains_key(segment) {
        root.insert(segment.clone(), Value::Object(Map::new()));
    }
    match root.get_mut(segment) {
        Some(Value::Object(map)) => descend(map, rest),
        Some(Value::Array(items)) => match items.last_mut() {
            Some(Value::Object(map)) => descend(map, rest),
            _ => Err(format!("cannot use `{segment}` as a table")),
        },
        _ => Err(format!("cannot use `{segment}` as a table")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_key_values() {
        let doc = parse("a = 1\nb = \"two\"\nc = true\n").unwrap();
        assert_eq!(doc.get("a").unwrap().as_i64(), Some(1));
        assert_eq!(doc.get("b").unwrap().as_str(), Some("two"));
        assert_eq!(doc.get("c").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn parses_tables_and_dotted_keys() {
        let doc = parse(
            r#"
            [server]
            host = "127.0.0.1"
            port = 8080

            [server.tls]
            enabled = false
            "#,
        )
        .unwrap();
        let server = doc.get("server").unwrap();
        assert_eq!(server.get("host").unwrap().as_str(), Some("127.0.0.1"));
        let tls = server.get("tls").unwrap();
        assert_eq!(tls.get("enabled").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn parses_arrays_and_inline_tables() {
        let doc = parse(
            r#"
            models = ["a", "b"]
            inline = { x = 1, y = "z" }
            "#,
        )
        .unwrap();
        let models = doc.get("models").unwrap().as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].as_str(), Some("b"));
        assert_eq!(
            doc.get("inline").unwrap().get("y").unwrap().as_str(),
            Some("z")
        );
    }

    #[test]
    fn parses_array_of_tables() {
        let doc = parse(
            r#"
            [[fallback]]
            model = "openai/gpt-4o"

            [[fallback]]
            model = "ollama/llama3"
            "#,
        )
        .unwrap();
        let list = doc.get("fallback").unwrap().as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[1].get("model").unwrap().as_str(),
            Some("ollama/llama3")
        );
    }

    #[test]
    fn parses_multiline_strings() {
        let doc = parse("text = \"\"\"\nline one\nline two\"\"\"\n").unwrap();
        assert_eq!(
            doc.get("text").unwrap().as_str(),
            Some("line one\nline two")
        );
    }

    #[test]
    fn ignores_comments() {
        let doc = parse("# comment\nvalue = 1 # trailing\n").unwrap();
        assert_eq!(doc.get("value").unwrap().as_i64(), Some(1));
    }

    #[test]
    fn reports_errors_with_position() {
        let err = parse("a = \n").unwrap_err();
        assert_eq!(err.line(), 1);
        assert!(err.message().contains("expected a value"));

        let err = parse("a 1\n").unwrap_err();
        assert!(err.message().contains("expected `=`"));
    }

    #[test]
    fn rejects_unterminated_tables() {
        assert!(parse("[server\n").is_err());
        assert!(parse("a = \"unterminated\n").is_err());
    }
}

//! A small, dependency free JSON implementation.
//!
//! Relay ships its own JSON layer so that the core library can be used in
//! environments without access to a package registry. The implementation
//! covers the whole JSON grammar (RFC 8259) and is used for provider
//! payloads, configuration overrides and the optional HTTP server.
//!
//! Values are represented by [`Value`]; objects keep their insertion order so
//! that generated payloads stay readable and stable.

use std::fmt;

/// A JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`
    Null,
    /// `true` / `false`
    Bool(bool),
    /// A number.
    Number(Number),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object.
    Object(Map),
}

/// A JSON number.
///
/// Integers and floating point values are kept apart so that a document
/// survives a parse/serialize round trip without turning `1` into `1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    /// A signed integer.
    Int(i64),
    /// A floating point number.
    Float(f64),
}

impl Number {
    /// Returns the value as a `f64`, if it is representable.
    pub fn as_f64(self) -> f64 {
        match self {
            Number::Int(v) => v as f64,
            Number::Float(v) => v,
        }
    }

    /// Returns the value as an `i64`, if it is an exact integer.
    pub fn as_i64(self) -> Option<i64> {
        match self {
            Number::Int(v) => Some(v),
            Number::Float(v) => {
                if v.fract() == 0.0 && v.is_finite() && v >= i64::MIN as f64 && v <= i64::MAX as f64
                {
                    Some(v as i64)
                } else {
                    None
                }
            }
        }
    }
}

/// An insertion ordered JSON object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// Creates an empty object.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a key, replacing an existing value but keeping its position.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        for entry in &mut self.entries {
            if entry.0 == key {
                return Some(std::mem::replace(&mut entry.1, value));
            }
        }
        self.entries.push((key, value));
        None
    }

    /// Returns a reference to the value stored under `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Returns a mutable reference to the value stored under `key`.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    /// Removes and returns the value stored under `key`.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }

    /// Returns `true` when the object contains `key`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// The number of entries in the object.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when the object has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterates over the entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Consumes the object and returns its entries.
    pub fn into_entries(self) -> Vec<(String, Value)> {
        self.entries
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        let mut map = Map::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl Value {
    /// Creates an empty object.
    pub fn object() -> Self {
        Value::Object(Map::new())
    }

    /// Creates a string value.
    pub fn string(value: impl Into<String>) -> Self {
        Value::String(value.into())
    }

    /// Creates a number from a `f32`.
    ///
    /// The shortest decimal form of the value is used so that `0.1` stays
    /// `0.1` instead of becoming `0.10000000149011612`.
    pub fn from_f32(value: f32) -> Self {
        let text = value.to_string();
        let parsed = text.parse::<f64>().unwrap_or(value as f64);
        Value::Number(Number::Float(parsed))
    }

    /// Returns `true` for `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Returns the contained string, if any.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the contained boolean, if any.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the contained number as `f64`, if any.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(n.as_f64()),
            _ => None,
        }
    }

    /// Returns the contained number as `i64`, if it is an exact integer.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    /// Returns the contained array, if any.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Returns the contained object, if any.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Looks up `key` when the value is an object.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|map| map.get(key))
    }

    /// Serializes the value as compact JSON.
    pub fn to_string_pretty(&self) -> String {
        let mut out = String::new();
        write_pretty(self, 0, &mut out);
        out
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_compact(self, &mut out);
        f.write_str(&out)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Number(Number::Int(value))
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Value::Number(Number::Int(value as i64))
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::Number(Number::Int(value as i64))
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Value::Number(Number::Int(value as i64))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Number(Number::Float(value))
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_owned())
    }
}

impl From<Map> for Value {
    fn from(value: Map) -> Self {
        Value::Object(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Value::Array(value)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

/// A JSON parse failure with the position where it occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    message: String,
    line: usize,
    column: usize,
}

impl JsonError {
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

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid JSON at line {} column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl std::error::Error for JsonError {}

const MAX_DEPTH: usize = 64;

/// Parses a JSON document.
pub fn parse(input: &str) -> Result<Value, JsonError> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    parser.skip_whitespace();
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    if parser.pos < parser.bytes.len() {
        return Err(parser.error("unexpected trailing characters"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn error(&self, message: &str) -> JsonError {
        let (line, column) = self.line_column();
        JsonError {
            message: message.to_owned(),
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

    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.peek() {
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected `{}`", byte as char)))
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.error("maximum nesting depth exceeded"));
        }
        match self.peek() {
            None => Err(self.error("unexpected end of input")),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => {
                self.expect_literal("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.expect_literal("null")?;
                Ok(Value::Null)
            }
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(other) => Err(self.error(&format!("unexpected character `{}`", other as char))),
        }
    }

    fn expect_literal(&mut self, literal: &str) -> Result<(), JsonError> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(())
        } else {
            Err(self.error(&format!("expected `{literal}`")))
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.expect(b'{')?;
        let mut map = Map::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            let value = self.parse_value(depth + 1)?;
            map.insert(key, value);
            self.skip_whitespace();
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

    fn parse_array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
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

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
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
                b'\\' => {
                    self.pos += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                if self.peek() == Some(b'\\') {
                                    self.pos += 1;
                                    self.expect(b'u')?;
                                    let low = self.parse_hex4()?;
                                    if !(0xDC00..0xE000).contains(&low) {
                                        return Err(self.error("invalid low surrogate"));
                                    }
                                    let combined = 0x10000
                                        + (((code - 0xD800) as u32) << 10)
                                        + (low - 0xDC00) as u32;
                                    out.push(
                                        char::from_u32(combined)
                                            .ok_or_else(|| self.error("invalid code point"))?,
                                    );
                                } else {
                                    return Err(self.error("unpaired high surrogate"));
                                }
                            } else {
                                out.push(
                                    char::from_u32(code as u32)
                                        .ok_or_else(|| self.error("invalid code point"))?,
                                );
                            }
                        }
                        other => {
                            return Err(
                                self.error(&format!("invalid escape `\\{}`", other as char))
                            );
                        }
                    }
                }
                b if b < 0x20 => {
                    return Err(self.error("control character in string"));
                }
                _ => {
                    // Copy the next UTF-8 character verbatim.
                    let start = self.pos;
                    let len = utf8_len(byte);
                    if start + len > self.bytes.len() {
                        return Err(self.error("truncated UTF-8 sequence"));
                    }
                    let slice = &self.bytes[start..start + len];
                    match std::str::from_utf8(slice) {
                        Ok(text) => out.push_str(text),
                        Err(_) => return Err(self.error("invalid UTF-8 sequence")),
                    }
                    self.pos += len;
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u16, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.error("truncated unicode escape"));
        }
        let text = std::str::from_utf8(&self.bytes[self.pos..self.pos + 4])
            .map_err(|_| self.error("invalid unicode escape"))?;
        let value =
            u16::from_str_radix(text, 16).map_err(|_| self.error("invalid unicode escape"))?;
        self.pos += 4;
        Ok(value)
    }

    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        let int_start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == int_start {
            return Err(self.error("expected digits"));
        }
        // JSON does not allow leading zeros such as `01`.
        if self.pos - int_start > 1 && self.bytes[int_start] == b'0' {
            return Err(self.error("numbers must not have leading zeros"));
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            let frac_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == frac_start {
                return Err(self.error("expected digits after `.`"));
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            let exp_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == exp_start {
                return Err(self.error("expected digits in exponent"));
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.error("invalid number"))?;
        if !is_float {
            if let Ok(value) = text.parse::<i64>() {
                return Ok(Value::Number(Number::Int(value)));
            }
        }
        let value = text
            .parse::<f64>()
            .map_err(|_| self.error("invalid number"))?;
        Ok(Value::Number(Number::Float(value)))
    }
}

fn utf8_len(byte: u8) -> usize {
    match byte {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(Number::Int(v)) => out.push_str(&v.to_string()),
        Value::Number(Number::Float(v)) => out.push_str(&format_float(*v)),
        Value::String(s) => write_escaped(s, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_escaped(key, out);
                out.push(':');
                write_compact(item, out);
            }
            out.push('}');
        }
    }
}

fn write_pretty(value: &Value, indent: usize, out: &mut String) {
    let pad = "  ".repeat(indent);
    let inner_pad = "  ".repeat(indent + 1);
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                out.push_str(&inner_pad);
                write_pretty(item, indent + 1, out);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push(']');
        }
        Value::Object(map) if !map.is_empty() => {
            out.push_str("{\n");
            let count = map.len();
            for (index, (key, item)) in map.iter().enumerate() {
                out.push_str(&inner_pad);
                write_escaped(key, out);
                out.push_str(": ");
                write_pretty(item, indent + 1, out);
                if index + 1 < count {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&pad);
            out.push('}');
        }
        other => write_compact(other, out),
    }
}

fn format_float(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    let text = format!("{value}");
    if text.contains('.') || text.contains('e') || text.contains('E') {
        text
    } else {
        format!("{text}.0")
    }
}

fn write_escaped(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars() {
        assert_eq!(parse("null").unwrap(), Value::Null);
        assert_eq!(parse("true").unwrap(), Value::Bool(true));
        assert_eq!(parse("false").unwrap(), Value::Bool(false));
        assert_eq!(parse("42").unwrap(), Value::Number(Number::Int(42)));
        assert_eq!(parse("-7").unwrap(), Value::Number(Number::Int(-7)));
        assert_eq!(parse("1.5").unwrap(), Value::Number(Number::Float(1.5)));
        assert_eq!(parse("\"hi\"").unwrap(), Value::string("hi"));
    }

    #[test]
    fn parses_nested_documents() {
        let value = parse(r#"{"a":[1,2,{"b":null}],"c":"d"}"#).unwrap();
        assert_eq!(value.get("c").unwrap().as_str(), Some("d"));
        let a = value.get("a").unwrap().as_array().unwrap();
        assert_eq!(a.len(), 3);
        assert!(a[2].get("b").unwrap().is_null());
    }

    #[test]
    fn handles_escapes_and_surrogates() {
        let value = parse(r#""line\nbreak \u0041 \ud83d\ude00""#).unwrap();
        assert_eq!(value.as_str().unwrap(), "line\nbreak A 😀");
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("nul").is_err());
        assert!(parse("{\"a\":1} trailing").is_err());
        assert!(parse("01").is_err());
    }

    #[test]
    fn reports_error_position() {
        let err = parse("{\n  \"a\": tru\n}").unwrap_err();
        assert_eq!(err.line(), 2);
        assert!(err.message().contains("expected `true`"));
    }

    #[test]
    fn serializes_compactly_and_preserves_order() {
        let mut map = Map::new();
        map.insert("z", 1i64);
        map.insert("a", "two");
        let value = Value::Object(map);
        assert_eq!(value.to_string(), r#"{"z":1,"a":"two"}"#);
    }

    #[test]
    fn escapes_control_characters() {
        let value = Value::string("tab\there\u{1}");
        assert_eq!(value.to_string(), "\"tab\\there\\u0001\"");
    }

    #[test]
    fn round_trips_unicode() {
        let original = Value::string("héllo wörld 🌍");
        let text = original.to_string();
        assert_eq!(parse(&text).unwrap(), original);
    }

    #[test]
    fn pretty_prints_with_indentation() {
        let mut map = Map::new();
        map.insert(
            "a",
            Value::Array(vec![Value::from(1i64), Value::from(2i64)]),
        );
        let text = Value::Object(map).to_string_pretty();
        assert!(text.contains("\n  \"a\": ["));
    }

    #[test]
    fn rejects_excessive_nesting() {
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(&deep).is_err());
    }
}

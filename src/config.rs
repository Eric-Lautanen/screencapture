//! Tiny settings format: ordered `key = value` pairs under `[section]`
//! headers with `#` comments and typed values, implemented with `std` only.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

/// A single typed value inside a settings document.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Free-form text.
    Str(String),
    /// Whole number.
    Int(i64),
    /// On/off flag.
    Bool(bool),
}

impl Value {
    /// Interpret `text`: quoted string, `true`/`false`, integer, or bare word.
    fn parse(text: &str) -> Self {
        let text = text.trim();
        if let Some(body) = text.strip_prefix('"') {
            return Self::Str(quoted_body(body).to_owned());
        }
        if text == "true" {
            return Self::Bool(true);
        }
        if text == "false" {
            return Self::Bool(false);
        }
        if let Ok(number) = text.parse::<i64>() {
            return Self::Int(number);
        }
        Self::Str(text.to_owned())
    }

    /// Text content, if any.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// Flag content, if any.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }
}

/// Render a value; strings are quoted only when needed (see the file-format
/// notes in the module docs).
impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Str(text) => {
                if text.is_empty() || text.contains('#') || text.chars().any(char::is_whitespace) {
                    write!(formatter, "\"{text}\"")
                } else {
                    write!(formatter, "{text}")
                }
            }
            Self::Int(number) => write!(formatter, "{number}"),
            Self::Bool(flag) => write!(formatter, "{flag}"),
        }
    }
}

/// Parse error with a 1-based line number.
#[derive(Debug)]
pub struct ParseError {
    /// Line the error was detected on.
    pub line: usize,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// An ordered settings document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    sections: Vec<Section>,
}

/// One `[section]` and its entries, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
struct Section {
    name: String,
    entries: Vec<(String, Value)>,
}

impl Section {
    fn set(&mut self, key: &str, value: Value) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|(name, _)| name.as_str() == key)
        {
            existing.1 = value;
        } else {
            self.entries.push((key.to_owned(), value));
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(name, _)| name.as_str() == key)
            .map(|(_, value)| value)
    }
}

impl Document {
    /// Parse from text, reporting the offending line on failure.
    ///
    /// # Errors
    /// Returns a [`ParseError`] on malformed lines, headers, or keys.
    pub fn from_text(text: &str) -> Result<Self, ParseError> {
        let mut document = Self::default();
        let mut section: Option<String> = None;
        for (offset, line) in text.lines().enumerate() {
            let number = offset + 1;
            let content = strip_comment(line).trim();
            if content.is_empty() {
                continue;
            }
            if let Some(head) = content.strip_prefix('[') {
                let Some(name) = head.strip_suffix(']') else {
                    return Err(ParseError {
                        line: number,
                        message: "section header must end with ']'".into(),
                    });
                };
                let name = name.trim();
                if name.is_empty() {
                    return Err(ParseError {
                        line: number,
                        message: "empty section name".into(),
                    });
                }
                section = Some(name.to_owned());
                continue;
            }
            let Some((key, value)) = content.split_once('=') else {
                return Err(ParseError {
                    line: number,
                    message: "expected `key = value` or a [section]".into(),
                });
            };
            let key = key.trim();
            if key.is_empty() {
                return Err(ParseError {
                    line: number,
                    message: "empty key".into(),
                });
            }
            let Some(name) = section.as_ref() else {
                return Err(ParseError {
                    line: number,
                    message: "entry before the first [section]".into(),
                });
            };
            section_mut(&mut document, name).set(key, Value::parse(value));
        }
        Ok(document)
    }

    /// Look up a value by `section` and `key`.
    #[must_use]
    pub fn value(&self, section: &str, key: &str) -> Option<&Value> {
        self.sections
            .iter()
            .find(|candidate| candidate.name.as_str() == section)?
            .get(key)
    }

    /// Render back to file-order text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for section in &self.sections {
            let _ = writeln!(&mut text, "[{}]", section.name);
            for (key, value) in &section.entries {
                let _ = writeln!(&mut text, "{key} = {value}");
            }
            let _ = writeln!(&mut text);
        }
        text
    }

    /// Write the rendered document to `path`, creating parent directories.
    ///
    /// # Errors
    /// Propagates filesystem errors from directory creation or writing.
    pub fn persist(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, self.to_text())
    }
}

fn section_mut<'doc>(document: &'doc mut Document, name: &str) -> &'doc mut Section {
    let index = if let Some(index) = document
        .sections
        .iter()
        .position(|candidate| candidate.name.as_str() == name)
    {
        index
    } else {
        document.sections.push(Section {
            name: name.to_owned(),
            entries: Vec::new(),
        });
        document.sections.len() - 1
    };
    document.sections.get_mut(index).expect("index is in range")
}

/// Remove a `#` comment; `#` inside a quoted string is preserved.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (index, character) in line.char_indices() {
        match character {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
}

/// The substring of a quoted value up to the closing quote; the opening quote
/// has already been removed. Quotes inside values are not supported.
fn quoted_body(text: &str) -> &str {
    match text.find('"') {
        Some(end) => &text[..end],
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::{Document, Value};

    #[test]
    fn parses_sections_values_and_comments() {
        let document = Document::from_text(
            "# top\n[alpha]\nflag = true\ncount = 42\nname = hello\n# tail\n[beta]\npad = \"two words\"\n",
        )
        .expect("sample must parse");
        assert_eq!(document.value("alpha", "flag"), Some(&Value::Bool(true)));
        assert_eq!(document.value("alpha", "count"), Some(&Value::Int(42)));
        assert_eq!(
            document.value("alpha", "name"),
            Some(&Value::Str("hello".to_owned()))
        );
        assert_eq!(
            document.value("beta", "pad"),
            Some(&Value::Str("two words".to_owned()))
        );
        assert!(document.value("missing", "x").is_none());
    }

    #[test]
    fn reports_line_numbers_on_errors() {
        let error = Document::from_text("[a]\nok = 1\nbroken\n").expect_err("must fail");
        assert_eq!(error.line, 3);
    }

    #[test]
    fn round_trips_render_and_reparses() {
        let original = Document::from_text("[a]\ns = \"spaced value\"\nn = 7\nflag = true\n")
            .expect("must parse");
        let reparsed =
            Document::from_text(&original.to_text()).expect("rendered text must reparse");
        assert_eq!(original, reparsed);
    }
}

/// Parse the contents of a .env file into key-value pairs.
///
/// Supports:
/// - `#` and `//` line comments
/// - `KEY=VALUE`, `KEY = VALUE` (spaces around `=`)
/// - Single-quoted, double-quoted, and unquoted values
/// - Multi-line values inside double or single quotes
/// - `KEY=` (empty string value)
/// - Blank lines are ignored
pub fn parse_env_file(content: &str) -> Result<Vec<(String, String)>, (usize, String)> {
	// A UTF-8 byte-order mark is not part of the first key. Windows PowerShell
	// 5.1's `Set-Content -Encoding UTF8` and other Windows tools write one, and it
	// made the first key `\u{FEFF}KEY` -- so a header on line 1 went unseen and
	// `:env set` wrote plaintext into an encrypted file (audit SA-034).
	let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
	let mut result = Vec::new();
	let lines: Vec<&str> = content.lines().collect();
	let mut i = 0;

	while i < lines.len() {
		let line = lines[i];
		let trimmed = line.trim();

		// Skip blank lines and comments
		if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
			i += 1;
			continue;
		}

		// Find the '=' separator
		let eq_pos = match trimmed.find('=') {
			Some(pos) => pos,
			None => {
				// The line itself is never echoed: a continuation line of a
				// multi-line secret, or a plaintext key body `:env encrypt` left
				// behind, lands here and would otherwise be printed to the
				// terminal and the CI log (audit SA-026). The caller already
				// reports the line number.
				return Err((i + 1, "expected KEY=VALUE (no '=' on this line)".to_string()));
			}
		};

		let key = trimmed[..eq_pos].trim();
		if key.is_empty() {
			return Err((i + 1, "empty key".to_string()));
		}

		// Strip `export ` prefix if present (common in .env files)
		let key = key.strip_prefix("export ").unwrap_or(key).trim();
		if key.is_empty() {
			return Err((i + 1, "empty key after 'export'".to_string()));
		}

		let raw_value = trimmed[eq_pos + 1..].trim();

		// Check for quoted multi-line values. A value is multi-line ONLY when it
		// opens with a quote that has no matching close on the same line — i.e.
		// `find_closing_quote` returns `None`. A quoted value with trailing
		// content (e.g. `URL="https://x" # comment`) DOES have a closing quote,
		// so it stays on the single-line path and its trailing text/comment is
		// handled there. Keying off "ends with a quote" instead wrongly treated
		// any quoted-value-with-trailing-content as unterminated and swallowed
		// following lines into the value.
		if (raw_value.starts_with('"') && find_closing_quote(&raw_value[1..], '"').is_none())
			|| (raw_value.starts_with('\'') && find_closing_quote(&raw_value[1..], '\'').is_none())
		{
			let quote_char = raw_value.as_bytes()[0] as char;
			let mut value = raw_value[1..].to_string(); // skip opening quote
			i += 1;

			loop {
				if i >= lines.len() {
					return Err((i, format!("unterminated {quote_char}-quoted value for key \"{key}\"")));
				}
				let next_line = lines[i];
				if let Some(end_pos) = find_closing_quote(next_line, quote_char) {
					value.push('\n');
					value.push_str(&next_line[..end_pos]);
					i += 1;
					break;
				} else {
					value.push('\n');
					value.push_str(next_line);
					i += 1;
				}
			}

			if quote_char == '"' {
				value = unescape_double_quoted(&value);
			}
			result.push((key.to_string(), value));
		} else {
			// Single-line value
			let value = parse_single_line_value(raw_value);
			result.push((key.to_string(), value));
			i += 1;
		}
	}

	Ok(result)
}

/// Find the position of a closing quote in a line.
fn find_closing_quote(line: &str, quote: char) -> Option<usize> {
	let bytes = line.as_bytes();
	for j in 0..bytes.len() {
		if bytes[j] == quote as u8 {
			// Check it's not escaped
			let mut backslash_count = 0;
			for k in (0..j).rev() {
				if bytes[k] == b'\\' {
					backslash_count += 1;
				} else {
					break;
				}
			}
			if backslash_count % 2 == 0 {
				return Some(j);
			}
		}
	}
	None
}

/// Parse a single-line value, handling quotes and inline comments.
fn parse_single_line_value(raw: &str) -> String {
	if raw.is_empty() {
		return String::new();
	}

	// Double-quoted value
	if raw.starts_with('"')
		&& raw.len() >= 2
		&& let Some(end) = find_closing_quote(&raw[1..], '"')
	{
		let inner = &raw[1..1 + end];
		return unescape_double_quoted(inner);
	}

	// Single-quoted value (no escape processing)
	if raw.starts_with('\'')
		&& raw.len() >= 2
		&& let Some(end) = find_closing_quote(&raw[1..], '\'')
	{
		return raw[1..1 + end].to_string();
	}

	// Unquoted value: strip inline comments (# or //)
	let value = if let Some(pos) = raw.find(" #") {
		raw[..pos].trim()
	} else if let Some(pos) = raw.find(" //") {
		raw[..pos].trim()
	} else {
		raw
	};

	value.to_string()
}

/// Process escape sequences in double-quoted strings.
fn unescape_double_quoted(s: &str) -> String {
	let mut result = String::with_capacity(s.len());
	let mut chars = s.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some('n') => result.push('\n'),
				Some('t') => result.push('\t'),
				Some('r') => result.push('\r'),
				Some('"') => result.push('"'),
				Some('\\') => result.push('\\'),
				Some(other) => {
					result.push('\\');
					result.push(other);
				}
				None => result.push('\\'),
			}
		} else {
			result.push(c);
		}
	}
	result
}

/// Whether `value`, written bare after `KEY=`, would not read back as itself.
///
/// A newline would split it into further `KEY=VALUE` lines (so a value could
/// smuggle in an extra variable -- `NODE_OPTIONS`, `LD_PRELOAD` -- past review);
/// a leading quote reads as a quoted value; a leading or trailing blank is
/// trimmed off; a `"`, a backslash, or an inline-comment ` #` / ` //` is re-read
/// differently. Any of these means the value must be written double-quoted
/// (audit SA-030).
fn needs_quoting(value: &str) -> bool {
	if value.is_empty() {
		return false;
	}
	let b = value.as_bytes();
	let ends = [b[0], b[b.len() - 1]];
	ends.contains(&b'"')
		|| ends.contains(&b'\'')
		|| ends.contains(&b' ')
		|| ends.contains(&b'\t')
		|| value.contains(['\n', '\r', '\t', '"', '\\'])
		|| value.contains(" #")
		|| value.contains(" //")
}

/// Serialize a value for the right-hand side of a `KEY=VALUE` line, the exact
/// inverse of the parser above: a value that would not read back as itself is
/// written double-quoted with `\n`, `\r`, `\t`, `\"` and `\\` escapes (the ones
/// [`unescape_double_quoted`] decodes), and a simple value is written bare so an
/// ordinary file stays unquoted. One serializer for every writer -- `:env
/// decrypt`, `:env set`, and the `decrypt()` builtin -- so a multi-line secret
/// (a PEM key, a certificate) survives the round trip and no value can inject a
/// second variable (audit SA-030). `parse_env_file(serialize_env_line(k, v))`
/// yields `v` for any value; a round-trip test pins it.
pub fn serialize_env_value(value: &str) -> String {
	if !needs_quoting(value) {
		return value.to_string();
	}
	let mut out = String::with_capacity(value.len() + 2);
	out.push('"');
	for c in value.chars() {
		match c {
			'\\' => out.push_str("\\\\"),
			'"' => out.push_str("\\\""),
			'\n' => out.push_str("\\n"),
			'\r' => out.push_str("\\r"),
			'\t' => out.push_str("\\t"),
			_ => out.push(c),
		}
	}
	out.push('"');
	out
}

/// `KEY=VALUE` with the value serialized by [`serialize_env_value`].
pub fn serialize_env_line(key: &str, value: &str) -> String {
	format!("{key}={}", serialize_env_value(value))
}

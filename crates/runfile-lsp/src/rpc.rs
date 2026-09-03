//! LSP framing: `Content-Length: N\r\n\r\n<json>`.
//!
//! Hand-rolled rather than pulled from a framework, because the whole of the
//! transport is a header and a byte count, and the alternative would reintroduce
//! an async runtime for a server that answers one client, one message at a time.

use std::io::{BufRead, Write};

use serde_json::Value;

#[derive(Debug)]
pub enum ReadError {
	/// The client closed the stream. Normal shutdown, not a failure.
	Eof,
	Io(std::io::Error),
	Protocol(String),
}

/// Read one message. Headers other than `Content-Length` are skipped, as the
/// spec requires -- `Content-Type` is the common one.
pub fn read_message(r: &mut impl BufRead) -> Result<Value, ReadError> {
	let mut len: Option<usize> = None;
	loop {
		let mut line = String::new();
		match r.read_line(&mut line) {
			Ok(0) => return Err(ReadError::Eof),
			Ok(_) => {}
			Err(e) => return Err(ReadError::Io(e)),
		}
		let line = line.trim_end_matches(['\r', '\n']);
		if line.is_empty() {
			break;
		}
		if let Some(v) = line.strip_prefix("Content-Length:") {
			len = v
				.trim()
				.parse()
				.map_err(|_| ReadError::Protocol(format!("bad Content-Length: {v}")))
				.map(Some)?;
		}
	}
	let len = len.ok_or_else(|| ReadError::Protocol("missing Content-Length".into()))?;
	let mut buf = vec![0u8; len];
	r.read_exact(&mut buf).map_err(ReadError::Io)?;
	serde_json::from_slice(&buf).map_err(|e| ReadError::Protocol(e.to_string()))
}

/// Write one message, headers and all.
pub fn write_message(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
	let body = serde_json::to_vec(v)?;
	write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
	w.write_all(&body)?;
	w.flush()
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn framed(body: &str) -> Vec<u8> {
		format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
	}

	#[test]
	fn a_framed_message_round_trips() {
		let mut buf = Vec::new();
		write_message(&mut buf, &json!({"id": 1})).unwrap();
		let mut r = std::io::BufReader::new(&buf[..]);
		assert_eq!(read_message(&mut r).unwrap(), json!({"id": 1}));
	}

	#[test]
	fn extra_headers_are_skipped() {
		let body = r#"{"ok":true}"#;
		let raw = format!(
			"Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc\r\n\r\n{body}",
			body.len()
		);
		let mut r = std::io::BufReader::new(raw.as_bytes());
		assert_eq!(read_message(&mut r).unwrap(), json!({"ok": true}));
	}

	#[test]
	fn the_byte_count_is_bytes_not_characters() {
		// A multi-byte body read as characters would truncate the JSON.
		let body = r#"{"s":"héllo →"}"#;
		let raw = framed(body);
		let mut r = std::io::BufReader::new(&raw[..]);
		assert_eq!(read_message(&mut r).unwrap()["s"], "héllo →");
	}

	#[test]
	fn a_closed_stream_is_eof_not_an_error() {
		let mut r = std::io::BufReader::new(&b""[..]);
		assert!(matches!(read_message(&mut r), Err(ReadError::Eof)));
	}

	#[test]
	fn a_missing_length_is_a_protocol_error() {
		let mut r = std::io::BufReader::new(&b"X: 1\r\n\r\n{}"[..]);
		assert!(matches!(read_message(&mut r), Err(ReadError::Protocol(_))));
	}

	#[test]
	fn two_messages_read_back_in_order() {
		let mut buf = Vec::new();
		write_message(&mut buf, &json!({"n": 1})).unwrap();
		write_message(&mut buf, &json!({"n": 2})).unwrap();
		let mut r = std::io::BufReader::new(&buf[..]);
		assert_eq!(read_message(&mut r).unwrap()["n"], 1);
		assert_eq!(read_message(&mut r).unwrap()["n"], 2);
	}
}

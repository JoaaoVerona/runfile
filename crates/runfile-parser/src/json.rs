/// Deserialize a JSON5 string into a value.
///
/// JSON5 is a superset of JSON that supports comments, trailing commas,
/// unquoted keys, single-quoted strings, and more. All valid JSON is also
/// valid JSON5.
pub fn from_json_str<'a, T: serde::Deserialize<'a>>(s: &'a str) -> Result<T, json5::Error> {
	json5::from_str(s)
}

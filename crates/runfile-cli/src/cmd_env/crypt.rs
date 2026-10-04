use super::*;

/// Decrypt an encrypted env file. Writes to `output` if provided, otherwise prints to stdout.
/// When `source` is `None`, falls back to [`RUNFILE_ENV_FILE_TARGET_ENV_VAR`], so a
/// caller that has already pointed at a file need not repeat the path.
pub fn cmd_decrypt_file(source: Option<&str>, output: Option<&str>) {
	let source_owned = match source {
		Some(s) => s.to_string(),
		None => env_file_target().unwrap_or_else(|| {
			eprintln!(
				"Error: no source file specified.\n\
				 Usage: run :env decrypt <source> [output]\n\
				 (Or set {RUNFILE_ENV_FILE_TARGET_ENV_VAR} to provide one.)"
			);
			process::exit(1);
		}),
	};
	let source = source_owned.as_str();
	let (pairs, _) = read_env_file(source);
	let env_map: HashMap<String, String> = pairs.iter().cloned().collect();

	let public_key = match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		Some(pk) => pk.clone(),
		None => {
			eprintln!(
				"Error: {source} does not contain {} — not an encrypted file",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			);
			process::exit(1);
		}
	};

	let key_hex = resolve_private_key_by_public(&public_key);

	// Build output content: decrypt encrypted values, skip the public key line
	let content = read_file_content(source);
	let mut out_lines = Vec::new();

	for line in content.lines() {
		let trimmed = line.trim();
		// Skip the public key line
		if trimmed.starts_with(&format!("{}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
			|| trimmed.starts_with(&format!("export {}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
		{
			continue;
		}
		// If it's a key=value line with an encrypted value, decrypt it
		if !trimmed.is_empty()
			&& !trimmed.starts_with('#')
			&& !trimmed.starts_with("//")
			&& let Some(eq_pos) = trimmed.find('=')
		{
			let val_part = &trimmed[eq_pos + 1..];
			let val_trimmed = val_part.trim();
			// Strip quotes if present
			let val_unquoted = if (val_trimmed.starts_with('"') && val_trimmed.ends_with('"'))
				|| (val_trimmed.starts_with('\'') && val_trimmed.ends_with('\''))
			{
				&val_trimmed[1..val_trimmed.len() - 1]
			} else {
				val_trimmed
			};
			if runfile_crypto::is_encrypted(val_unquoted) {
				let key_part = &trimmed[..eq_pos];
				match runfile_crypto::decrypt(val_unquoted, &key_hex) {
					Ok(plaintext) => {
						out_lines.push(format!("{key_part}={plaintext}"));
						continue;
					}
					Err(e) => {
						eprintln!("Error decrypting {key_part}: {e}");
						process::exit(1);
					}
				}
			}
		}
		out_lines.push(line.to_string());
	}

	out_lines.retain(|line| !line.trim().is_empty());

	let mut out_content = out_lines.join("\n");
	if !out_content.ends_with('\n') {
		out_content.push('\n');
	}

	match output {
		Some(path) => {
			// Decrypted plaintext to disk — restrict to 0600 (Unix) so the
			// secrets aren't left world-readable on a shared host.
			if let Err(e) = write_secret_file(path, out_content.as_bytes()) {
				eprintln!("Error writing {path}: {e}");
				process::exit(1);
			}
			eprintln!("Decrypted {source} -> {path}");
		}
		None => {
			use std::io::Write;
			let stdout = std::io::stdout();
			let mut handle = stdout.lock();
			if let Err(e) = handle.write_all(out_content.as_bytes()) {
				eprintln!("Error writing to stdout: {e}");
				process::exit(1);
			}
		}
	}
}

/// Encrypt a plaintext env file into a new encrypted file.
pub fn cmd_encrypt_file(source: &str, output: &str, partial_key: &str) {
	// Check output isn't already encrypted
	let out_path = Path::new(output);
	if out_path.exists() {
		let out_content = read_file_content(output);
		if let Ok(pairs) = runfile_env::parse_env_file(&out_content) {
			let has_pub_key = pairs
				.iter()
				.any(|(k, _)| k == runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR);
			if has_pub_key {
				eprintln!(
					"Error: {output} is already encrypted (contains {})",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				);
				process::exit(1);
			}
		}
	}

	let all_keys = keyring_keys::all_private_keys();
	let key_hex = match runfile_crypto::find_private_key_by_public_prefix(partial_key, &all_keys) {
		Ok(k) => k,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	let public_key = runfile_crypto::derive_public_key(&key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	let content = read_file_content(source);
	let out_content = match encrypt_file_content(&content, &key_hex, &public_key) {
		Ok(c) => c,
		Err(e) => {
			eprintln!("Error: {e}");
			process::exit(1);
		}
	};

	if let Err(e) = std::fs::write(output, &out_content) {
		eprintln!("Error writing {output}: {e}");
		process::exit(1);
	}

	// A comment or blank line in the source is not carried into the encrypted
	// copy -- the parser does not record where they sit. The plaintext source is
	// left in place, so nothing is lost; say so rather than dropping them silently.
	if content
		.lines()
		.any(|l| matches!(l.trim(), t if t.starts_with('#') || t.starts_with("//")))
	{
		eprintln!("Note: comments in {source} were not carried into {output}.");
	}
	println!("Encrypted {source} -> {output}");
}

/// The encrypted form of a `.env` file: a public-key header, then each value the
/// real parser reads encrypted as one `KEY=encrypted:…` pair.
///
/// Encrypting the parsed value, not the raw text after the first `=`, is the
/// fix for audit SA-013: the line-by-line version encrypted only a multi-line
/// value's first line and copied the rest (a PEM key's body, a certificate)
/// into the "encrypted" output in plaintext, and folded a value's own quotes
/// and trailing `# comment` into the secret. Before returning, the output is
/// re-parsed and every value decrypted and checked against what was parsed, so
/// an encrypt that quietly changed or dropped a value is an error rather than a
/// file written as "encrypted" that is not.
fn encrypt_file_content(content: &str, key_hex: &str, public_key: &str) -> Result<String, String> {
	let pairs = runfile_env::parse_env_file(content).map_err(|(line, msg)| format!("line {line}: {msg}"))?;

	let mut out_lines = vec![format!("{}={public_key}", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR)];
	for (key, value) in &pairs {
		if key == runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR {
			continue; // our own header, re-added above
		}
		// An empty value, or one already encrypted, is written as it is.
		if value.is_empty() || runfile_crypto::is_encrypted(value) {
			out_lines.push(format!("{key}={value}"));
			continue;
		}
		let encrypted = runfile_crypto::encrypt(value, key_hex).map_err(|e| format!("encrypting `{key}`: {e}"))?;
		out_lines.push(format!("{key}={encrypted}"));
	}
	let out_content = out_lines.join("\n") + "\n";

	// Round-trip check: the output has to decrypt back to exactly what was parsed.
	let reparsed = runfile_env::parse_env_file(&out_content)
		.map_err(|(line, msg)| format!("the output would not parse back (line {line}: {msg})"))?;
	let got: HashMap<&str, &str> = reparsed.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
	for (key, value) in &pairs {
		if key == runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR {
			continue;
		}
		let written = got
			.get(key.as_str())
			.ok_or_else(|| format!("`{key}` is missing from the encrypted output"))?;
		let back = match runfile_crypto::is_encrypted(written) {
			true => runfile_crypto::decrypt(written, key_hex).map_err(|e| format!("`{key}` would not decrypt: {e}"))?,
			false => (*written).to_string(),
		};
		if &back != value {
			return Err(format!("`{key}` would not decrypt back to its source value"));
		}
	}
	Ok(out_content)
}

/// Rotate the private key for an encrypted env file: decrypt every value with
/// the old key and re-encrypt with a freshly generated key.
pub fn cmd_rotate(file: &str, delete_current_key: bool) {
	let content = read_file_content(file);
	let pairs = match runfile_env::parse_env_file(&content) {
		Ok(p) => p,
		Err((line, msg)) => {
			eprintln!("Error parsing {file} at line {line}: {msg}");
			process::exit(1);
		}
	};
	let env_map: HashMap<String, String> = pairs.iter().cloned().collect();

	// Verify this is an encrypted file
	let old_public_key = match env_map.get(runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR) {
		Some(pk) => pk.clone(),
		None => {
			eprintln!(
				"Error: {file} does not contain {} — not an encrypted file",
				runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
			);
			process::exit(1);
		}
	};

	// Resolve old private key
	let old_key_hex = resolve_private_key_by_public(&old_public_key);

	// Generate a new private key and store it
	let new_key_hex = runfile_crypto::generate_key();
	match keyring_keys::add(&new_key_hex) {
		Ok(true) => {}
		Ok(false) => {
			eprintln!("Error: generated key already exists. Try again.");
			process::exit(1);
		}
		Err(e) => {
			eprintln!("Error storing new key: {e}");
			process::exit(1);
		}
	}

	let new_public_key = runfile_crypto::derive_public_key(&new_key_hex).unwrap_or_else(|e| {
		eprintln!("Error deriving public key: {e}");
		process::exit(1);
	});

	// Re-encrypt the file: decrypt each value with old key, encrypt with new key
	let mut out_lines = Vec::new();

	for line in content.lines() {
		let trimmed = line.trim();

		// Replace the public key line
		if trimmed.starts_with(&format!("{}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
			|| trimmed.starts_with(&format!("export {}=", runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR))
		{
			let has_export = trimmed.starts_with("export ");
			if has_export {
				out_lines.push(format!(
					"export {}={new_public_key}",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				));
			} else {
				out_lines.push(format!(
					"{}={new_public_key}",
					runfile_crypto::ENCRYPTION_PUBLIC_KEY_VAR
				));
			}
			continue;
		}

		// Pass through comments and blank lines
		if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
			out_lines.push(line.to_string());
			continue;
		}

		// Re-encrypt key=value lines with encrypted values
		if let Some(eq_pos) = trimmed.find('=') {
			let key_part = &trimmed[..eq_pos];
			let val_part = &trimmed[eq_pos + 1..];
			let val_trimmed = val_part.trim();

			// Strip quotes if present
			let val_unquoted = if (val_trimmed.starts_with('"') && val_trimmed.ends_with('"'))
				|| (val_trimmed.starts_with('\'') && val_trimmed.ends_with('\''))
			{
				&val_trimmed[1..val_trimmed.len() - 1]
			} else {
				val_trimmed
			};

			if runfile_crypto::is_encrypted(val_unquoted) {
				let clean_key = key_part.strip_prefix("export ").unwrap_or(key_part).trim();
				let has_export = key_part.starts_with("export ");

				// Decrypt with old key
				let plaintext = match runfile_crypto::decrypt(val_unquoted, &old_key_hex) {
					Ok(p) => p,
					Err(e) => {
						eprintln!("Error decrypting {clean_key}: {e}");
						process::exit(1);
					}
				};

				// Encrypt with new key
				let encrypted = match runfile_crypto::encrypt(&plaintext, &new_key_hex) {
					Ok(enc) => enc,
					Err(e) => {
						eprintln!("Error encrypting {clean_key}: {e}");
						process::exit(1);
					}
				};

				if has_export {
					out_lines.push(format!("export {clean_key}={encrypted}"));
				} else {
					out_lines.push(format!("{clean_key}={encrypted}"));
				}
				continue;
			}
		}

		// Non-encrypted lines pass through unchanged
		out_lines.push(line.to_string());
	}

	let mut out_content = out_lines.join("\n");
	if !out_content.ends_with('\n') {
		out_content.push('\n');
	}

	if let Err(e) = std::fs::write(file, &out_content) {
		eprintln!("Error writing {file}: {e}");
		process::exit(1);
	}

	// Optionally delete the old key -- but never when the new one is only in
	// volatile keyutils, or a reboot would leave nothing that can decrypt the
	// file just rotated (audit SA-017).
	let persistent = runfile_state::keyring_store::is_persistent();
	let removed = if delete_current_key && !persistent {
		eprintln!(
			"[runfile] warning: keeping the old key: the new one is held in kernel keyutils (cleared on reboot), \
			 so deleting the old key could leave {file} unrecoverable. Save the new key and remove the old one \
			 by hand once it is safe: run :env secret-keys remove {}",
			&old_public_key[..old_public_key.len().min(8)]
		);
		false
	} else if delete_current_key {
		match keyring_keys::remove(&old_public_key) {
			Ok(true) => true,
			Ok(false) => {
				eprintln!("Warning: old key not found in credential store (already removed?).");
				false
			}
			Err(e) => {
				eprintln!("Warning: failed to remove old key: {e}");
				false
			}
		}
	} else {
		false
	};

	println!("Key rotated for {file}.");
	println!();
	println!("  Old public key: {old_public_key}");
	println!("  New public key: {new_public_key}");
	super::warn_if_volatile(&new_public_key);

	if removed {
		println!();
		println!("Old key has been removed from the OS credential store.");
	}

	println!();
	println!("To share the new key with teammates:");
	println!("  run :env secret-keys get-private {}...", &new_public_key[..8]);
}

// ══════════════════════════════════════════════════════════════════════
// Helpers
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
	use super::encrypt_file_content;

	/// A multi-line value (a PEM key), a quoted value with a trailing comment,
	/// and a plain value all encrypt as whole values and decrypt back unchanged,
	/// with no plaintext left in the output -- the audit SA-013 regression.
	#[test]
	fn encrypt_handles_multiline_quoted_and_commented_values() {
		let key = runfile_crypto::generate_key();
		let public = runfile_crypto::derive_public_key(&key).unwrap();
		let source = "SIGNING_KEY=\"-----BEGIN KEY-----\nLINE-ONE\nLINE-TWO==\n-----END KEY-----\"\n\
		              API_URL=\"https://api.example.test\" # prod\n\
		              TOKEN=tok_FAKE_abc123 # rotate\n";
		let out = encrypt_file_content(source, &key, &public).expect("encrypts");

		// No plaintext key material survives.
		assert!(
			!out.contains("LINE-ONE") && !out.contains("BEGIN KEY"),
			"plaintext leaked:\n{out}"
		);
		assert!(
			!out.contains("# prod") && !out.contains("# rotate"),
			"a comment became part of a value:\n{out}"
		);

		// Every value decrypts back to exactly what the parser read.
		let pairs = runfile_env::parse_env_file(&out).unwrap();
		let get = |k: &str| {
			let v = &pairs.iter().find(|(key, _)| key == k).unwrap().1;
			runfile_crypto::decrypt(v, &key).unwrap()
		};
		assert_eq!(
			get("SIGNING_KEY"),
			"-----BEGIN KEY-----\nLINE-ONE\nLINE-TWO==\n-----END KEY-----"
		);
		assert_eq!(get("API_URL"), "https://api.example.test");
		assert_eq!(get("TOKEN"), "tok_FAKE_abc123");
	}
}

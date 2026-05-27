use super::*;
use crate::args::StdinPrompter;
use std::sync::{Arc, Mutex};

/// Test prompter that returns scripted answers and records every prompt.
#[derive(Debug, Default)]
struct MockPrompter {
	value_answers: Mutex<HashMap<String, Option<String>>>,
	flag_answers: Mutex<HashMap<String, bool>>,
	value_calls: Mutex<Vec<(String, Option<String>)>>,
	flag_calls: Mutex<Vec<String>>,
}

impl MockPrompter {
	fn with_value(self, key: &str, answer: Option<&str>) -> Self {
		self.value_answers
			.lock()
			.unwrap()
			.insert(key.to_string(), answer.map(|s| s.to_string()));
		self
	}
	fn with_flag(self, key: &str, present: bool) -> Self {
		self.flag_answers.lock().unwrap().insert(key.to_string(), present);
		self
	}
}

impl StdinPrompter for MockPrompter {
	fn prompt_value(&self, key: &str, default: Option<&str>) -> Option<String> {
		self.value_calls
			.lock()
			.unwrap()
			.push((key.to_string(), default.map(|s| s.to_string())));
		self.value_answers.lock().unwrap().get(key).cloned().unwrap_or(None)
	}
	fn prompt_flag(&self, key: &str) -> bool {
		self.flag_calls.lock().unwrap().push(key.to_string());
		self.flag_answers.lock().unwrap().get(key).copied().unwrap_or(false)
	}
}

fn args_with(prompter: Arc<dyn StdinPrompter>) -> RunArgs {
	RunArgs::parse(&[]).with_stdin_prompter(Some(prompter))
}

#[test]
fn missing_args_prompts_and_uses_answer() {
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.name", Some("alice")));
	let args = args_with(prompter.clone());
	let result = args.substitute("hello {{ ARG.name }}", &HashMap::new()).unwrap();
	assert_eq!(result, "hello alice");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], ("ARG.name".to_string(), None));
}

#[test]
fn missing_args_with_default_prompts_and_falls_through_when_empty() {
	// Empty answer (None) should fall through to the literal default.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.env", None));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("env={{ ARG.env ? 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "env=production");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls[0], ("ARG.env".to_string(), Some("production".to_string())));
}

#[test]
fn missing_args_with_default_prompts_and_overrides_when_provided() {
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.env", Some("staging")));
	let args = args_with(prompter);
	let result = args
		.substitute("env={{ ARG.env ? 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "env=staging");
}

#[test]
fn missing_args_no_default_no_answer_errors() {
	// Required substitution; user pressed Enter; nothing else in the chain
	// → fall through to MissingArg as if --stdin-args wasn't set.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.name", None));
	let args = args_with(prompter);
	let err = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MissingArg(ref k) if k == "name"));
}

#[test]
fn provided_args_skip_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["--name=bob".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap();
	assert_eq!(result, "hi bob");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn missing_positional_args_prompts_for_bare_args() {
	// Bare `{{ ARGS }}` with no positional args should prompt under
	// --stdin-args (the bump-target use case: `"match": "{{ ARGS }}"`).
	let prompter = Arc::new(MockPrompter::default().with_value("ARGS", Some("major")));
	let args = args_with(prompter.clone());
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=major");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], ("ARGS".to_string(), None));
}

#[test]
fn missing_positional_args_empty_answer_falls_back_to_empty() {
	// Empty answer (user pressed Enter): `{{ ARGS }}` resolves to "",
	// matching prior behavior.
	let prompter = Arc::new(MockPrompter::default().with_value("ARGS", None));
	let args = args_with(prompter);
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=");
}

#[test]
fn provided_positional_args_skip_bare_args_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["minor".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=minor");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn missing_env_prompts_and_uses_answer() {
	let prompter = Arc::new(MockPrompter::default().with_value("ENV.SECRET", Some("hush")));
	let args = args_with(prompter);
	let result = args.substitute("token={{ ENV.SECRET }}", &HashMap::new()).unwrap();
	assert_eq!(result, "token=hush");
}

#[test]
fn provided_env_skips_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = args_with(prompter.clone());
	let mut env = HashMap::new();
	env.insert("HOST".to_string(), "example.com".to_string());
	let result = args.substitute("host={{ ENV.HOST }}", &env).unwrap();
	assert_eq!(result, "host=example.com");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn chain_args_to_env_to_default_prompts_once_with_first_source_key() {
	// `{{ ARG.x ? ENV.X ? 'fallback' }}` — neither set, prompt key is
	// the first source (ARG.x), default is "fallback".
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.x", Some("entered")));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("v={{ ARG.x ? ENV.X ? 'fallback' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "v=entered");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], ("ARG.x".to_string(), Some("fallback".to_string())));
}

#[test]
fn flags_missing_prompts_for_presence() {
	let prompter = Arc::new(MockPrompter::default().with_flag("--verbose", true));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("cmd {{ FLAG.verbose ? '-v' : }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "cmd -v");
	let calls = prompter.flag_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], "--verbose");
}

#[test]
fn flags_provided_skips_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["--verbose".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args
		.substitute("cmd {{ FLAG.verbose ? '-v' : }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "cmd -v");
	assert!(prompter.flag_calls.lock().unwrap().is_empty());
}

#[test]
fn flags_user_declines_returns_false_branch() {
	let prompter = Arc::new(MockPrompter::default().with_flag("--release", false));
	let args = args_with(prompter);
	let result = args
		.substitute(
			"cargo build {{ FLAG.release ? '--release' : '--debug' }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "cargo build --debug");
}

#[test]
fn no_prompter_preserves_existing_error() {
	// Sanity check: with no prompter, missing args still error.
	let args = RunArgs::parse(&[]);
	let err = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MissingArg(_)));
}

//! Each rule, on what it must find and on the text nearest to it that it must
//! leave alone -- which is where a false report would come from.

use crate::script::{self, Script, Source};
use crate::{Finding, check};
use runfile_lang::{Block, Expr, Statement, Target};

fn parsed(src: &str) -> Target {
	runfile_lang::parse(src).unwrap_or_else(|e| panic!("{src}\n{e}"))
}

fn findings(src: &str) -> Vec<Finding> {
	check(src, &parsed(src), Some(&[]))
}

/// The one finding in `src`, which has to be `rule`'s and cover `covers`.
fn one(src: &str, rule: &str, covers: &str) -> Finding {
	let f = findings(src);
	assert_eq!(f.len(), 1, "{src}\n{f:#?}");
	assert_eq!(f[0].rule, rule, "{src}\n{f:#?}");
	assert_eq!(&src[f[0].span.start..f[0].span.end], covers, "{src}\n{f:#?}");
	f[0].clone()
}

fn clean(src: &str) {
	let f = findings(src);
	assert!(f.is_empty(), "{src}\n{f:#?}");
}

fn fix(f: &Finding) -> &str {
	f.fix.as_deref().unwrap_or_default()
}

// ---- quoted-interpolation

#[test]
fn an_interpolation_in_single_quotes_is_found_wherever_it_is() {
	let f = one(
		"let port = \"8080\"\n$ sudo ss -ltnp 'sport = :{{ port }}'\n",
		"quoted-interpolation",
		"'sport = :{{ port }}'",
	);
	assert_eq!(fix(&f), "write `'sport = :'{{ port }}`");
	one("$ echo '{{ RUN.os }}'\n", "quoted-interpolation", "'{{ RUN.os }}'");
}

#[test]
fn an_interpolation_in_double_quotes_is_found_where_no_shell_reads_the_word_again() {
	let program = one(
		"$ nohup \"{{ ENV.SDK }}/emulator/{{ RUN.os }}\" -avd x\n",
		"quoted-interpolation",
		"\"{{ ENV.SDK }}/emulator/{{ RUN.os }}\"",
	);
	assert_eq!(fix(&program), "write `{{ ENV.SDK }}/emulator/{{ RUN.os }}`");
	let redirect = one(
		"$ curl -o - x > \"{{ ARG.out }}.part\"\n",
		"quoted-interpolation",
		"\"{{ ARG.out }}.part\"",
	);
	assert_eq!(fix(&redirect), "write `{{ ARG.out }}.part`");
	let volume = one(
		"$ docker run -v \"$(pwd)/.restore/{{ ARG.src }}:/src:ro\" alpine true\n",
		"quoted-interpolation",
		"\"$(pwd)/.restore/{{ ARG.src }}:/src:ro\"",
	);
	assert_eq!(fix(&volume), "write `\"$(pwd)/.restore/\"{{ ARG.src }}:/src:ro`");
	let pattern = one(
		"$ find . -lname \"{{ RUN.parent }}/*\" -delete\n",
		"quoted-interpolation",
		"\"{{ RUN.parent }}/*\"",
	);
	assert_eq!(fix(&pattern), "write `{{ RUN.parent }}\"/*\"`");
	one(
		"$ test -L \"{{ ARG.ext }}\"\n",
		"quoted-interpolation",
		"\"{{ ARG.ext }}\"",
	);
	one(
		"$ mv \"{{ ARG.out }}.part\" x\n",
		"quoted-interpolation",
		"\"{{ ARG.out }}.part\"",
	);
}

#[test]
fn double_quotes_around_code_for_another_shell_are_left_alone() {
	// The interpolation's own quoting is exactly right for the shell that reads
	// the string again, so these are not mistakes.
	clean("$ ssh host \"cd {{ ARG.dir }} && ls\"\n");
	clean("$ sh -c \"rm -rf {{ ARG.dir }}\"\n");
	clean("$ docker run alpine sh -c \"ls {{ ARG.dir }}\"\n");
	clean("$ echo \"export PATH={{ ARG.bin }}:\\$PATH\" >> rc\n");
	clean("$ echo \"Needs the {{ ARG.target }} target\"\n");
	clean("$ find . -exec sh -c \"echo {{ ARG.x }}\" \\;\n");
	// Two quoted interpolations compared with each other agree whatever they hold.
	clean("$ [ \"{{ ARG.a }}\" = \"{{ ARG.b }}\" ]\n");
}

#[test]
fn an_interpolation_outside_quotes_or_in_a_heredoc_is_fine() {
	clean("$ cp {{ ARG.a }} {{ ARG.b }}/x\n");
	clean("$ cat <<EOF\n$ name = '{{ ARG.name }}'\n$ EOF\n");
	clean("$ echo \"$(basename {{ ARG.path }})\"\n");
}

// ---- where an interpolation sits, which the runner writes its value for

/// Where each interpolation in `src`'s first `$` run or `exec` sits.
fn spots_of(src: &str) -> Option<Vec<Option<runfile_lang::Spot>>> {
	let t = parsed(src);
	let Some(Statement::Exec { command, body, .. }) = t.body.statements.first() else {
		panic!("no shell in {src}");
	};
	let program: Option<String> = command.as_ref().map(|c| {
		c.iter()
			.map(|p| match p {
				runfile_lang::InterpPart::Literal(s) => s.as_str(),
				runfile_lang::InterpPart::Expr(_) => "",
			})
			.collect()
	});
	crate::spots(program.as_deref(), body)
}

fn quoting(src: &str) -> Vec<runfile_lang::Quoting> {
	spots_of(src)
		.unwrap_or_else(|| panic!("not read: {src}"))
		.into_iter()
		.map(|s| {
			s.unwrap_or_else(|| panic!("an interpolation was passed without being placed: {src}"))
				.quoting
		})
		.collect()
}

#[test]
fn an_interpolation_is_placed_inside_the_quotes_that_hold_it() {
	use runfile_lang::Quoting::*;
	assert_eq!(quoting("$ echo {{ v }}\n"), [Bare]);
	assert_eq!(quoting("$ echo \"a {{ v }} b\"\n"), [Double]);
	assert_eq!(quoting("$ echo 'a{{ v }}'\n"), [Single]);
	assert_eq!(quoting("$ echo $'a{{ v }}'\n"), [Ansi]);
	assert_eq!(quoting("$ X={{ v }} cmd\n"), [Bare]);
	assert_eq!(quoting("$ ssh host \"cd {{ v }} && ./restart\"\n"), [Double]);
	// Quoting starts over inside a command substitution, in double quotes or not.
	assert_eq!(quoting("$ echo \"$(basename {{ v }})\"\n"), [Bare]);
	assert_eq!(quoting("$ echo \"$(echo \"{{ v }}\")\"\n"), [Double]);
	// A `case` inside one is not ended at its first `)`.
	assert_eq!(quoting("$ echo \"$(case a in a) echo {{ v }};; esac)\"\n"), [Bare]);
	// A `${…}` is quoted the way it is, and a `'…'` inside it is a quote only
	// outside double quotes.
	assert_eq!(quoting("$ echo ${x:-{{ v }}}\n"), [Bare]);
	assert_eq!(quoting("$ echo \"${x:-{{ v }}}\"\n"), [Double]);
	assert_eq!(quoting("$ echo ${x:-'{{ v }}'}\n"), [Single]);
	assert_eq!(quoting("$ echo \"${x:-'{{ v }}'}\"\n"), [SingleExpanded]);
	// Inside a heredoc a `${…}` is written for the way bash reads it too.
	assert_eq!(quoting("$ cat <<EOF\n$ ${x:-{{ v }}}\n$ EOF\n"), [Double]);
	// Arithmetic expands what is in it the way double quotes do.
	assert_eq!(quoting("$ echo $(( {{ v }} + 1 ))\n"), [Double]);
	assert_eq!(quoting("$ (( {{ v }} > 1 )) && echo big\n"), [Double]);
	assert_eq!(quoting("$ echo hi # {{ v }}\n"), [Comment]);
	// Every one of them, in order, across a run of lines.
	assert_eq!(
		quoting("$ echo {{ a }}\n$ echo \"{{ b }}\" '{{ c }}'\n"),
		[Bare, Double, Single]
	);
}

#[test]
fn an_interpolation_in_a_heredoc_is_placed_with_the_line_that_ends_it() {
	use runfile_lang::{Delimiter, Quoting, Spot};
	let ends = |text: &str, strip_tabs| {
		vec![Delimiter {
			text: text.into(),
			strip_tabs,
		}]
	};
	let spot = |quoting, heredocs| Some(vec![Some(Spot { quoting, heredocs })]);
	assert_eq!(
		spots_of("$ cat <<EOF\n$ name={{ v }}\n$ EOF\n"),
		spot(Quoting::Heredoc, ends("EOF", false))
	);
	// A quoted delimiter: nothing in the body expands.
	assert_eq!(
		spots_of("$ cat <<'EOF'\n$ name={{ v }}\n$ EOF\n"),
		spot(Quoting::Literal, ends("EOF", false))
	);
	// Quoting starts over in a substitution inside the body, and the body still
	// ends on its own line.
	assert_eq!(
		spots_of("exec bash\n\tcat <<-END\n\t$(basename {{ v }})\n\tEND\nend\n"),
		spot(Quoting::Bare, ends("END", true))
	);
}

#[test]
fn an_interpolation_in_backticks_is_placed_inside_them() {
	use runfile_lang::Quoting::*;
	assert_eq!(
		quoting("$ echo `basename {{ v }}`\n"),
		[Backtick(Box::new(Bare), false)]
	);
	assert_eq!(
		quoting("$ echo \"`basename {{ v }}`\"\n"),
		[Backtick(Box::new(Bare), true)]
	);
	assert_eq!(
		quoting("$ echo `echo \"{{ v }}\"`\n"),
		[Backtick(Box::new(Double), false)]
	);
}

#[test]
fn a_body_this_reading_does_not_follow_places_nothing() {
	// The runner then writes every value the one way it always has.
	assert_eq!(spots_of("exec zsh\n\techo \"{{ v }}\"\nend\n"), None);
	assert_eq!(spots_of("exec python3\n\tprint({{ v }})\nend\n"), None);
	assert_eq!(spots_of("$ coproc cat {{ v }}\n"), None);
}

// ---- tilde-in-quotes

#[test]
fn a_quoted_tilde_in_a_path_is_found() {
	let f = one(
		"$ docker run -v \"~/.config/stripe:/root/.config/stripe\" -it stripe\n",
		"tilde-in-quotes",
		"\"~/.config/stripe:/root/.config/stripe\"",
	);
	assert_eq!(fix(&f), "write `\"$HOME/.config/stripe:/root/.config/stripe\"`");
	let single = one("$ cd '~/src'\n$ make\n", "tilde-in-quotes", "'~/src'");
	assert_eq!(fix(&single), "write `~/'src'`");
}

#[test]
fn a_quoted_tilde_that_is_text_is_left_alone() {
	clean("$ echo \"~/x is where it goes\"\n");
	clean("$ grep \"~/\" notes.txt\n");
	clean("$ sed 's|~/|/home/|' f\n");
}

// ---- unexpanded-string

#[test]
fn a_string_spelling_home_that_reaches_a_path_is_found_where_it_is_written() {
	let src = "let sdk = \"\"\nif RUN.os == \"mac\"\n\tsdk = \"$HOME/Library/Android/sdk\"\nend\nlet dir = ENV.ANDROID_HOME ? sdk\n$ nohup {{ dir }}/emulator/emulator -avd x\n";
	let f = one(src, "unexpanded-string", "\"$HOME/Library/Android/sdk\"");
	assert_eq!(fix(&f), "write `{{ ENV.HOME }}` in its place");
	assert!(f.message.contains("line 6"), "{}", f.message);
	one(
		"let avd = ENV.AVD ? \"$HOME/.android/avd\"\n$ mkdir -p {{ avd }}\n",
		"unexpanded-string",
		"\"$HOME/.android/avd\"",
	);
	one(
		"write_file(\"~/notes.txt\", \"hi\")\n",
		"unexpanded-string",
		"\"~/notes.txt\"",
	);
}

#[test]
fn a_string_spelling_home_is_left_alone_where_it_is_not_a_path() {
	clean("let line = \"export PATH=$HOME/bin:$PATH\"\n$ echo {{ line }} >> rc\n");
	clean("print(\"$HOME\")\n");
	clean("let p = \"{{ ENV.HOME }}/x\"\n$ ls {{ p }}\n");
	// Rebound before it is used, so the first value never reaches the path.
	clean("let p = \"$HOME/x\"\np = \"{{ ENV.HOME }}/x\"\n$ ls {{ p }}\n");
	clean("let bin = \"$RECYCLE.BIN\"\n$ ls {{ bin }}\n");
}

// ---- unexpanded-glob

#[test]
fn a_pattern_interpolated_as_a_file_is_found() {
	let src = "let manifests = \"*@*/metadata.json\"\nfor m in lines($ ls -d {{ manifests }})\n\tprint(m)\nend\n";
	one(src, "unexpanded-glob", "{{ manifests }}");
}

#[test]
fn a_pattern_from_a_shared_file_is_found_at_the_interpolation() {
	let shared = parsed("let manifests = \"*@*/metadata.json\"\n");
	let src = "$ rm {{ manifests }}\n";
	let f = check(src, &parsed(src), Some(std::slice::from_ref(&shared)));
	assert_eq!(f.len(), 1, "{f:#?}");
	assert_eq!(f[0].rule, "unexpanded-glob");
	assert_eq!(&src[f[0].span.start..f[0].span.end], "{{ manifests }}");
}

#[test]
fn a_pattern_handed_to_a_command_that_takes_patterns_is_left_alone() {
	clean("let p = \"*.js\"\n$ find . -name {{ p }}\n");
	clean("let p = \"*.js\"\n$ git ls-files {{ p }}\n");
	clean("let dir = \"src\"\n$ ls {{ dir }}/*\n");
}

// ---- positional-parameter

#[test]
fn a_positional_parameter_in_a_shell_given_none_is_found() {
	let f = one("$ echo \"$1\"\n", "positional-parameter", "$1");
	assert_eq!(fix(&f), "the target's own are `{{ ARGS[0] }}`");
	one("$ cargo test \"$@\"\n", "positional-parameter", "$@");
	one("$ [ $# -eq 0 ]\n", "positional-parameter", "$#");
	one("$ echo ${2:-x}\n", "positional-parameter", "${2:-x}");
}

#[test]
fn a_positional_parameter_the_shell_has_is_left_alone() {
	clean("$ f() { echo \"$1\"; }\n$ f x\n");
	clean("$ set -- a b\n$ echo \"$1\"\n");
	clean("$ set -euo pipefail\n$ echo ok\n");
	clean("$ xargs sh -c 'echo \"$1\"' _\n");
	clean("exec bash -s one two\n\techo \"$1\"\nend\n");
	clean("$ echo \"$0\"\n");
}

#[test]
fn set_with_options_alone_keeps_the_rule() {
	one("$ set -euo pipefail\n$ echo $1\n", "positional-parameter", "$1");
}

// ---- lost-effect

#[test]
fn cd_or_export_as_the_last_command_of_its_shell_is_found() {
	one("$ cd web\nprint(\"x\")\n", "lost-effect", "cd web");
	one(
		"$ export NODE_ENV=production\n",
		"lost-effect",
		"export NODE_ENV=production",
	);
	one("$ set -euo pipefail\n", "lost-effect", "set -euo pipefail");
}

#[test]
fn cd_that_something_uses_is_left_alone() {
	clean("$ cd web\n\n# install\n$ npm install\n");
	clean("let here = $ cd web && pwd\n");
	clean("if $ cd web\n\tprint(\"there\")\nend\n");
	clean("$ export STAMP=$(date)\n");
	clean("$ cd -\n");
}

// ---- dropped-backslash

#[test]
fn a_backslash_bash_drops_is_found() {
	let f = one("$ rm -rf frontend\\target\n", "dropped-backslash", "\\t");
	assert!(f.message.contains("`frontendtarget`"), "{}", f.message);
	assert_eq!(fix(&f), "write `'frontend\\target'`");
	one("$ tr -d \\r\n", "dropped-backslash", "\\r");
}

#[test]
fn a_backslash_that_does_a_job_is_left_alone() {
	clean("$ find . -name \\*.js -exec echo {} \\;\n");
	clean("$ \\rm -f x\n");
	clean("$ cat <<\\EOF\n$ $x\n$ EOF\n");
	clean("$ echo \"a\\tb\" 'c\\d'\n");
	clean("$ echo a \\\n$   b\n");
}

// ---- windows-command

#[test]
fn cmd_syntax_in_a_dollar_line_is_found() {
	let f = findings("$ rmdir /S /Q frontend\\target\n");
	let rules: Vec<&str> = f.iter().map(|f| f.rule).collect();
	assert_eq!(rules, ["windows-command", "dropped-backslash"], "{f:#?}");
	assert_eq!(fix(&f[0]), "write `rm -rf frontend/target`");
	clean("$ ls /S\n");
}

// ---- bracket-spacing, test-redirect

#[test]
fn a_bracket_against_a_word_is_found() {
	let f = one("$ if [ -f x]; then echo; fi\n", "bracket-spacing", "x]");
	assert_eq!(fix(&f), "write `x ]`");
	one("$ [-f x ] && echo\n", "bracket-spacing", "[-f");
	clean("$ [ -f x ] && echo\n");
	clean("$ [ \"$a\" = \"]\" ] && echo\n");
}

#[test]
fn a_redirection_inside_a_test_is_found() {
	one("$ [ \"$a\" > \"$b\" ] && echo\n", "test-redirect", "> \"$b\"");
	clean("$ [ -f x ] 2>/dev/null\n");
}

// ---- outside-function, spaced-assignment, sudo-builtin

#[test]
fn local_and_return_outside_a_function_are_found() {
	one("$ local x=1\n", "outside-function", "local");
	one("$ return 0\n", "outside-function", "return");
	clean("$ f() { local x=1; return 0; }\n$ f\n");
}

#[test]
fn an_assignment_with_blanks_is_found() {
	let f = one("$ FOO = bar\n", "spaced-assignment", "FOO =");
	assert_eq!(fix(&f), "write `FOO=bar`");
	clean("$ FOO= bar\n");
	clean("$ [ a = b ]\n");
}

#[test]
fn sudo_of_a_builtin_is_found() {
	one("$ sudo cd /root\n", "sudo-builtin", "sudo cd");
	clean("$ sudo ls /root\n");
}

#[test]
fn a_declaration_with_blanks_around_its_equals_is_found() {
	let f = one(
		"$ export NODE_ENV = production\n$ npm run build\n",
		"spaced-assignment",
		"NODE_ENV =",
	);
	assert_eq!(fix(&f), "write `NODE_ENV=production`");
	let f = one("$ readonly TAG =v1\n$ echo \"$TAG\"\n", "spaced-assignment", "TAG =v1");
	assert_eq!(fix(&f), "write `TAG=v1`");
	one("$ f() { local x = 1; echo \"$x\"; }\n$ f\n", "spaced-assignment", "x =");
	clean("$ export NODE_ENV=production\n$ npm run build\n");
	clean("$ declare -A map\n$ map[a]=1\n$ echo \"${map[a]}\"\n");
	clean("$ export $NAME=1\n$ echo ok\n");
}

// ---- outside-loop, dollar-assignment

#[test]
fn break_or_continue_with_no_shell_loop_around_it_is_found() {
	let f = one("for x in [1]\n\t$ [ -f x ] || break\nend\n", "outside-loop", "break");
	assert!(fix(&f).contains("without `$`"), "{}", fix(&f));
	one("$ continue 2\n", "outside-loop", "continue");
}

#[test]
fn break_inside_a_shell_loop_or_a_function_is_left_alone() {
	clean("$ for f in *; do [ -f \"$f\" ] || continue; echo \"$f\"; done\n");
	clean("$ while true; do break; done\n$ until false; do break; done\n");
	clean("$ select x in a b; do break; done\n$ for ((i = 0; i < 3; i++)); do break; done\n");
	clean("$ f() { break; }\n$ f\n");
	clean("$ for f in *; do\n$ \tcase $f in *.md) continue ;; esac\n$ done\n");
	clean("$ for f in *; do x=$(echo \"$f\"; break); done\n");
}

#[test]
fn a_dollar_in_front_of_the_name_being_set_is_found() {
	let f = one("$ $VERSION=1.2\n$ echo ok\n", "dollar-assignment", "$VERSION=1.2");
	assert_eq!(fix(&f), "write `VERSION=1.2`");
	one("$ ${VERSION}=1.2\n$ echo ok\n", "dollar-assignment", "${VERSION}=1.2");
	let f = one("$ for $f in *.log; do gzip \"$f\"; done\n", "dollar-assignment", "$f");
	assert_eq!(fix(&f), "write `f`");
	clean("$ echo $VERSION=1.2\n");
	clean("$ for f in *.log; do gzip \"$f\"; done\n");
}

// ---- unterminated-exec, truncated-input

#[test]
fn a_find_exec_nothing_ends_is_found() {
	let f = one(
		"$ find . -name \"*.orig\" -exec rm {} ;\n",
		"unterminated-exec",
		"-exec",
	);
	assert!(fix(&f).contains("\\;"), "{}", fix(&f));
	one("$ find . -ok rm {} +\n", "unterminated-exec", "-ok");
	one(
		"$ find . -exec rm {} \\; -execdir echo {}\n",
		"unterminated-exec",
		"-execdir",
	);
	one("$ find . -exec echo {}+ ';'x\n", "unterminated-exec", "-exec");
}

#[test]
fn a_find_exec_that_ends_or_may_end_is_left_alone() {
	clean("$ find . -name \"*.orig\" -exec rm {} \\;\n$ find . -exec rm {} +\n");
	clean("$ find . -exec sh -c 'echo \"$0\"' {} ';'\n");
	clean("$ find . -name -exec -print\n");
	clean("$ find . -exec rm {} {{ ARG.end }}\n");
	clean("$ find . -exec echo + \\;\n");
	clean("$ find . -newer $ref -exec rm {} \\;\n");
}

#[test]
fn a_file_emptied_by_the_command_that_reads_it_is_found() {
	let f = one(
		"$ jq '.version = \"2\"' package.json > package.json\n",
		"truncated-input",
		"> package.json",
	);
	assert!(fix(&f).contains("package.json.tmp"), "{}", fix(&f));
	one("$ sort -u names.txt > names.txt\n", "truncated-input", "> names.txt");
	let f = one(
		"$ sed -e 's/a/b/' notes.txt > notes.txt\n",
		"truncated-input",
		"> notes.txt",
	);
	assert_eq!(fix(&f), "`sed -i` edits the file in place");
	one(
		"$ tr -d '\\r' < notes.txt > notes.txt\n",
		"truncated-input",
		"> notes.txt",
	);
	one("$ grep -v '^#' .env 2> .env\n", "truncated-input", "2> .env");
	one(
		"$ awk -F: '{ print $1 }' users.txt > users.txt\n",
		"truncated-input",
		"> users.txt",
	);
	one(
		"$ jq --arg v 2 '.version = $v' package.json > package.json\n",
		"truncated-input",
		"> package.json",
	);
	one("$ head -n5 -q log.txt >| log.txt\n", "truncated-input", ">| log.txt");
}

#[test]
fn a_file_that_may_not_be_the_one_read_is_left_alone() {
	clean("$ jq '.version = \"2\"' package.json > package.json.tmp\n$ mv package.json.tmp package.json\n");
	// An option's value, or the program, is not a file.
	clean("$ head -n 5 > 5\n");
	clean("$ grep -e x.y > x.y\n");
	clean("$ jq --arg f a.json '$f' > a.json\n");
	clean("$ awk '{ print }' count=1 > count=1\n");
	// Nothing is read, or not from there.
	clean("$ jq -n '{a: 1}' > a.json\n");
	clean("$ sort names.txt >> names.txt\n");
	clean("$ cat - > -\n");
	clean("$ sort names.txt < names.txt > names.txt.sorted\n");
	// What this reading cannot be sure of.
	clean("$ sort {{ ARG.file }} > {{ ARG.file }}\n");
	clean("$ sort --files0-from=list names.txt > names.txt\n");
	clean("$ grep -r x . > x\n");
	clean("$ cat names.txt | sort > names.txt\n");
}

// ---- arithmetic-interpolation

#[test]
fn an_interpolation_in_a_shell_arithmetic_position_must_be_a_number() {
	// `[[ ]]` numeric comparisons, `let`, and `declare -i` evaluate their
	// operands as arithmetic, so a non-number there can carry a `$( )`.
	one("$ [[ 1 -eq {{ ARG.v }} ]]\n", "arithmetic-interpolation", "{{ ARG.v }}");
	one(
		"$ [[ {{ ARG.v }} -gt 10 ]]\n",
		"arithmetic-interpolation",
		"{{ ARG.v }}",
	);
	one("$ let x={{ ARG.v }}\n", "arithmetic-interpolation", "{{ ARG.v }}");
	one(
		"$ declare -i n={{ ARG.v }}\n",
		"arithmetic-interpolation",
		"{{ ARG.v }}",
	);
	let f = one("$ [[ 1 -eq {{ ARG.v }} ]]\n", "arithmetic-interpolation", "{{ ARG.v }}");
	assert_eq!(fix(&f), "make it a number, as `{{ number(ARG.v) }}`");
}

#[test]
fn a_number_in_an_arithmetic_position_is_left_alone() {
	// The type decides it: `number(…)`, a literal, or a name bound to one.
	clean("$ [[ 1 -eq {{ number(ARG.v) }} ]]\n");
	clean("let n = number(ARG.v)\n$ [[ {{ n }} -gt 10 ]]\n");
	clean("$ let x={{ 5 }}\n");
	// A string comparison is not arithmetic, so a string operand is fine.
	clean("$ [[ {{ ARG.v }} == main ]]\n");
	// `[` / `test` is a builtin that does not re-evaluate its operands, so an
	// interpolation there is inert -- unlike `[[ … ]]`.
	clean("$ [ {{ ARG.v }} -gt 3 ]\n");
	// `docker run -v` and `command -v` are not bash's `[[ -v ]]`.
	clean("$ docker run --rm -v {{ ARG.v }}:/dst alpine true\n");
	clean("$ command -v {{ ARG.v }}\n");
}

// ---- the tests of `[`

#[test]
fn a_test_with_no_closing_bracket_is_found() {
	let f = one(
		"$ if [ -f .env; then cp .env .env.bak; fi\n",
		"missing-bracket",
		"[ -f .env",
	);
	assert_eq!(fix(&f), "write `.env ]`");
	one(
		"$ [ -d dist ] && [ -d build || echo none\n",
		"missing-bracket",
		"[ -d build",
	);
	one("$ [\n", "missing-bracket", "[");
}

#[test]
fn a_test_that_ends_or_may_end_is_left_alone() {
	clean("$ [ -f .env ] && cp .env .env.bak\n");
	clean("$ [ -f .env \"]\" && echo yes\n");
	// What expands could be the `]`.
	clean("$ [ -f {{ ARG.file }}\n");
	clean("$ [ -f \"$file\" && echo\n");
}

#[test]
fn a_comparison_written_as_one_word_is_found() {
	let f = one(
		"$ [ \"$a\"=\"$b\" ] && echo same\n",
		"glued-comparison",
		"\"$a\"=\"$b\"",
	);
	assert_eq!(fix(&f), "write `\"$a\" = \"$b\"`");
	let f = one("$ [[ $a!=$b ]] && echo differ\n", "glued-comparison", "$a!=$b");
	assert_eq!(fix(&f), "write `$a != $b`");
	let f = one(
		"$ test {{ ARG.a }}=={{ ARG.b }} && echo same\n",
		"glued-comparison",
		"{{ ARG.a }}=={{ ARG.b }}",
	);
	assert_eq!(fix(&f), "write `{{ ARG.a }} == {{ ARG.b }}`");
	let f = one("$ [ \"$a=$b\" ] && echo same\n", "glued-comparison", "\"$a=$b\"");
	assert_eq!(f.fix, None, "blanks inside the quotes would not make three words");
}

#[test]
fn a_comparison_with_blanks_or_an_equals_that_is_not_one_is_left_alone() {
	clean("$ [ \"$a\" = \"$b\" ] && echo same\n");
	clean("$ [[ $x == *=* ]] && echo has\n");
	clean("$ [[ ${x#*=} ]] && echo set\n");
	clean("$ [ = ] && echo odd\n");
	clean("$ [ -n \"$a=$b\" ] && echo odd\n");
}

#[test]
fn an_unquoted_expansion_tested_with_n_is_found() {
	let f = one(
		"$ [ -n $(git status --porcelain) ] && echo dirty\n",
		"vanishing-operand",
		"$(git status --porcelain)",
	);
	assert_eq!(fix(&f), "write `\"$(git status --porcelain)\"`");
	one("$ test -n $TOKEN && echo set\n", "vanishing-operand", "$TOKEN");
	one("$ [ -n ${TOKEN} ] && echo set\n", "vanishing-operand", "${TOKEN}");
	one("$ [ -n `cat token` ] && echo set\n", "vanishing-operand", "`cat token`");
}

#[test]
fn a_quoted_or_never_empty_operand_is_left_alone() {
	clean("$ [ -n \"$TOKEN\" ] && echo set\n");
	clean("$ [ -n {{ ARG.token }} ] && echo set\n");
	clean("$ [ -z $TOKEN ] && echo unset\n");
	clean("$ [ -n ${TOKEN:-x} ] && echo set\n");
	clean("$ [[ -n $TOKEN ]] && echo set\n");
}

#[test]
fn a_regex_or_a_word_where_a_test_wants_a_number_is_found() {
	one("$ [ \"$v\" =~ ^v[0-9] ] && echo tag\n", "test-regex", "=~");
	one("$ test \"$v\" =~ ^v && echo tag\n", "test-regex", "=~");
	let f = one("$ [ \"$branch\" -eq main ] && echo main\n", "not-a-number", "main");
	assert_eq!(fix(&f), "compare text with `=`");
	one("$ [ \"\" -lt 3 ] && echo\n", "not-a-number", "\"\"");
	one("$ [ 0x10 -gt 3 ] && echo\n", "not-a-number", "0x10");
}

#[test]
fn a_regex_in_double_brackets_or_a_whole_number_is_left_alone() {
	clean("$ [[ \"$v\" =~ ^v[0-9] ]] && echo tag\n");
	clean("$ [ \"$count\" -eq 0 ] && echo none\n");
	clean("$ [ \" 12 \" -eq 12 ] && [ -5 -lt +3 ] && [ 010 -eq 10 ] && echo\n");
	clean("$ [ {{ ARG.n }} -gt 3 ] && echo\n");
	clean("$ [[ $count -eq main ]] && echo\n");
	clean("$ [ \"$a\" = -eq ] && echo\n");
	clean("$ [ $'\\t5' -eq 5 ] && echo\n");
}

#[test]
fn a_double_bracket_test_bash_refuses_is_found() {
	let f = one("$ [[ -f a -a -f b ]] && echo both\n", "unexpected", "-a");
	assert!(f.message.contains("write `&&`"), "{}", f.message);
	one("$ [[ $x foo ]] && echo\n", "unexpected", "foo");
	one("$ [[ -f ]] && echo\n", "unexpected", "-f");
	one("$ [[ ]] && echo\n", "unexpected", "[[");
	// An interpolation could become an operator once it is rendered.
	clean("$ [[ -f {{ ARG.x }} -a -f b ]] && echo\n");
}

#[test]
fn double_brackets_are_held_to_bash_s_grammar_only_where_bash_reads_them() {
	one(
		"exec bash\n\t[[ -f a -a -f b ]] && echo both\nend\n",
		"unexpected",
		"-a",
	);
	one(
		".shell = \"/usr/bin/bash\"\n\n$ [[ -f a -a -f b ]] && echo both\n",
		"unexpected",
		"-a",
	);
	// busybox reads `[[` as `test`, where `-a` joins two tests.
	clean(".shell = \"sh\"\n\n$ [[ -f a -a -f b ]] && echo both\n");
	clean("exec busybox sh\n\t[[ -f a -a -f b ]] && echo both\nend\n");
}

#[test]
fn a_command_the_script_defines_as_a_function_is_that_function() {
	clean("$ test() { echo \"$@\"; }\n$ test a=b\n");
	clean("$ sort() { cat \"$@\"; }\n$ sort names.txt > names.txt\n");
	clean("$ find() { :; }\n$ find . -exec rm {}\n");
	clean("$ break() { :; }\n$ break\n");
	clean("$ copy() { cp \"$@\"; }\n$ copy /Y a b\n");
	// A function of another name changes nothing.
	one("$ tests() { :; }\n$ test a=b\n", "glued-comparison", "a=b");
}

// ---- syntax

#[test]
fn what_bash_refuses_to_read_is_found_where_it_starts() {
	one("$ echo it's\n", "unclosed", "'");
	one("$ if true; then\n$ \techo\n", "unclosed", "if");
	one("$ fi\n", "unexpected", "fi");
	one("$ if true; then fi\n", "unexpected", "fi");
	one("$ echo a |\n", "unclosed", "|");
	one("$ echo $(date\n", "unclosed", "$(");
	one("$ cat <<EOF\n$ hello\n", "unterminated-heredoc", "<<");
}

#[test]
fn what_bash_reads_is_not_refused() {
	clean("$ if true; then\n$ \techo yes\n$ fi\n");
	clean("exec bash\n\tfor f in *; do\n\t\techo \"$f\"\n\tdone\nend\n");
	clean("$ x=$(case a in a) echo \")\";; esac)\n");
	clean("$ echo ${x:-{a} \"${y:-\"a b\"}\" $((1 + (2)))\n");
	clean("$ [[ $x =~ ^(a|b)$ ]] && echo\n");
	clean("$ cat <<-EOF\n$ \tindented\n$ \tEOF\n");
}

#[test]
fn shell_that_is_not_bash_is_not_read() {
	clean("exec python3\n\tif x:\n\t\tprint('it's')\nend\n");
	clean(".shell = \"zsh\"\n\n$ echo ${(j:,:)arr}'\n");
	clean("exec zsh\n\techo 'unclosed\nend\n");
}

#[test]
fn a_capture_is_checked_as_its_own_script() {
	one("let x = $ echo it's\n", "unclosed", "'");
	one(
		"for f in lines($ ls '{{ ARG.dir }}')\n\tprint(f)\nend\n",
		"quoted-interpolation",
		"'{{ ARG.dir }}'",
	);
}

// ---- the mistakes this was written for

#[test]
fn the_mistakes_fixed_across_the_workspace_are_found() {
	// Each line as it stood before it was fixed by hand.
	one(
		"let port = \"8080\"\n$ sudo ss -ltnp 'sport = :{{ port }}'\n",
		"quoted-interpolation",
		"'sport = :{{ port }}'",
	);
	one(
		"$ docker run --rm --network host -v \"~/.config/stripe:/root/.config/stripe\" -it stripe/stripe-cli:latest listen\n",
		"tilde-in-quotes",
		"\"~/.config/stripe:/root/.config/stripe\"",
	);
	let f =
		findings("if RUN.os == \"windows\"\n\t$ rmdir /S /Q frontend\\target\nelse\n\t$ rm -rf frontend/target\nend\n");
	let rules: Vec<&str> = f.iter().map(|f| f.rule).collect();
	assert_eq!(rules, ["windows-command", "dropped-backslash"], "{f:#?}");
	let android = "let sdk = \"\"\nif RUN.os == \"windows\"\n\tsdk = \"$LOCALAPPDATA/Android/Sdk\"\nelse\n\tsdk = \"$HOME/Library/Android/sdk\"\nend\nlet dir = (ENV.ANDROID_HOME ? ENV.ANDROID_SDK_ROOT) ? sdk\n$ nohup \"{{ dir }}/emulator/emulator\" -avd x -no-snapshot-save >/dev/null 2>&1 &\n";
	let rules: Vec<&str> = findings(android).iter().map(|f| f.rule).collect();
	assert_eq!(
		rules,
		["unexpanded-string", "unexpanded-string", "quoted-interpolation"],
		"{:#?}",
		findings(android)
	);
	let shared = parsed(
		"let ext = \"{{ ENV.HOME }}/.local/share/gnome-shell/extensions\"\nlet manifests = \"*@*/metadata.json\"\n",
	);
	let link = "let kind = $ test -L \"{{ ext }}\" && echo symlink || echo directory\nfor manifest in lines($ ls -d {{ manifests }})\n\t$ ln -sfnT \"{{ RUN.parent }}/{{ dirname(manifest) }}\" \"{{ ext }}/{{ dirname(manifest) }}\"\nend\n";
	let f = check(link, &parsed(link), Some(std::slice::from_ref(&shared)));
	let rules: Vec<&str> = f.iter().map(|f| f.rule).collect();
	assert_eq!(
		rules,
		[
			"quoted-interpolation",
			"unexpanded-glob",
			"quoted-interpolation",
			"quoted-interpolation"
		],
		"{f:#?}"
	);
	let backup =
		"$ run backup:_arca download x -o - > \"{{ ARG.out }}.part\"\n$ mv \"{{ ARG.out }}.part\" {{ ARG.out }}\n";
	let rules: Vec<&str> = findings(backup).iter().map(|f| f.rule).collect();
	assert_eq!(
		rules,
		["quoted-interpolation", "quoted-interpolation"],
		"{:#?}",
		findings(backup)
	);
}

// ---- bash as the referee

/// Every `$` script in a runfile, with each interpolation as a plain word.
fn scripts(src: &str) -> Vec<String> {
	script_list(src)
		.iter()
		.map(|s| {
			s.chars
				.iter()
				.map(|c| if c.hole.is_some() { 'x' } else { c.c })
				.collect()
		})
		.collect()
}

/// Every script in a runfile the checker reads.
fn script_list(src: &str) -> Vec<Script> {
	fn block(b: &Block, source: &Source, out: &mut Vec<Script>) {
		for st in &b.statements {
			out.extend(script::of_statement(st, source, Some(false), true));
			match st {
				Statement::Let { value, .. } | Statement::Assign { value, .. } => expr(value, source, out),
				Statement::If {
					cond, then, otherwise, ..
				} => {
					expr(cond, source, out);
					block(then, source, out);
					if let Some(o) = otherwise {
						block(o, source, out);
					}
				}
				Statement::For { iter, body, .. } => {
					expr(iter, source, out);
					block(body, source, out);
				}
				Statement::Do { body, .. } | Statement::Loop { body, .. } | Statement::Retry { body, .. } => {
					block(body, source, out)
				}
				Statement::Match { cases, default, .. } => {
					for c in cases {
						block(&c.body, source, out);
					}
					if let Some(d) = default {
						block(d, source, out);
					}
				}
				_ => {}
			}
		}
	}
	fn expr(e: &Expr, source: &Source, out: &mut Vec<Script>) {
		match e {
			Expr::Capture { .. } => out.extend(script::of_capture(e, source, Some(false), true)),
			Expr::Call { args, .. } => args.iter().for_each(|a| expr(a, source, out)),
			Expr::Chain { lhs, rhs, .. } | Expr::Binary { lhs, rhs, .. } => {
				expr(lhs, source, out);
				expr(rhs, source, out);
			}
			Expr::Unary { rhs, .. } => expr(rhs, source, out),
			_ => {}
		}
	}
	let source = Source::new(src);
	let mut out = Vec::new();
	block(&parsed(src).body, &source, &mut out);
	out
}

/// The bash to referee with. `None` where there is none.
///
/// Found by path on Windows, never handed over as a bare `bash`: the standard
/// library looks in `System32` before PATH there, and `System32\bash.exe` is
/// WSL's launcher, not a bash. GitHub's Windows images ship it with no
/// distribution installed, so every script was answered with "Windows
/// Subsystem for Linux has no installed distributions" and taken for bash's
/// verdict. The runner passes over the launcher for the same reason, in both
/// places WSL puts one -- `System32` and the Store's `WindowsApps` alias
/// (`runfile-runtime/src/shell.rs`, `is_wsl_launcher`) -- so this does too,
/// and finds Git for Windows' bash wherever PATH has it.
fn bash_program() -> Option<std::path::PathBuf> {
	if !cfg!(windows) {
		return Some("bash".into());
	}
	let launcher = |p: &std::path::Path| {
		p.parent()
			.and_then(std::path::Path::file_name)
			.is_some_and(|dir| dir.eq_ignore_ascii_case("system32") || dir.eq_ignore_ascii_case("windowsapps"))
	};
	std::env::split_paths(&std::env::var_os("PATH")?)
		.map(|dir| dir.join("bash.exe"))
		.find(|p| p.is_file() && !launcher(p))
}

/// What `bash -n` says of a script: whether it reads it, and its warnings.
/// `None` where bash is not installed.
fn bash(script: &str) -> Option<(bool, String)> {
	use std::io::Write;
	let mut child = std::process::Command::new(bash_program()?)
		.arg("-n")
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::piped())
		.spawn()
		.ok()?;
	child.stdin.take()?.write_all(script.as_bytes()).ok()?;
	let out = child.wait_with_output().ok()?;
	Some((out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// Bash agrees with every syntax finding in `src`, and with every script
/// that has none.
fn agrees(src: &str) {
	let syntax = ["unclosed", "unexpected", "unterminated-heredoc"];
	let refused = findings(src).iter().any(|f| syntax.contains(&f.rule));
	for s in scripts(src) {
		let Some((reads, warnings)) = bash(&s) else {
			eprintln!("skipped: bash is not installed");
			return;
		};
		let warned = warnings.contains("here-document");
		assert_eq!(
			!reads || warned,
			refused,
			"bash and the checker disagree about:\n{s}\n{warnings}"
		);
	}
}

#[test]
fn bash_agrees_with_every_syntax_finding() {
	for src in [
		"$ echo it's\n",
		"$ if true; then\n$ \techo\n",
		"$ fi\n",
		"$ if true; then fi\n",
		"$ echo a |\n",
		"$ echo $(date\n",
		"$ cat <<EOF\n$ hello\n",
		"$ { echo }\n",
		"$ echo a; ;\n",
		"$ while true; do echo; done; done\n",
		"$ echo \"${x:-it's}\"\n",
		"$ case x in a) echo;; esac esac\n",
		"$ for x in a; do; echo; done\n",
		"$ echo a >\n",
		"$ if true; then\n$ \techo yes\n$ fi\n",
		"$ x=$(case a in a) echo \")\";; esac)\n",
		"$ echo ${x:-{a} \"${y:-\"a b\"}\" $((1 + (2)))\n",
		"$ [[ $x =~ ^(a|b)$ ]] && echo\n",
		"$ cat <<-EOF\n$ \tindented\n$ \tEOF\n",
		"$ echo $((echo 1) )\n",
		"$ x=$(cat <<EOF\n$ )\n$ EOF\n$ )\n",
		"$ echo `echo }` ${x:-`echo }`}\n",
		"$ f() ( echo )\n",
		"$ for ((i=0;i<3;i++)); do :; done\n",
		"$ select x in a b; do break; done\n",
		"$ cat < <(ls) > >(cat)\n",
		"$ a=(1 2\n$ 3)\n",
		"$ if(true)then echo; fi\n",
		"$ echo a && # c\n$ echo b\n",
		"$ [[ -f a -a -f b ]] && echo\n",
		"$ [[ a -o b ]] && echo\n",
		"$ [[ a b ]]\n",
		"$ [[ -f ]]\n",
		"$ [[ a == ]]\n",
		"$ [[ ( a ]]\n",
		"$ [[ ( ) ]]\n",
		"$ [[ ( a ) b ]]\n",
		"$ [[ ]]\n",
		"$ [[ ! ]]\n",
		"$ [[ ! -n ]]\n",
		"$ [[ a || b c ]]\n",
		"$ [[ a == b && ]]\n",
		"$ [[ && a ]]\n",
		"$ [[ a < b > c ]]\n",
		"$ [[ \"-f\" x ]]\n",
		"$ [[ \\( a \\) ]]\n",
		"$ [[ a = b = c ]]\n",
		"$ [[ < ]]\n",
		"$ [[ -f a && ( -f b || -f c ) ]] && echo\n",
		"$ [[ -n ! ]] && [[ a == ! ]] && [[ -eq ]] && [[ == ]] && [[ -n == ]] && echo\n",
		"$ [[ ! a == b ]] && [[ a < b ]] && [[ $x ]] && [[ ! ( a ) ]] && [[ a -nt b ]] && echo\n",
		"$ [[ $x =~ ^(a|b)$ ]] && [[ $x =~ ^(a b)$ ]] && echo\n",
		"$ [[ $x == @(a|b) ]] && [[ a<b ]] && [[ a&&b ]] && [[ (a) ]] && echo\n",
		"$ [[ -f a\n$ ]] && echo\n",
	] {
		agrees(src);
	}
}

/// What bash does with `script` under `-e`, in a directory of its own: its
/// status, and everything it wrote. `None` where bash is not installed.
fn bash_runs(script: &str) -> Option<(i32, String)> {
	static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
	let bash = bash_program()?;
	let dir = std::env::temp_dir().join(format!(
		"runfile-shell-{}-{}",
		std::process::id(),
		NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
	));
	std::fs::create_dir_all(&dir).ok()?;
	let out = std::process::Command::new(bash)
		.args(["-e", "-c", script])
		.current_dir(&dir)
		.stdin(std::process::Stdio::null())
		.output();
	let _ = std::fs::remove_dir_all(&dir);
	let out = out.ok()?;
	let text = format!(
		"{}{}",
		String::from_utf8_lossy(&out.stdout),
		String::from_utf8_lossy(&out.stderr)
	);
	Some((out.status.code().unwrap_or(-1), text))
}

#[test]
fn bash_does_what_each_rule_says_it_does() {
	// A rule that is not about syntax is about what running the script does,
	// so what running it does is asked of bash: each script is flagged by the
	// rule named, and fails -- or answers wrongly -- the way its message says.
	/// Whether bash's status and output show what the rule says.
	type Holds = fn(i32, &str) -> bool;
	let cases: &[(&str, &str, Holds)] = &[
		("missing-bracket", "[ -f x", |code, out| {
			code != 0 && out.contains("missing")
		}),
		(
			"glued-comparison",
			"a=1; b=2; [ \"$a\"=\"$b\" ] && echo always",
			|_, out| out.contains("always"),
		),
		("vanishing-operand", "x=; [ -n $x ] && echo always", |_, out| {
			out.contains("always")
		}),
		("test-regex", "[ a =~ a ]", |code, _| code != 0),
		// 2 is `[` refusing its operands rather than answering false; the
		// wording is bash's to change, and 5.3 shortened it to `integer expected`.
		("not-a-number", "[ x -eq main ]", |code, out| {
			code == 2 && out.contains("integer")
		}),
		("outside-loop", "break; echo carried on", |code, out| {
			code == 0 && out.contains("carried on")
		}),
		("spaced-assignment", "export X = 1", |code, _| code != 0),
		("dollar-assignment", "$FOO=bar", |code, _| code == 127),
		("dollar-assignment", "for $f in a; do :; done", |code, _| code != 0),
		("unterminated-exec", "find . -maxdepth 0 -exec echo {}", |code, _| {
			code != 0
		}),
		(
			"truncated-input",
			"printf 'a\\nb\\n' > f; sort f > f; wc -c < f",
			|_, out| out.trim() == "0",
		),
	];
	for (rule, script, holds) in cases {
		let src = format!("$ {script}\n");
		let found = findings(&src);
		assert!(
			!found.is_empty() && found.iter().all(|f| f.rule == *rule),
			"`{rule}` on {script}: {found:#?}"
		);
		let Some((code, out)) = bash_runs(script) else {
			eprintln!("skipped: bash is not installed");
			return;
		};
		assert!(holds(code, &out), "`{rule}`: bash ran {script} and gave {code}:\n{out}");
	}
}

// ---- the corpus

/// Every finding across a list of runfiles, for reading by hand: set
/// `RUNFILE_CORPUS_LIST` to a file with one path per line. The language's own
/// findings are printed with the shell's. Each script is also held to bash,
/// both ways: one the checker reads that bash refuses is `MISSED`, and a file
/// the checker refuses whose every script bash reads is `REFUSED`.
#[test]
fn corpus() {
	let Ok(list) = std::env::var("RUNFILE_CORPUS_LIST") else {
		return;
	};
	let paths = std::fs::read_to_string(list).expect("the list");
	let (mut files, mut reports) = (0, Vec::new());
	let (mut read, mut lost) = (0, 0);
	// Where each interpolation sits, which is what the runner writes values for.
	// One left unplaced is written the old way, so each is worth a look.
	let (mut placed, mut unplaced) = (0, 0);
	for path in paths.lines().filter(|l| !l.is_empty()) {
		let Ok(src) = std::fs::read_to_string(path) else {
			continue;
		};
		let Ok(file) = runfile_lang::parse(&src) else { continue };
		files += 1;
		for s in script_list(&src) {
			match crate::syntax::parse(&s.chars, s.bash) {
				Err(crate::syntax::Stop::Lost) => {
					lost += 1;
					let text: String = s.chars.iter().map(|c| c.c).collect();
					reports.push(format!("{path}: LOST: {}", text.lines().next().unwrap_or_default()));
				}
				_ => read += 1,
			}
			if let Some(spots) = crate::syntax::spots(&s.chars, s.holes.len(), s.bash) {
				for (k, spot) in spots.iter().enumerate() {
					if spot.is_some() {
						placed += 1;
					} else {
						unplaced += 1;
						let text: String = s.chars.iter().map(|c| c.c).collect();
						reports.push(format!(
							"{path}: UNPLACED: interpolation {k} in {}",
							text.lines().next().unwrap_or_default()
						));
					}
				}
			}
		}
		let chain = shared_above(std::path::Path::new(path));
		let shared = path.ends_with("_shared.run");
		let language = runfile_lang::check::check(&src, &file, chain.as_deref(), shared);
		for f in check(&src, &file, chain.as_deref()).into_iter().chain(language) {
			reports.push(format!(
				"{path}:{}: [{}] {}{}",
				f.span.line,
				f.rule,
				f.message,
				f.fix.map(|x| format!(" -- {x}")).unwrap_or_default()
			));
		}
		let refused = findings(&src)
			.iter()
			.any(|f| matches!(f.rule, "unclosed" | "unexpected" | "unterminated-heredoc"));
		let verdicts: Vec<Option<(bool, String)>> = scripts(&src).iter().map(|s| bash(s)).collect();
		if refused {
			let reads = |v: &Option<(bool, String)>| matches!(v, Some((true, w)) if !w.contains("here-document"));
			if verdicts.iter().all(reads) {
				reports.push(format!(
					"{path}: REFUSED: the checker refuses a file bash reads every script of"
				));
			}
		} else {
			for (reads, why) in verdicts.into_iter().flatten() {
				if !reads {
					reports.push(format!(
						"{path}: MISSED: bash refuses a script the checker reads: {why}"
					));
				}
			}
		}
	}
	println!(
		"{files} files, {read} scripts read, {lost} given up on, {placed} interpolations placed, {unplaced} not\n{}",
		reports.join("\n")
	);
}

/// The `_shared.run` files above a runfile, outermost first, up to its
/// `runfiles` directory. `None` when one of them does not parse.
fn shared_above(path: &std::path::Path) -> Option<Vec<Target>> {
	let mut found = Vec::new();
	let mut dir = path.parent();
	while let Some(d) = dir {
		let shared = d.join("_shared.run");
		if shared.is_file() && shared != path {
			let text = std::fs::read_to_string(&shared).ok()?;
			found.push(runfile_lang::parse(&text).ok()?);
		}
		if d.file_name().is_some_and(|n| n == "runfiles") {
			break;
		}
		dir = d.parent();
	}
	found.reverse();
	Some(found)
}

#[test]
fn a_deeply_nested_parameter_expansion_gives_up_instead_of_overflowing() {
	// `${x+${x+…}}` recurses dollar→parameter→dollar; past the bound the word
	// reader gives up (`Stop::Lost`) rather than overflowing the stack and
	// aborting the process (audit SA-022). Reaching this line is the assertion.
	let line = format!("$ echo {}y{}\n", "${x+".repeat(4096), "}".repeat(4096));
	let _ = findings(&line);
}

#[test]
fn a_very_wide_conditional_does_not_overflow() {
	// `[[ x || x || … ]]` tens of thousands wide used to recurse once per `||`;
	// `or`/`and` are loops now (audit SA-022).
	let line = format!("$ [[ {}x ]]\n", "x || ".repeat(60000));
	let _ = findings(&line);
}

#[test]
fn nested_loops_do_not_cause_an_exponential_walk() {
	// A sequential `for` walks its body twice for flow sensitivity, which is
	// multiplicative across nesting -- 40 nested loops would be 2^40 walks
	// without the budget (audit SA-024). Run in a thread with a timeout so a
	// regression fails the test rather than hanging the suite.
	let mut src = String::from("$ true\n");
	for _ in 0..40 {
		src = format!("for x in [1, 2]\n{src}end\n");
	}
	let (tx, rx) = std::sync::mpsc::channel();
	std::thread::spawn(move || {
		let _ = findings(&src);
		let _ = tx.send(());
	});
	assert!(
		rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(),
		"the shell-check walk did not terminate -- the loop budget regressed"
	);
}

//! How wide text is on a terminal, and cutting it to fit.

use crate::term::{fit, width_of};

#[test]
fn a_line_that_fits_is_left_exactly_as_it_is() {
	assert_eq!(
		fit("Debug build of the whole workspace", 80),
		"Debug build of the whole workspace"
	);
	assert_eq!(
		fit("exactly ten", 11),
		"exactly ten",
		"a line as wide as the room still fits"
	);
}

#[test]
fn a_line_that_does_not_fit_ends_in_an_ellipsis_at_the_edge() {
	let cut = fit("Checks every project for uncommitted changes", 20);
	assert_eq!(width_of(&cut), 20, "{cut:?}");
	assert!(cut.ends_with('…'), "{cut:?}");
}

#[test]
fn the_ellipsis_sits_against_the_last_word_rather_than_after_a_space() {
	// Cut right after `every`, the next character is a space; `every …` reads
	// as a word left hanging where `every…` reads as a word cut short.
	assert_eq!(fit("Checks every project", 13), "Checks every…");
}

#[test]
fn a_wide_character_takes_two_columns_and_is_never_half_drawn() {
	assert_eq!(width_of("日本"), 4);
	assert_eq!(width_of("🚀 Deploy"), 9, "the rocket is two columns, not one");
	// The emoji below U+1F000 that are wide too, not only the blocks above it.
	assert_eq!(width_of("✅ ❌ ⚡"), 8);
	// Four columns of room leave three before the ellipsis, and `日本語`'s
	// second character would straddle the edge: it is left out whole.
	assert_eq!(fit("日本語テキスト", 4), "日…");
}

#[test]
fn a_combining_mark_takes_no_column_of_its_own() {
	// `e` plus a combining acute is one visible character.
	assert_eq!(width_of("cafe\u{301}"), 4);
}

#[test]
fn no_room_at_all_is_nothing_and_one_column_is_the_ellipsis() {
	assert_eq!(fit("anything", 0), "");
	assert_eq!(fit("anything", 1), "…");
}

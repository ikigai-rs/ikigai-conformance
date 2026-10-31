# Accessibility

What `ikigai-conformance` emits that people read, the rules that output follows, how it is
tested, and how to report a barrier. Its one product people read is the conformance report a
module's test prints when it fails. It is not a conformance claim (about accessibility or
anything else). Adapted from the [ACCESSIBILITY.md
template](https://github.com/mgifford/ACCESSIBILITY.md) (MIT).

| Field | Value |
|---|---|
| Target | WCAG 2.2 Level AA, for any page that presents this output; this repository writes text only |
| Status | Evaluation in progress. No conformance claim. |
| Owner | the maintainers of this repository |
| Report a barrier | [a GitHub issue](https://github.com/ikigai-rs/ikigai-conformance/issues/new?labels=accessibility) with the `accessibility` label; anything security-sensitive through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-conformance/security/advisories/new) |
| Last reviewed | 2026-10-31 |

## What it emits that people read

- **The report**: the `Display` of `Report` (`src/report.rs`), printed by `assert_clean`,
  `assert_clean_with` and `check(..).unwrap()` when a module's test fails, so it reaches a person
  through `cargo test` output in a terminal or a CI log. It is plain lines: an optional `run:`
  label; one line per finding (`Finding`'s `Display`: the endpoint id, the check's label, the
  verb, then the detail); a count line; `checked:` and `skipped:` lists; and evidence lines
  (`probed:`, `required:`, `recomputed:`, `pipeline:`, `opted out:`, `unprobed:` and others).
- **The finding details** (`src/suite.rs`): each finding's sentence, written where the check
  runs.
- **The missed-expectation lines** `assert_clean_with` prints above the report.

Not this repository's: the terminal, CI page or editor that shows the test output, and how
`cargo test` frames a failed assertion.

## The rules that output follows

Verified by reading the source on 2026-10-31:

- One finding per line, the endpoint id first, so a failing report reads as a checklist, line by
  line, without a table or layout to follow.
- A finding says what is wrong in words and, in most checks, what to do about it (for example,
  `declare an rdfs:Class IRI for an entity or an XSD datatype IRI for a scalar`).
- The report states what did NOT happen in words rather than by absence: a walk that probed
  nothing says so, a face with zero triples says `nothing was checked`, and a check waived on
  some endpoints is starred with a line saying on how many it ran.
- Every endpoint id and run label the report prints, and a refused value a finding quotes, has
  its control characters escaped, so a module's string cannot split a line or write a terminal
  escape.
- No color and no markup is written.

## How it is tested

- `src/report.rs`: `a_finding_line_leads_with_the_endpoint_id`,
  `every_id_the_report_prints_is_escaped` and `the_report_prints_skipped_checks_and_opt_outs`
  (which also pins the `nothing was checked` line).
- `tests/violations.rs`: `a_control_character_in_an_endpoint_id_never_splits_a_report_line`.
- `tests/ergonomics.rs`: `a_label_names_the_run_on_the_reports_first_line`,
  `assert_clean_with_names_every_expectation_a_clean_report_missed`.
- `tests/evidence.rs`: `a_clean_required_prints_every_input_it_dropped`.
- **No automated accessibility checker, browser or assistive technology has been used on this
  output.**

## Known limitations

Found by reading the source on 2026-10-31, not by testing:

- Check labels are capitals run together or hyphenated (`ARGSPECS`, `SKOLEM-RDF`,
  `REQUIRES-VERB`), which a screen reader may spell letter by letter.
- A finding line separates its columns with two spaces, not with words or punctuation, so read
  aloud the id, the label and the detail run together.
- Counts are written `finding(s)`, `endpoint(s)`, `triple(s)`, which a screen reader reads with
  the parentheses.
- An opt-out's reason and an `opted out at:` pattern are printed as the suite wrote them, not
  escaped, so a reason holding a newline splits its line.
- Many details are long single lines and use backticks, `…` and `—`; a reader on a narrow
  terminal or a braille display gets them wrapped and with that punctuation read aloud.

## Contributing

Read this file before changing the report or any finding's wording. Keep one finding per line,
name the endpoint first, say what to do, keep meaning in words, and say in the pull request what
was tested and what was not.

## Reporting a barrier

[Open an issue](https://github.com/ikigai-rs/ikigai-conformance/issues/new?labels=accessibility)
with the `accessibility` label (the link sets it when GitHub lets you add labels; otherwise a
maintainer adds it): what you were doing, what happened, and your terminal or editor and any
assistive technology, if you are comfortable sharing them. Anything security-sensitive goes
through [private vulnerability reporting](https://github.com/ikigai-rs/ikigai-conformance/security/advisories/new).
**We acknowledge a report within 7 days**, and promise nothing further.

## Change history

| Date | Change |
|---|---|
| 2026-10-31 | First version. |

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
| Last reviewed | 2026-11-12 |

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
- **The race report**: the `Display` of `race::RaceReport` (`src/race.rs`), printed by
  `assert_holds` when a concurrency probe fails. One count line (`the invariant broke in K of N
  round(s) of C concurrent call(s):`), then up to five `round R: <the invariant's message>`
  lines and an `and M more` line; one line when every round held.

Not this repository's: the terminal, CI page or editor that shows the test output, and how
`cargo test` frames a failed assertion.

## The rules that output follows

Output design rules; the results and limits of verification are recorded below:

- One finding per line, the endpoint id first, so a failing report reads as a checklist, line by
  line, without a table or layout to follow.
- A finding says what is wrong in words and, in most checks, what to do about it (for example,
  `declare an rdfs:Class IRI for an entity or an XSD datatype IRI for a scalar`).
- The report states what did NOT happen in words rather than by absence: a walk that probed
  nothing says so, a face with zero triples says `nothing was checked`, and a check waived on
  some endpoints is starred with a line saying on how many it ran.
- Endpoint ids and run labels in `Report`, and refused values in validation findings, have
  control characters escaped. This does not cover every string: opt-out patterns, reasons
  and race invariant messages are printed verbatim.
- No color and no markup is written.

## How it is tested

Every named check has a result: Pass, Fail or Blocked. A Pass covers only the stated
assertions, not overall accessibility or the terminal, CI page or editor presenting the text.
The tests below ran on 2026-11-12 against main commit
[`2eb62278a534ebd3b3f3bb6ec197a83d454d43c1`](https://github.com/ikigai-rs/ikigai-conformance/commit/2eb62278a534ebd3b3f3bb6ec197a83d454d43c1),
using `cargo test --all-targets`, Cargo 1.99.1 and rustc 1.99.1, on macOS. CI repeats these
unit and integration tests through `cargo test --workspace`; it does not repeat manual
source reviews or assistive-technology evaluations. The source review recorded on
2026-10-31 is historical context, not a current Pass.

| Check | Result | Evidence |
|---|---|---|
| Finding starts with the endpoint id and expresses the check and detail in text | Pass | `src/report.rs`: `a_finding_line_leads_with_the_endpoint_id`, run with the command above. Exact text assertions; does not assess spoken clarity or every finding's advice. |
| Endpoint ids do not split report lines | Pass | `src/report.rs`: `every_id_the_report_prints_is_escaped`; `tests/violations.rs`: `a_control_character_in_an_endpoint_id_never_splits_a_report_line`. Tested newline fixtures; not every possible user-supplied string. |
| Skipped checks, opt-outs and empty RDF coverage are stated in words | Pass | `src/report.rs`: `the_report_prints_skipped_checks_and_opt_outs`, including `nothing was checked`; `tests/violations.rs`: `identical_waivers_group_onto_one_line_and_checked_says_it_was_partial` and `the_report_names_the_rdf_faces_the_walk_actually_reached`. Text assertions only. |
| Run label appears first and escapes control characters | Pass | `tests/ergonomics.rs`: `a_label_names_the_run_on_the_reports_first_line`. Does not test how a terminal or screen reader announces it. |
| Missed expectations are named above the report | Pass | `tests/ergonomics.rs`: `assert_clean_with_names_every_expectation_a_clean_report_missed`. Checks panic text, not the test runner's presentation. |
| Required-input evidence names dropped inputs | Pass | `tests/evidence.rs`: `a_clean_required_prints_every_input_it_dropped`. Checks recorded inputs and printed evidence, not readability of all evidence lines. |
| Race success and failure reports use words and include the invariant's message | Pass | `tests/race.rs`: `copies_pass_an_atomic_update`, `copies_catch_a_lost_update` and `assert_holds_panics_with_the_report`. Checks success text, failure text and panic; does not assert the five-message truncation layout or escape invariant messages. |
| Race report limits displayed violations to five and states the remainder | Blocked | Source inspection on 2026-11-12 found `take(5)` and `and M more` in `src/race.rs`; no dedicated assertion of this layout was run, and CI does not repeat source inspection. |
| Every finding offers useful corrective advice | Blocked | Historical source review, 2026-10-31; no current systematic wording evaluation. Text tests above cover selected examples only. CI does not repeat this review. |
| No color or markup is emitted across all report paths | Blocked | Historical source review, 2026-10-31; no dedicated automated check or current rendering evaluation. CI does not repeat this review. |
| Automated accessibility checker | Blocked | No checker was run; no checker is configured for this plain-text output. Rust text assertions do not evaluate WCAG 2.2 AA. |
| Browser presentation | Blocked | No browser evaluation was run; the repository supplies text, and no consuming CI page or editor was evaluated. |
| Manual keyboard access | Blocked | No person performed a keyboard pass in a terminal, CI log or editor for this review; no interactive evaluation session was available. |
| Screen reader | Blocked | No screen reader was used on this output; no screen reader session was available. Pronunciation and reading order remain unevaluated. |
| Screen magnifier | Blocked | No magnifier was used; no magnifier session was available. Narrow-view reading remains unevaluated. |
| Speech input | Blocked | No speech-input software was used; no speech-input session was available. |
| Switch access | Blocked | No switch-access software was used; no switch-access session was available. |
| Braille display | Blocked | No braille display was used; no braille hardware was available. Wrapping and punctuation remain unevaluated. |

## Known limitations

Source observations recorded on 2026-10-31; the output forms below remain in main.
These are potential barriers, not measured assistive-technology results:

- Check labels are capitals run together or hyphenated (`ARGSPECS`, `SKOLEM-RDF`,
  `REQUIRES-VERB`), which a screen reader may spell letter by letter.
- A finding line separates its columns with two spaces, not with words or punctuation, which may make
  the id, label and detail difficult to distinguish when read aloud; this remains untested.
- Counts are written `finding(s)`, `endpoint(s)`, `triple(s)`, whose pronunciation with
  assistive technology has not been evaluated.
- An opt-out's reason and an `opted out at:` pattern are printed as the suite wrote them, not
  escaped, so a reason holding a newline splits its line.
- Many details are long single lines and use backticks, `…` and `—`; wrapping and
  punctuation handling depend on the terminal and assistive technology and remain untested.
- Race invariant messages are printed verbatim; a message containing a newline can split
  the displayed round across lines.

## Contributing

Read this file before changing the report or any finding's wording. Keep one finding per line,
name the endpoint first, say what to do, keep meaning in words, and record each check in the
pull request in a `Check | Result | Evidence` table. Use
Pass, Fail or Blocked, with the test or command, tool and version, or why it could not run.
Keep unrun checks Blocked, including assistive technology nobody used. A historical run CI
does not repeat stays Blocked with its date.

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
| 2026-11-12 | Record current main test results and explicit Blocked evaluations, with coverage limits. |

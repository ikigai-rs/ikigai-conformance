//! `RESOLVES` (ledger #338): a failed minimal resolution is reported under its own
//! label, once per action, with the refusal's kind, the check that needed it and, where
//! the walk had to invent a value for an untyped input, that input by name. Before it,
//! the failure was reported under whichever check reached it first, so it read as a
//! caching finding on an endpoint whose caching was fine.

use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Report, Suite};
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// A cacheable read that refuses any `query` but a real one: the shape of a SPARQL
/// endpoint walked with no fixture. `typed` says whether `query` declares a class.
fn sparql(typed: bool) -> FnEndpoint {
    let query = ArgSpec::new("query");
    let query = if typed {
        query.class(XSD_STRING)
    } else {
        query
    };
    FnEndpoint::new("sparql", |inv: &Invocation<'_>| {
        let q = inv.inline_str("query")?;
        if !q.trim_start().to_ascii_uppercase().starts_with("SELECT") {
            return Err(Error::InvalidArgument {
                name: "query".into(),
                detail: format!("not a SPARQL query: {q:?}"),
            });
        }
        Ok(
            Representation::new(ReprType::new("text/plain"), "rows".as_bytes().to_vec())
                .cacheable(),
        )
    })
    .with_description(
        Description::new("sparql")
            .verb(Verb::Source)
            .input(query)
            .output("text/plain"),
    )
}

fn walk(suite: Suite, endpoint: FnEndpoint) -> Report {
    suite.run_blocking(&Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:example:sparql"), endpoint),
    )))
}

fn details(report: &Report, check: Check) -> Vec<String> {
    report.of(check).map(|f| f.detail.clone()).collect()
}

#[test]
fn a_failed_minimal_resolution_is_reported_under_resolves_once() {
    let report = walk(Suite::new().pure("sparql"), sparql(true));
    let resolves = details(&report, Check::Resolves);
    assert_eq!(resolves.len(), 1, "once per action:\n{report}");
    let detail = &resolves[0];
    assert!(
        detail.starts_with(
            "did not resolve with the minimal inputs its ArgSpecs allow (refused as \
             `invalid-argument`: invalid argument `query`: not a SPARQL query: \"x\")"
        ),
        "{report}"
    );
    // The check that needed it is named, and no longer carries the failure.
    assert!(
        detail.contains("Needed by CACHEABLE, and by every check that reads the same resolution"),
        "{report}"
    );
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
    assert!(report.of(Check::Outputs).next().is_none(), "{report}");
    // Typed input: the walk sent a sampled value, not an invented one, so no BECAUSE.
    assert!(!detail.contains("no value to send"), "{report}");
}

#[test]
fn an_untyped_input_the_walk_invented_a_value_for_is_named_as_the_cause() {
    let report = walk(Suite::new().pure("sparql"), sparql(false));
    // Two findings, and the second says the first is why: one cause, one fix.
    assert!(
        report
            .of(Check::ArgSpecs)
            .any(|f| f.detail.contains("input `query` has no class")),
        "{report}"
    );
    let resolves = details(&report, Check::Resolves);
    assert_eq!(resolves.len(), 1, "{report}");
    assert!(
        resolves[0].contains(
            "The walk had no value to send for `query`: it declares no class, so it sent the \
             placeholder `x` (the ARGSPECS finding on this action)"
        ),
        "{report}"
    );
    // A fixture that supplies it takes the BECAUSE away: the value was not invented.
    let fixed = walk(
        Suite::new().pure("sparql").fixture(
            ikigai_conformance::Fixture::new("sparql", Verb::Source).arg("query", "ASK {}"),
        ),
        sparql(false),
    );
    assert!(
        details(&fixed, Check::Resolves)
            .iter()
            .all(|d| !d.contains("no value to send")),
        "{fixed}"
    );
}

/// A selection written before 0.8.4 never names RESOLVES, and must read as it did.
#[test]
fn unselected_the_failure_stays_under_the_check_that_needed_it() {
    let report = walk(
        Suite::new()
            .pure("sparql")
            .checks(Checks::all() - Checks::RESOLVES),
        sparql(true),
    );
    assert!(report.of(Check::Resolves).next().is_none(), "{report}");
    let cacheable = details(&report, Check::Cacheable);
    assert_eq!(cacheable.len(), 1, "{report}");
    assert!(
        cacheable[0].starts_with("did not resolve with the minimal inputs"),
        "{report}"
    );
    assert!(!cacheable[0].contains("Needed by"), "{report}");
    assert!(report.to_string().contains("skipped: RESOLVES"), "{report}");
}

#[test]
fn resolves_is_waivable_and_an_idle_waiver_is_a_note() {
    let waived = walk(
        Suite::new().pure("sparql").opt_out_check(
            "sparql",
            Check::Resolves,
            "the store is seeded by the host",
        ),
        sparql(true),
    );
    assert!(waived.is_clean(), "{waived}");
    assert!(
        waived.idle_waivers().is_empty(),
        "it set the failure aside:\n{waived}"
    );

    let resolving = walk(
        Suite::new()
            .pure("sparql")
            .fixture(
                ikigai_conformance::Fixture::new("sparql", Verb::Source).arg("query", "SELECT *"),
            )
            .opt_out_check("sparql", Check::Resolves, "the store is seeded by the host"),
        sparql(true),
    );
    assert!(resolving.is_clean(), "{resolving}");
    assert_eq!(resolving.idle_waivers().len(), 1, "{resolving}");
    assert!(
        resolving
            .to_string()
            .contains("note: the waiver of RESOLVES for sparql suppressed nothing"),
        "{resolving}"
    );
}

/// A waived check that needed a resolution which failed observed nothing, so its
/// waiver cannot be judged idle: no `note:` saying it suppressed nothing.
#[test]
fn a_waiver_whose_check_saw_only_a_failed_resolution_is_not_called_idle() {
    let report = walk(
        Suite::new().pure("sparql").opt_out_check(
            "sparql",
            Check::Cacheable,
            "measured in the host's own test",
        ),
        sparql(true),
    );
    // OUTPUTS still needed the resolution, so the failure is reported, naming it.
    assert!(
        details(&report, Check::Resolves)
            .iter()
            .any(|d| d.contains("Needed by OUTPUTS")),
        "{report}"
    );
    assert!(report.idle_waivers().is_empty(), "{report}");
}

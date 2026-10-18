//! What every composing adopter wrote by hand around a `Report` (ledger #354, #341):
//! the `(endpoint, check)` key, a richer `assert_clean`, a label saying which run
//! produced a report, and who owns a finding's fix.

use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Expected, Finding, Fixture, Report, Suite};
use ikigai_core::{
    builtins, ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

fn upper() -> FnEndpoint {
    FnEndpoint::new("upper", |inv: &Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            inv.inline_str("in")?.to_uppercase().into_bytes(),
        )
        .cacheable())
    })
    .with_description(
        Description::new("upper")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").class(XSD_STRING))
            .output("text/plain"),
    )
}

fn clean_kernel() -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:example:upper"), upper()),
    ))
}

fn panic_text(f: impl FnOnce()) -> String {
    let err = catch_unwind(AssertUnwindSafe(f)).expect_err("expected a panic");
    err.downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default()
}

#[test]
fn keys_are_the_endpoint_check_pairs_as_a_set() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:toUpper"), builtins::to_upper())
        .bind(Exact::new("urn:example:upper"), upper());
    // `to-upper` has an untyped input and is a pure function nobody declared pure:
    // two checks against one endpoint, so two keys.
    let report = Suite::new()
        .checks(Checks::ARGSPECS | Checks::CACHEABLE)
        .pure("upper")
        .run_blocking(&Kernel::new(Arc::new(space)));
    let keys: BTreeSet<(&str, Check)> = report.keys();
    assert_eq!(
        keys,
        BTreeSet::from([
            ("to-upper", Check::ArgSpecs),
            ("to-upper", Check::Cacheable)
        ]),
        "{report}"
    );
}

#[test]
fn a_label_names_the_run_on_the_reports_first_line() {
    let report = Suite::new()
        .pure("upper")
        .label("mounted")
        .run_blocking(&clean_kernel());
    assert_eq!(report.label(), Some("mounted"));
    assert_eq!(report.to_string().lines().next(), Some("run: mounted"));
    // Unlabeled, nothing is printed for it.
    let unlabeled = Suite::new().pure("upper").run_blocking(&clean_kernel());
    assert_eq!(unlabeled.label(), None);
    assert!(!unlabeled.to_string().contains("run:"), "{unlabeled}");
    // A label is printed with its control characters escaped, like an id.
    let odd = Suite::new()
        .pure("upper")
        .label("a\nb")
        .run_blocking(&clean_kernel());
    assert!(odd.to_string().starts_with("run: a\\u{a}b\n"), "{odd}");
}

#[test]
fn assert_clean_with_passes_a_report_that_matches() {
    Suite::new()
        .pure("upper")
        .run_blocking(&clean_kernel())
        .assert_clean_with(
            &Expected::new()
                .endpoints(1)
                .actions(1)
                .walked(["upper"])
                .no_opt_outs()
                .nothing_unprobed(),
        );
}

#[test]
fn assert_clean_with_names_every_expectation_a_clean_report_missed() {
    let report: Report = Suite::new()
        .pure("upper")
        .opt_out_check("upper", Check::Names, "kept on purpose")
        .run_blocking(&clean_kernel());
    assert!(report.is_clean(), "{report}");
    let text = panic_text(|| {
        report.assert_clean_with(
            &Expected::new()
                .endpoints(2)
                .actions(1)
                .walked(["upper", "lower"])
                .no_opt_outs(),
        )
    });
    for needle in [
        "endpoints: expected 2, walked 1",
        "walked: expected exactly lower upper; not walked: lower; walked but not expected: none",
        "opt-outs: expected none, the suite declares 1",
    ] {
        assert!(text.contains(needle), "{needle:?}:\n{text}");
    }
    assert!(
        !text.contains("actions:"),
        "a met expectation is not listed:\n{text}"
    );
    // The whole report follows the list, as `assert_clean` prints it.
    assert!(text.contains("0 finding(s) across 1 endpoint(s)"), "{text}");
}

#[test]
fn assert_clean_with_still_fails_a_report_with_findings() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
    let report = Suite::new().run_blocking(&Kernel::new(Arc::new(space)));
    let text = panic_text(|| report.assert_clean_with(&Expected::new()));
    assert!(
        text.starts_with(&format!("{} finding(s)", report.findings.len())),
        "{text}"
    );
}

/// ledger #341: a DECLARATIONS finding names the endpoint its declaration was about,
/// and is owned by the suite; a host partitioning on `owner()` files it correctly.
#[test]
fn a_declaration_finding_is_owned_by_the_suite_not_the_endpoint_it_names() {
    let report = Suite::new()
        .pure("upper")
        // The suite author's mistakes, each about `upper`: a fixture for an action
        // it does not declare, and a waiver of a check that cannot run on it.
        .fixture(Fixture::new("upper", Verb::Sink).arg("content", "x"))
        .opt_out_check("upper", Check::Authority, "it never writes")
        .own("upper")
        .run_blocking(&clean_kernel());
    let declarations: Vec<&Finding> = report.of(Check::Declarations).collect();
    assert_eq!(declarations.len(), 2, "{report}");
    for f in &declarations {
        assert_eq!(
            f.endpoint, "upper",
            "printed by the id it is about:\n{report}"
        );
        assert_eq!(f.owner(), None, "but owned by the suite:\n{report}");
    }
    // A host's partition: the module's own findings are the ones it owns.
    let module: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.owner() == Some("upper"))
        .collect();
    assert!(module.is_empty(), "the module is blameless:\n{report}");
    // A suite-wide finding is the suite's too.
    let typo = Suite::new()
        .checks(Checks::NAMES)
        .own("uper")
        .run_blocking(&clean_kernel());
    assert!(typo.findings.iter().all(|f| f.owner().is_none()), "{typo}");
}

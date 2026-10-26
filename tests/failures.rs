//! `FAILURES` and `Suite::fails_under` (ledger #1546): an action that states its failure
//! classes (core 0.1.108's `ActionSpec::fails` / `Description::fails`) is held to them.
//! Every failure the walk observes must have a class core's `ActionSpec::admits_failure`
//! admits, and a `fails_under` probe must fail with exactly the class it states. The check runs by
//! default since 0.11.0, including probes without `Suite::check_failures`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Fixture, Report, Suite};
use ikigai_core::{
    ArgSpec, Capability, Description, EndpointSpace, Error, Exact, FailureClass, Failures,
    FnEndpoint, Invocation, Kernel, ReprType, Representation, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

fn kernel(id: &str, endpoint: FnEndpoint) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new(format!("urn:example:{id}")), endpoint),
    ))
}

/// A bounded parser over `content`: past `bound` bytes it fails with `over`, and every
/// call is counted. `declared` is what its description states (`None`: nothing).
fn parser(
    bound: usize,
    over: fn(usize) -> Error,
    declared: Option<&[FailureClass]>,
    calls: Arc<AtomicUsize>,
) -> FnEndpoint {
    let mut description = Description::new("parse")
        .verb(Verb::Source)
        .input(ArgSpec::new("content").class(XSD_STRING))
        .output("text/plain");
    match declared {
        // The builder's form, one class at a time.
        Some(classes) if !classes.is_empty() => {
            for class in classes {
                description = description.fails(*class);
            }
        }
        // "Only the implied classes" has no builder: the field says it.
        Some(_) => description.failures = Failures::Only(Vec::new()),
        None => {}
    }
    FnEndpoint::new("parse", move |inv: &Invocation<'_>| {
        calls.fetch_add(1, Ordering::SeqCst);
        let content = inv.inline_str("content")?;
        if content.len() > bound {
            return Err(over(content.len()));
        }
        Ok(text(content.to_uppercase()))
    })
    .with_description(description)
}

fn over_budget(n: usize) -> Error {
    Error::OverBudget {
        detail: format!("{n} bytes, past the bound"),
    }
}

fn opaque(n: usize) -> Error {
    Error::Endpoint(format!("could not parse {n} bytes"))
}

/// A bound of 0: the walk's own sample already fails.
fn always_failing(over: fn(usize) -> Error, declared: Option<&[FailureClass]>) -> Kernel {
    kernel(
        "parse",
        parser(0, over, declared, Arc::new(AtomicUsize::new(0))),
    )
}

fn failures(suite: Suite, kernel: &Kernel) -> Report {
    suite
        .checks(Checks::FAILURES | Checks::DECLARATIONS)
        .run_blocking(kernel)
}

#[test]
fn a_declared_action_failing_with_an_undeclared_class_is_a_finding() {
    let report = failures(
        Suite::new(),
        &always_failing(opaque, Some(&[FailureClass::OverBudget])),
    );
    let found: Vec<_> = report.of(Check::Failures).collect();
    assert_eq!(found.len(), 1, "{report}");
    let detail = &found[0].detail;
    assert!(
        detail.contains("declares its failure classes (over-budget)"),
        "{report}"
    );
    assert!(
        detail.contains("the minimal resolution failed as `endpoint`, class `unknown`"),
        "{report}"
    );
    assert!(
        detail.contains("`.fails(FailureClass::Unknown)`"),
        "{report}"
    );
    let seen = &report.failures_seen()[0];
    assert_eq!(seen.failed, [FailureClass::Unknown]);
    assert_eq!(seen.resolutions, 1);
}

#[test]
fn a_declared_class_and_an_implied_one_are_admitted() {
    // OverBudget is declared; the walk's sample fails with it.
    let report = failures(
        Suite::new(),
        &always_failing(over_budget, Some(&[FailureClass::OverBudget])),
    );
    report.assert_clean();
    assert!(
        report.to_string().contains(
            "failures: parse source declares over-budget: 1 resolution(s), failed as over-budget\n"
        ),
        "{report}"
    );
    // `permanent` is implied for every action, so an empty declaration admits it.
    let invalid = |_: usize| Error::InvalidArgument {
        name: "content".into(),
        detail: "bad".into(),
    };
    let report = failures(Suite::new(), &always_failing(invalid, Some(&[])));
    report.assert_clean();
    assert!(
        report
            .to_string()
            .contains("failures: parse source declares only the implied classes:"),
        "{report}"
    );
}

#[test]
fn an_unstated_action_is_listed_and_never_a_finding() {
    let report = failures(Suite::new(), &always_failing(opaque, None));
    report.assert_clean();
    let seen = &report.failures_seen()[0];
    assert_eq!(seen.declared, None);
    assert_eq!(seen.failed, [FailureClass::Unknown]);
    assert!(
        report.to_string().contains(
            "failures: 1 action(s) state no failure classes, so nothing was compared: parse source\n"
        ),
        "{report}"
    );
}

#[test]
fn a_fails_under_probe_that_gets_the_wrong_class_is_a_finding() {
    // Bound 4: the walk's sample passes, the probe's input does not, and the bound
    // refuses as an opaque `Endpoint` error.
    let k = kernel(
        "parse",
        parser(
            4,
            opaque,
            Some(&[FailureClass::OverBudget]),
            Arc::new(AtomicUsize::new(0)),
        ),
    );
    let probe = Fixture::new("parse", Verb::Source).arg("content", "far too long");
    let report = failures(
        Suite::new().fails_under(probe, FailureClass::OverBudget),
        &k,
    );
    let details: Vec<&str> = report
        .of(Check::Failures)
        .map(|f| f.detail.as_str())
        .collect();
    assert_eq!(details.len(), 2, "{report}");
    // The probe's own verdict: the wrong class.
    assert!(
        details[0]
            .contains("was to fail with class `over-budget`, and got `endpoint` (class `unknown`)"),
        "{report}"
    );
    // And its failure is an observation like any other, so the declaration is held to it.
    assert!(
        details[1].contains("a `fails_under` probe failed as `endpoint`, class `unknown`"),
        "{report}"
    );
    let probe = &report.fails_under_probes()[0];
    assert_eq!(probe.expected, FailureClass::OverBudget);
    assert_eq!(probe.got, Some(FailureClass::Unknown));
    assert!(
        report.to_string().contains(
            "fails under: parse source content=\"far too long\" expecting over-budget: failed \
             endpoint, class unknown — not the stated class\n"
        ),
        "{report}"
    );
}

#[test]
fn a_fails_under_probe_that_resolves_is_a_finding_and_the_right_class_is_clean() {
    let k = |bound| {
        kernel(
            "parse",
            parser(
                bound,
                over_budget,
                Some(&[FailureClass::OverBudget]),
                Arc::new(AtomicUsize::new(0)),
            ),
        )
    };
    let probe = || Fixture::new("parse", Verb::Source).arg("content", "far too long");
    // A bound larger than the probe's input: nothing was provoked.
    let report = failures(
        Suite::new().fails_under(probe(), FailureClass::OverBudget),
        &k(100),
    );
    let finding = report.of(Check::Failures).next().expect("{report}");
    assert!(
        finding
            .detail
            .contains("resolved: it was to fail with class `over-budget`"),
        "{report}"
    );
    assert_eq!(report.fails_under_probes()[0].got, None);
    // A bound the probe's input passes: the stated class, and clean.
    let report = failures(
        Suite::new().fails_under(probe(), FailureClass::OverBudget),
        &k(4),
    );
    report.assert_clean();
    assert!(
        report.to_string().contains(
            "fails under: parse source content=\"far too long\" expecting over-budget: failed \
             over-budget\n"
        ),
        "{report}"
    );
}

#[test]
fn failures_run_by_default_and_subtracting_the_check_turns_them_off() {
    let run = |suite: Suite| {
        let calls = Arc::new(AtomicUsize::new(0));
        let k = kernel(
            "parse",
            parser(0, opaque, Some(&[FailureClass::OverBudget]), calls.clone()),
        );
        let report = suite.run_blocking(&k);
        (report, calls.load(Ordering::SeqCst))
    };
    let (default, default_calls) = run(Suite::new());
    let (subtracted, subtracted_calls) = run(Suite::new().checks(Checks::all() - Checks::FAILURES));
    let (selected, selected_calls) = run(Suite::new().checks(Checks::all()));
    assert_eq!(default.of(Check::Failures).count(), 1, "{default}");
    assert!(!default.failures_seen().is_empty());
    assert!(!default.declared.evidence.check_failures);
    assert_eq!(default.to_string(), selected.to_string());
    assert_eq!(default_calls, subtracted_calls);
    assert_eq!(selected_calls, subtracted_calls);
    assert_eq!(subtracted.of(Check::Failures).count(), 0, "{subtracted}");
    assert!(subtracted.failures_seen().is_empty());
    assert_eq!(
        subtracted.of(Check::Declarations).count(),
        0,
        "{subtracted}"
    );
    assert!(
        subtracted
            .to_string()
            .lines()
            .any(|l| l.starts_with("skipped:") && l.ends_with(" FAILURES")),
        "{subtracted}"
    );
    assert!(
        !default
            .to_string()
            .lines()
            .any(|l| l.starts_with("skipped:") && l.contains("FAILURES")),
        "{default}"
    );
    let (explicit, explicit_calls) = run(Suite::new().check_failures());
    assert_eq!(explicit_calls, default_calls);
    assert_eq!(explicit.to_string(), default.to_string());
    assert!(explicit.declared.evidence.check_failures);
}

#[test]
fn failures_alone_resolves_a_read_once_and_never_fires_a_write() {
    let calls = Arc::new(AtomicUsize::new(0));
    let k = kernel("parse", parser(100, opaque, Some(&[]), calls.clone()));
    failures(Suite::new(), &k).assert_clean();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let writes = Arc::new(AtomicUsize::new(0));
    let counted = writes.clone();
    let sink = FnEndpoint::new("notes", move |_: &Invocation<'_>| {
        counted.fetch_add(1, Ordering::SeqCst);
        Err(Error::Endpoint("disk full".into()))
    })
    .with_description(
        Description::new("notes")
            .verb(Verb::Sink)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain")
            .requires("urn:cap:notes:write")
            .fails(FailureClass::Transient),
    );
    let k = kernel("notes", sink);
    let report = failures(Suite::new(), &k);
    report.assert_clean();
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert!(
        report
            .to_string()
            .contains("failures: notes sink declares transient: 0 resolution(s), none failed\n"),
        "{report}"
    );
    // Under PIPELINE the write is fired once, and FAILURES judges that firing.
    let report = Suite::new()
        .checks(Checks::PIPELINE | Checks::FAILURES)
        .check_failures()
        .run_blocking(&k);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    let finding = report.of(Check::Failures).next().expect("{report}");
    assert!(
        finding
            .detail
            .contains("the pipeline probe's firing failed as `endpoint`, class `unknown`"),
        "{report}"
    );
}

#[test]
fn a_denial_under_no_grants_and_a_required_drop_are_left_to_their_own_checks() {
    // Declares no `requires`, yet refuses a caller holding nothing: ENFORCED's
    // undeclared gate. FAILURES does not report the same denial a second time.
    let gate = FnEndpoint::new("gate", |inv: &Invocation<'_>| {
        if inv.capability.scopes().is_some_and(|s| s.is_empty()) {
            return Err(Error::Denied("no grants".into()));
        }
        Ok(text("ok"))
    })
    .with_description(
        Description::new("gate")
            .verb(Verb::Source)
            .output("text/plain")
            .fails(FailureClass::Transient),
    );
    let report = Suite::new()
        .checks(Checks::ENFORCED | Checks::FAILURES)
        .check_failures()
        .run_blocking(&kernel("gate", gate));
    assert_eq!(report.of(Check::Enforced).count(), 1, "{report}");
    assert_eq!(report.of(Check::Failures).count(), 0, "{report}");

    // Refuses a missing required input as an opaque `Endpoint`: REQUIRED's finding.
    let strict = FnEndpoint::new("strict", |inv: &Invocation<'_>| {
        if inv.inline_str("name").is_err() {
            return Err(Error::Endpoint("no name".into()));
        }
        Ok(text("ok"))
    })
    .with_description(
        Description::new("strict")
            .verb(Verb::Source)
            .input(ArgSpec::new("name").class(XSD_STRING))
            .output("text/plain")
            .fails(FailureClass::Transient),
    );
    let report = Suite::new()
        .checks(Checks::REQUIRED | Checks::FAILURES)
        .check_failures()
        .run_blocking(&kernel("strict", strict));
    assert_eq!(report.of(Check::Required).count(), 1, "{report}");
    assert_eq!(report.of(Check::Failures).count(), 0, "{report}");
}

#[test]
fn a_denial_beside_a_requires_is_implied() {
    let read = FnEndpoint::new("secret", |_: &Invocation<'_>| Ok(text("s"))).with_description(
        Description::new("secret")
            .verb(Verb::Source)
            .output("text/plain")
            .requires("urn:cap:secret:read")
            .fails(FailureClass::Transient),
    );
    let k = kernel("secret", read);
    // The fixture's caller lacks the scope, so the minimal resolution is refused.
    let report = Suite::new()
        .checks(Checks::FAILURES)
        .check_failures()
        .fixture(
            Fixture::new("secret", Verb::Source).capability(Capability::scoped(["urn:cap:other"])),
        )
        .run_blocking(&k);
    report.assert_clean();
    assert_eq!(report.failures_seen()[0].failed, [FailureClass::Denied]);
}

#[test]
fn a_waiver_sets_the_finding_aside() {
    let k = always_failing(opaque, Some(&[FailureClass::OverBudget]));
    let report = failures(
        Suite::new().opt_out_check("parse", Check::Failures, "classified next release"),
        &k,
    );
    report.assert_clean();
    // The waiver suppressed a finding, so it is not idle.
    assert!(report.idle_waivers().is_empty(), "{report}");
}

#[test]
fn probes_run_without_an_explicit_request_and_unused_requests_are_findings() {
    let k = always_failing(over_budget, Some(&[FailureClass::OverBudget]));
    // A selected probe runs without an explicit request.
    let report = Suite::new()
        .checks(Checks::FAILURES | Checks::DECLARATIONS)
        .fails_under(
            Fixture::new("parse", Verb::Source).arg("content", "long"),
            FailureClass::OverBudget,
        )
        .run_blocking(&k);
    report.assert_clean();
    assert_eq!(report.fails_under_probes().len(), 1);
    assert!(!report.declared.evidence.check_failures);
    // An unselected probe still reports why it never ran.
    let report = Suite::new()
        .checks(Checks::DECLARATIONS)
        .fails_under(
            Fixture::new("parse", Verb::Source),
            FailureClass::OverBudget,
        )
        .run_blocking(&k);
    assert_eq!(report.of(Check::Declarations).count(), 1, "{report}");
    assert!(
        report
            .of(Check::Declarations)
            .next()
            .unwrap()
            .detail
            .contains("FAILURES is not selected"),
        "{report}"
    );
    // An explicit request without the check produces exactly one finding.
    let report = Suite::new()
        .checks(Checks::NAMES | Checks::DECLARATIONS)
        .check_failures()
        .run_blocking(&k);
    assert_eq!(report.of(Check::Declarations).count(), 1, "{report}");
    let finding = report.of(Check::Declarations).next().expect("{report}");
    assert!(
        finding
            .detail
            .contains("`Suite::check_failures` reached nothing: FAILURES is not selected"),
        "{report}"
    );
    // A probe naming a verb the endpoint does not declare.
    let report = failures(
        Suite::new().fails_under(Fixture::new("parse", Verb::Sink), FailureClass::OverBudget),
        &k,
    );
    let finding = report.of(Check::Declarations).next().expect("{report}");
    assert!(
        finding.detail.contains("`parse` declares no sink action"),
        "{report}"
    );
}

//! What a clean report stands on (ledger #1493): the inputs `REQUIRED` dropped and
//! what each drop got, a plain line when it had nothing to drop, a bytes fixture for
//! an endpoint whose input is not text, and a note for a waiver that suppressed
//! nothing.
//!
//! Every case is falsified both ways: the line is there when the thing happened, and
//! absent (or different) when it did not. The waiver cases COUNT firings, because the
//! promise that matters is the one no outcome shows: judging a waiver never fires a
//! `Sink` the walk would not otherwise fire.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ikigai_conformance::{Check, Checks, Fixture, Report, Suite};
use ikigai_core::{
    ActionSpec, ArgSpec, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, Kernel,
    ReprType, Representation, UriTemplate, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// The 20-byte gzip of an empty input: what a decompressor accepts and no `String`
/// fixture could carry (`0x8b` is not UTF-8).
const EMPTY_GZIP: [u8; 20] = [
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
];

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

fn kernel(space: EndpointSpace) -> Kernel {
    Kernel::new(Arc::new(space))
}

/// A `Source` that reads every named input with `inline_str`, so a missing one is a
/// typed `MissingArgument`, and counts its runs.
fn typed(id: &'static str, inputs: &'static [&'static str], runs: Arc<AtomicUsize>) -> FnEndpoint {
    let mut description = Description::new(id).verb(Verb::Source).output("text/plain");
    for name in inputs {
        description = description.input(ArgSpec::new(*name).class(XSD_STRING));
    }
    FnEndpoint::new(id, move |inv: &Invocation<'_>| {
        runs.fetch_add(1, Ordering::SeqCst);
        let mut out = String::new();
        for name in inputs {
            out.push_str(inv.inline_str(name)?);
        }
        Ok(text(out))
    })
    .with_description(description)
}

/// A `Source` declaring `in` required that does without it (the runbook's shape).
fn lenient() -> FnEndpoint {
    FnEndpoint::new("lenient", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("in").unwrap_or("friend").to_string()))
    })
    .with_description(
        Description::new("lenient")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").class(XSD_STRING))
            .output("text/plain"),
    )
}

/// A `Source` refusing a missing `key` untyped: the shape `REQUIRED` reports.
fn untyped_refusal() -> FnEndpoint {
    FnEndpoint::new("seal", |inv: &Invocation<'_>| {
        let key = inv
            .inline_str("key")
            .map_err(|_| Error::Endpoint("no key".into()))?;
        Ok(text(format!("sealed with {key}")))
    })
    .with_description(
        Description::new("seal")
            .verb(Verb::Source)
            .input(ArgSpec::new("key").class(XSD_STRING))
            .output("text/plain"),
    )
}

fn line(report: &Report, needle: &str) -> bool {
    report.to_string().lines().any(|l| l.contains(needle))
}

// ----- REQUIRED's evidence ------------------------------------------------------

#[test]
fn a_clean_required_prints_every_input_it_dropped() {
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:example:greet"),
            typed("greet", &["name", "greeting"], Arc::default()),
        )
        .bind(Exact::new("urn:example:lenient"), lenient());
    let report = Suite::new().run_blocking(&kernel(space));
    eprintln!("{report}");

    let probes: Vec<(&str, &str, Option<&str>)> = report
        .required_probes()
        .iter()
        .map(|p| (p.endpoint.as_str(), p.input.as_str(), p.refusal.as_deref()))
        .collect();
    assert_eq!(
        probes,
        [
            // Declaration order within an action, walk order across them.
            ("greet", "name", Some("missing-argument")),
            ("greet", "greeting", Some("missing-argument")),
            ("lenient", "in", None),
        ],
        "{report}"
    );
    assert_eq!(report.required_nothing(), None);
    let text = report.to_string();
    for expected in [
        "required: dropped 3 input(s) across 2 endpoint(s)\n",
        "required: greet source without `greeting`: refused missing-argument\n",
        "required: greet source without `name`: refused missing-argument\n",
        "required: lenient source without `in`: resolved — declared required, actually optional\n",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    assert!(!text.contains("required: nothing dropped"), "{text}");
    // The evidence and the finding agree: the one drop that resolved is the finding.
    let found: Vec<&str> = report
        .of(Check::Required)
        .map(|f| f.endpoint.as_str())
        .collect();
    assert_eq!(found, ["lenient"], "{report}");
}

#[test]
fn an_untyped_refusal_names_its_kind() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), untyped_refusal());
    let report = Suite::new().run_blocking(&kernel(space));
    assert!(
        line(
            &report,
            "required: seal source without `key`: refused endpoint"
        ),
        "{report}"
    );
    assert_eq!(report.of(Check::Required).count(), 1, "{report}");
}

/// The ikigai-fs shape: a read that takes only a binding, and a write requiring
/// `content`. Under the default verbs `REQUIRED` has nothing to drop.
fn binding_read_and_content_write() -> FnEndpoint {
    FnEndpoint::new("file", |inv: &Invocation<'_>| {
        if inv.request.verb == Verb::Sink {
            return Ok(text(format!("wrote {}", inv.inline_str("content")?.len())));
        }
        Ok(text(inv.bindings.get("path").unwrap_or("").to_string()))
    })
    .with_description(
        Description::new("file")
            .action(
                ActionSpec::new(Verb::Source)
                    .input(ArgSpec::new("path").class(XSD_STRING).binding())
                    .output("text/plain"),
            )
            .action(
                ActionSpec::new(Verb::Sink)
                    .requires("urn:cap:example:write")
                    .input(ArgSpec::new("path").class(XSD_STRING).binding())
                    .input(ArgSpec::new("content").class(XSD_STRING))
                    .output("text/plain"),
            ),
    )
}

#[test]
fn a_required_with_nothing_to_drop_says_why() {
    let space = || {
        EndpointSpace::new().bind(
            UriTemplate::parse("urn:file:{path}").unwrap(),
            binding_read_and_content_write(),
        )
    };
    // Default verbs: the read takes only a binding, the write is not walked.
    let report = Suite::new().run_blocking(&kernel(space()));
    eprintln!("{report}");
    assert!(report.required_probes().is_empty(), "{report}");
    let why = report.required_nothing().expect("a reason");
    assert!(
        why.contains("no `Source` or `Exists` action declares a required by-value input")
            && why.contains("a `Sink` or `Delete` does")
            && why.contains("`Suite::required_on_mutating`"),
        "{why}"
    );
    assert!(
        line(
            &report,
            "required: nothing dropped — no `Source` or `Exists`"
        ),
        "{report}"
    );
    assert!(
        report.to_string().contains("REQUIRED"),
        "still listed as checked"
    );

    // The opt-in reaches the write, and the evidence says what it dropped.
    let report = Suite::new()
        .required_on_mutating()
        .run_blocking(&kernel(space()));
    eprintln!("{report}");
    assert_eq!(report.required_nothing(), None, "{report}");
    assert!(
        line(
            &report,
            "required: file sink without `content`: refused missing-argument"
        ),
        "{report}"
    );

    // No mutating action at all, and only an optional input: mutating verbs are off
    // and there is nothing on the reads either.
    let stats = FnEndpoint::new("stats", |_inv: &Invocation<'_>| Ok(text("0"))).with_description(
        Description::new("stats")
            .verb(Verb::Source)
            .input(ArgSpec::new("since").class(XSD_STRING).optional())
            .output("text/plain"),
    );
    let report = Suite::new().run_blocking(&kernel(
        EndpointSpace::new().bind(Exact::new("urn:example:stats"), stats),
    ));
    let why = report.required_nothing().expect("a reason");
    assert!(
        why.contains("`Sink` and `Delete` are not walked (`Suite::required_on_mutating` is off)"),
        "{why}"
    );

    // Something to drop, but the only action that has it is opted out.
    let report = Suite::new()
        .opt_out("greet", None, "spawns a process")
        .run_blocking(&kernel(EndpointSpace::new().bind(
            Exact::new("urn:example:greet"),
            typed("greet", &["name"], Arc::default()),
        )));
    let why = report.required_nothing().expect("a reason");
    assert!(
        why.contains("was opted out, waived, or did not resolve"),
        "{why}"
    );

    // Not selected: no line either way.
    let report = Suite::new()
        .checks(Checks::all() - Checks::REQUIRED)
        .run_blocking(&kernel(space()));
    assert_eq!(report.required_nothing(), None);
    assert!(!report.to_string().contains("required:"), "{report}");
}

// ----- a bytes fixture ------------------------------------------------------------

/// A decompressor's shape: refuses anything that is not a gzip stream, and records
/// what it was handed.
fn gunzip(seen: Arc<Mutex<Vec<Vec<u8>>>>) -> FnEndpoint {
    FnEndpoint::new("gunzip", move |inv: &Invocation<'_>| {
        let bytes = inv.inline_arg("content")?;
        seen.lock().unwrap().push(bytes.to_vec());
        if !bytes.starts_with(&[0x1f, 0x8b]) {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "not a gzip stream".into(),
            });
        }
        Ok(text(""))
    })
    .with_description(
        Description::new("gunzip")
            .verb(Verb::Source)
            // Opaque bytes: `xsd:string` is the type the wire carries (README).
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn a_bytes_fixture_lets_a_binary_input_endpoint_be_walked() {
    let space = |seen: &Arc<Mutex<Vec<Vec<u8>>>>| {
        EndpointSpace::new().bind(Exact::new("urn:example:gunzip"), gunzip(seen.clone()))
    };

    // Without one: no `String` is a gzip stream, so nothing about it is observed.
    let seen = Arc::default();
    let report = Suite::new().run_blocking(&kernel(space(&seen)));
    eprintln!("{report}");
    assert!(!report.is_clean(), "the suite's text sample is refused");
    assert!(report.required_probes().is_empty(), "{report}");
    assert!(
        line(&report, "unprobed: gunzip source REQUIRED"),
        "{report}"
    );

    // With one, the walk reaches it and REQUIRED bites.
    let seen = Arc::default();
    let report = Suite::new()
        .fixture(Fixture::new("gunzip", Verb::Source).arg_bytes("content", EMPTY_GZIP))
        .run_blocking(&kernel(space(&seen)));
    eprintln!("{report}");
    assert!(report.is_clean(), "{report}");
    assert!(
        line(
            &report,
            "fixture: gunzip source content=0x1f8b0800000000000003030000000000… (20 bytes)"
        ),
        "{report}"
    );
    assert!(
        line(
            &report,
            "required: gunzip source without `content`: refused missing-argument"
        ),
        "{report}"
    );
    assert!(
        line(&report, "probed: gunzip source `text/plain`: 0 byte(s)"),
        "{report}"
    );
    // Byte for byte: what the endpoint was handed is the fixture, never re-encoded.
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|b| b == &EMPTY_GZIP), "{seen:?}");
}

// ----- a waiver that suppressed nothing ------------------------------------------

#[test]
fn a_waiver_that_suppressed_nothing_is_a_note_never_a_finding() {
    let runs = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:greet"),
        typed("greet", &["name"], runs.clone()),
    );
    let report = Suite::new()
        .opt_out_check(
            "greet",
            Check::Required,
            "the upstream refusal is untyped until 0.4",
        )
        .run_blocking(&kernel(space));
    eprintln!("{report}");
    assert!(
        report.is_clean(),
        "a note never turns a suite red:\n{report}"
    );
    let idle: Vec<(&str, Check, &str)> = report
        .idle_waivers()
        .iter()
        .map(|w| (w.endpoint.as_str(), w.check, w.reason.as_str()))
        .collect();
    assert_eq!(
        idle,
        [(
            "greet",
            Check::Required,
            "the upstream refusal is untyped until 0.4"
        )]
    );
    assert!(
        report.to_string().contains(
            "note: the waiver of REQUIRED for greet suppressed nothing — the check ran with it \
             set aside and found nothing; drop it if its reason has passed (the upstream \
             refusal is untyped until 0.4)\n"
        ),
        "{report}"
    );
    // The set-aside run's drops are NOT evidence: the waiver is what the report says.
    assert!(report.required_probes().is_empty(), "{report}");
    assert!(
        report
            .required_nothing()
            .is_some_and(|why| why.contains("waived")),
        "{report}"
    );

    // Its footprint on a read is the footprint the unwaived check has: the minimal
    // call, the no-grants call, one drop. Nothing more.
    let unwaived = Arc::new(AtomicUsize::new(0));
    Suite::new().run_blocking(&kernel(EndpointSpace::new().bind(
        Exact::new("urn:example:greet"),
        typed("greet", &["name"], unwaived.clone()),
    )));
    assert_eq!(runs.load(Ordering::SeqCst), unwaived.load(Ordering::SeqCst));
}

#[test]
fn a_waiver_doing_its_job_gets_no_note() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), untyped_refusal());
    let report = Suite::new()
        .opt_out_check("seal", Check::Required, "sign types its refusals in 0.4")
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert!(report.idle_waivers().is_empty(), "{report}");
    assert!(!report.to_string().contains("note:"), "{report}");
}

#[test]
fn a_description_only_waiver_is_judged_by_reading_the_description() {
    let upper = |id: &'static str| typed(id, &["in"], Arc::default());
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:Upper"), upper("Upper"))
        .bind(Exact::new("urn:example:lower"), upper("lower"));
    let report = Suite::new()
        .opt_out_check("Upper", Check::Names, "a proper noun")
        .opt_out_check("lower", Check::Names, "was `Lower` once")
        .run_blocking(&kernel(space));
    eprintln!("{report}");
    assert!(report.is_clean(), "{report}");
    let idle: Vec<&str> = report
        .idle_waivers()
        .iter()
        .map(|w| w.endpoint.as_str())
        .collect();
    assert_eq!(idle, ["lower"], "{report}");
    // The checked line still stars NAMES: the waivers are in force either way.
    assert!(line(&report, "* NAMES ran on"), "{report}");
}

#[test]
fn a_declarations_waiver_is_judged_too() {
    let typo = || {
        EndpointSpace::new().bind(
            Exact::new("urn:example:greet"),
            typed("greet", &["name"], Arc::default()),
        )
    };
    // A fixture for a verb `greet` does not declare is inert: the waiver is needed.
    let report = Suite::new()
        .fixture(Fixture::new("greet", Verb::Sink).arg("content", "x"))
        .opt_out_check("greet", Check::Declarations, "the sink lands next release")
        .run_blocking(&kernel(typo()));
    assert!(report.is_clean(), "{report}");
    assert!(report.idle_waivers().is_empty(), "{report}");

    // Nothing inert to waive: the same waiver suppressed nothing.
    let report = Suite::new()
        .opt_out_check("greet", Check::Declarations, "the sink lands next release")
        .run_blocking(&kernel(typo()));
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.idle_waivers().len(), 1, "{report}");
}

#[test]
fn judging_a_waiver_never_fires_a_sink_the_walk_would_not() {
    // A Sink declaring `content`, PIPELINE waived: the pipeline probe is what would
    // fire it under root, so judging the waiver would need a firing it may not make.
    let walk = |suite: Suite| {
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = fired.clone();
        let notes = FnEndpoint::new("notes-write", move |inv: &Invocation<'_>| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(text(format!("stored {}", inv.inline_str("content")?.len())))
        })
        .with_description(
            Description::new("notes-write")
                .verb(Verb::Sink)
                .requires("urn:cap:example:write")
                .input(ArgSpec::new("content").class(XSD_STRING))
                .output("text/plain"),
        );
        let report = suite.run_blocking(&kernel(
            EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes),
        ));
        (report, fired.load(Ordering::SeqCst))
    };
    let (waived, fired_waived) = walk(Suite::new().opt_out_check(
        "notes-write",
        Check::Pipeline,
        "a write the test cannot undo",
    ));
    let (_, fired_unselected) = walk(Suite::new().checks(Checks::all() - Checks::PIPELINE));
    eprintln!("{waived}");
    assert_eq!(
        fired_waived, fired_unselected,
        "the waiver fired the Sink exactly as often as not selecting PIPELINE does"
    );
    assert!(
        waived.idle_waivers().is_empty(),
        "unjudged, so no note: {waived}"
    );
    assert!(waived.is_clean(), "{waived}");
}

#[test]
fn a_structurally_inert_waiver_keeps_its_finding_and_gets_no_note() {
    // No required input at all: DECLARATIONS already says the waiver could not
    // apply. The note would say the same thing twice, and more weakly.
    let stats = FnEndpoint::new("stats", |_inv: &Invocation<'_>| Ok(text("0"))).with_description(
        Description::new("stats")
            .verb(Verb::Source)
            .input(ArgSpec::new("since").class(XSD_STRING).optional())
            .output("text/plain"),
    );
    let report = Suite::new()
        .opt_out_check("stats", Check::Required, "defaults its window")
        .run_blocking(&kernel(
            EndpointSpace::new().bind(Exact::new("urn:example:stats"), stats),
        ));
    assert_eq!(report.of(Check::Declarations).count(), 1, "{report}");
    assert!(report.idle_waivers().is_empty(), "{report}");
}

//! The suite against the contract shape of core 0.1.105 (ledger #1547): input groups,
//! `needs`, examples, an empty enumeration, open and envelope argument sets, and
//! `output_from`. Each test is an endpoint that honors the new declaration (clean) or
//! breaks it on purpose (caught).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Fixture, Report, Suite};
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Error, Exact, FnEndpoint, InputGroup, Invocation, Kernel,
    OutputFrom, ReprType, Representation, UriTemplate, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_ANY_URI: &str = "http://www.w3.org/2001/XMLSchema#anyURI";

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

fn kernel(space: EndpointSpace) -> Kernel {
    Kernel::new(Arc::new(space))
}

fn assert_caught(report: &Report, check: Check, needle: &str) {
    assert!(
        report.of(check).any(|f| f.detail.contains(needle)),
        "expected a {check} finding containing {needle:?}; got:\n{report}"
    );
}

/// What a piped-or-named intake does with its two members.
#[derive(Clone, Copy)]
enum Intake {
    /// Exactly one of `content` or `in`: none is `MissingArgument`, both is
    /// `InvalidArgument`. What the group says.
    Honest,
    /// Neither is fine: it falls back to a placeholder document.
    DefaultsWhenNone,
    /// Both are fine: `content` wins.
    TakesBoth,
    /// Reads `content` only; `in` is declared and never read.
    IgnoresIn,
}

/// `urn:example:lift`: reads a document from `content` (where a pipe lands) or `in`.
fn lift(intake: Intake) -> FnEndpoint {
    FnEndpoint::new("lift", move |inv: &Invocation<'_>| {
        let content = inv
            .inline_str("content")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let named = inv.inline_str("in").ok().filter(|v| !v.trim().is_empty());
        let doc = match (intake, content, named) {
            (Intake::IgnoresIn, _, _) => inv.inline_str("content")?.to_string(),
            (Intake::TakesBoth, Some(c), _) => c.to_string(),
            (Intake::Honest, Some(_), Some(_)) => {
                return Err(Error::InvalidArgument {
                    name: "in".into(),
                    detail: "give `content` or `in`, not both".into(),
                })
            }
            (_, Some(c), None) => c.to_string(),
            (_, None, Some(n)) => n.to_string(),
            (Intake::DefaultsWhenNone, None, None) => "(empty)".to_string(),
            (_, _, _) => return Err(Error::MissingArgument("content".into())),
        };
        Ok(text(doc.to_uppercase()).cacheable())
    })
    .with_description(
        Description::new("lift")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING).optional())
            .input(ArgSpec::new("in").class(XSD_STRING).optional())
            .group(InputGroup::exactly_one(["content", "in"]).pipe("content"))
            .output("text/plain"),
    )
}

fn lift_report(intake: Intake) -> Report {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:lift"), lift(intake));
    Suite::new().pure("lift").run_blocking(&kernel(space))
}

#[test]
fn an_honest_exactly_one_intake_is_clean_and_says_what_it_probed() {
    let report = lift_report(Intake::Honest);
    report.assert_clean();
    let text = report.to_string();
    for line in [
        // The minimal call filled the group's pipe member: no fixture was needed.
        "required: lift source group:exactly-one:content,in given none of its members \
         (the contract refuses it): refused missing-argument\n",
        "required: lift source group:exactly-one:content,in given `in` (the contract \
         accepts it): resolved\n",
        "required: lift source group:exactly-one:content,in given `content`, `in` (the \
         contract refuses it): refused invalid-argument\n",
    ] {
        assert!(text.contains(line), "missing {line:?}:\n{text}");
    }
    let probes = report.required_probes();
    assert_eq!(probes.len(), 3, "{report}");
    assert!(probes
        .iter()
        .all(|p| p.group.as_deref() == Some("group:exactly-one:content,in")));
    assert_eq!(probes[1].given, ["in"]);
    assert!(!probes[1].contract_refuses);
}

#[test]
fn a_group_the_endpoint_does_not_hold_is_caught_both_ways() {
    // The contract refuses a call with neither member; the endpoint serves it.
    let report = lift_report(Intake::DefaultsWhenNone);
    assert_caught(
        &report,
        Check::Required,
        "declares exactly one of `content`, `in`, but resolved with none of them",
    );
    // The contract refuses both together; the endpoint serves that too.
    let report = lift_report(Intake::TakesBoth);
    assert_caught(
        &report,
        Check::Required,
        "resolved with both `content`, `in`: the contract refuses a call the endpoint serves",
    );
    assert_caught(&report, Check::Required, "`InputGroup::at_least_one`");
    // The contract blesses `in` alone; the endpoint refuses it: the false green.
    let report = lift_report(Intake::IgnoresIn);
    assert_caught(
        &report,
        Check::Required,
        "so a call with only `in` conforms, but the endpoint refused it as missing `content`",
    );
}

#[test]
fn a_read_whose_only_intake_is_a_pipe_less_group_is_named_only() {
    let named = FnEndpoint::new("pick", |inv: &Invocation<'_>| {
        let v = inv
            .inline_str("a")
            .or_else(|_| inv.inline_str("b"))?
            .to_string();
        Ok(text(v).cacheable())
    })
    .with_description(
        Description::new("pick")
            .verb(Verb::Source)
            .input(ArgSpec::new("a").class(XSD_STRING).optional())
            .input(ArgSpec::new("b").class(XSD_STRING).optional())
            .group(InputGroup::at_least_one(["a", "b"]))
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:pick"), named);
    let report = Suite::new().pure("pick").run_blocking(&kernel(space));
    report.assert_clean(); // information, not a finding
    assert_eq!(report.named_only().len(), 1, "{report}");
    assert!(
        report.to_string().contains(
            "pipeline: pick source is named-only: `group:at-least-one:a,b` has no `pipe` member"
        ),
        "{report}"
    );
    // A pipe member removes the line.
    assert!(lift_report(Intake::Honest).named_only().is_empty());
}

#[test]
fn the_minimal_call_gives_what_an_input_needs() {
    let probe = FnEndpoint::new("probe", |inv: &Invocation<'_>| {
        if inv.inline_str("endpoint").is_ok() {
            inv.inline_str("verb")?; // `endpoint` needs `verb`, enforced
        }
        Ok(text("ok").cacheable())
    })
    .with_description(
        Description::new("probe")
            .verb(Verb::Source)
            .input(
                ArgSpec::new("endpoint")
                    .class(XSD_STRING)
                    .optional()
                    .needs(["verb"]),
            )
            .input(ArgSpec::new("verb").class(XSD_STRING).optional())
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:probe"), probe);
    // The fixture names `endpoint`; the walk adds the `verb` it needs.
    let report = Suite::new()
        .pure("probe")
        .fixture(Fixture::new("probe", Verb::Source).arg("endpoint", "urn:x"))
        .run_blocking(&kernel(space));
    report.assert_clean();
}

#[test]
fn a_declared_example_is_the_value_the_walk_sends() {
    let contrast = FnEndpoint::new("contrast", |inv: &Invocation<'_>| {
        let color = inv.inline_str("from")?;
        if !color.starts_with('#') || color.len() != 7 {
            return Err(Error::InvalidArgument {
                name: "from".into(),
                detail: format!("`{color}` is not a #rrggbb color"),
            });
        }
        Ok(text(color.to_string()).cacheable())
    })
    .with_description(
        Description::new("contrast")
            .verb(Verb::Source)
            .input(ArgSpec::new("from").class(XSD_STRING).example("#336699"))
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:contrast"), contrast);
    // No fixture: before examples this needed one only to supply a color.
    Suite::new()
        .pure("contrast")
        .run_blocking(&kernel(space))
        .assert_clean();
}

#[test]
fn an_action_whose_required_input_accepts_nothing_is_not_invoked() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let none: Vec<String> = Vec::new(); // e.g. no provider configured
    let ask = FnEndpoint::new("ask", move |_inv: &Invocation<'_>| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(text("answer"))
    })
    .with_description(
        Description::new("ask")
            .verb(Verb::Source)
            .input(ArgSpec::new("provider").class(XSD_STRING).one_of(none))
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:ask"), ask);
    let report = Suite::new().run_blocking(&kernel(space));
    report.assert_clean();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "{report}");
    assert!(
        report.to_string().contains(
            "ask source ENFORCED: uncallable, so no invoking check ran: the required input \
             `provider` declares an empty enumeration"
        ),
        "{report}"
    );
}

/// `compose`: serves the template at `src` with `{name}` markers filled from any other
/// argument, in `src`'s type.
fn compose(served: &'static str) -> FnEndpoint {
    FnEndpoint::new("compose", move |inv: &Invocation<'_>| {
        inv.inline_str("src")?;
        Ok(Representation::new(ReprType::new(served), b"<p>hi</p>".to_vec()).cacheable())
    })
    .with_description(
        Description::new("compose")
            .verb(Verb::Source)
            .input(ArgSpec::new("src").class(XSD_ANY_URI))
            .open_arguments("each fills a {name} marker in the body")
            .output_from(OutputFrom::Input("src".into())),
    )
}

fn template() -> FnEndpoint {
    FnEndpoint::new("template", |_inv: &Invocation<'_>| {
        Ok(Representation::new(ReprType::new("text/html"), b"<p>{who}</p>".to_vec()).cacheable())
    })
    .with_description(
        Description::new("template")
            .verb(Verb::Source)
            .output("text/html"),
    )
}

fn compose_report(served: &'static str) -> Report {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:compose"), compose(served))
        .bind(Exact::new("urn:example:template"), template());
    Suite::new()
        .pure("compose")
        .pure("template")
        // `who` is no declared input: on an OPEN argument set it is an argument too,
        // so the fixture naming it is not a DECLARATIONS finding.
        .fixture(
            Fixture::new("compose", Verb::Source)
                .arg("src", "urn:example:template")
                .arg("who", "world"),
        )
        .run_blocking(&kernel(space))
}

#[test]
fn output_from_an_input_holds_the_served_type_to_the_inputs_resource() {
    // Serves `src`'s type: clean, with no `Check::Outputs` waiver.
    compose_report("text/html").assert_clean();
    // Serves something else: caught.
    let report = compose_report("application/json");
    assert_caught(
        &report,
        Check::Outputs,
        "declares its output type comes from `src` (`output_from`), and `src` is \
         `urn:example:template`, which serves `text/html`; but it served `application/json`",
    );
    assert_eq!(report.findings.len(), 1, "{report}");
}

#[test]
fn a_fixture_naming_an_undeclared_input_is_still_a_finding_on_a_closed_set() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:contrast"),
        FnEndpoint::new("closed", |_inv: &Invocation<'_>| Ok(text("x").cacheable()))
            .with_description(
                Description::new("closed")
                    .verb(Verb::Source)
                    .output("text/plain"),
            ),
    );
    let report = Suite::new()
        .pure("closed")
        .fixture(Fixture::new("closed", Verb::Source).arg("who", "world"))
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "sends `who`, which no input");
}

#[test]
fn output_from_origin_is_a_declared_pass_through_not_a_finding() {
    let fetch = FnEndpoint::new("fetch", |_inv: &Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new("image/png"),
            vec![0x89, b'P'],
        ))
    })
    .with_description(
        Description::new("fetch")
            .verb(Verb::Source)
            .output("application/octet-stream")
            .output_from(OutputFrom::Origin),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:fetch"), fetch);
    let report = Suite::new().run_blocking(&kernel(space));
    report.assert_clean();
    assert!(
        report.to_string().contains(
            "fetch source OUTPUTS: a declared pass-through (`output_from` origin): served \
             `image/png`"
        ),
        "{report}"
    );
}

#[test]
fn an_envelope_sends_its_carrier_not_its_fields() {
    let intake = FnEndpoint::new("intake", |inv: &Invocation<'_>| {
        // The fields arrive inside `content`; a named field is a caller's mistake.
        if inv.inline_str("email").is_ok() {
            return Err(Error::InvalidArgument {
                name: "email".into(),
                detail: "a field belongs inside `content`".into(),
            });
        }
        Ok(text(inv.inline_str("content")?.to_string()))
    })
    .with_description(
        Description::new("intake")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .input(ArgSpec::new("email").class(XSD_STRING))
            .envelope("content")
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:intake"), intake);
    let report = Suite::new()
        .checks(Checks::all() - Check::Cacheable - Check::Recompute)
        .run_blocking(&kernel(space));
    report.assert_clean();
}

#[test]
fn validate_names_what_a_default_on_a_binding_costs() {
    let page = FnEndpoint::new("page", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("ledger").unwrap_or("default").to_string()).cacheable())
    })
    .with_description(
        Description::new("page")
            .verb(Verb::Source)
            .input(
                ArgSpec::new("ledger")
                    .class(XSD_STRING)
                    .default_value("default")
                    .binding(),
            )
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(
        UriTemplate::parse("urn:example:page:{ledger}").unwrap(),
        page,
    );
    let report = Suite::new()
        .checks(Checks::VALIDATE)
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Validate, "is a binding with a default");
    assert_caught(
        &report,
        Check::Validate,
        "A binding is part of the resource's name and is never defaulted",
    );
}

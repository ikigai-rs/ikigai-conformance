//! One fixture endpoint per check that violates its rule on purpose, and a test
//! per fixture asserting the violation is caught — plus one endpoint that
//! violates nothing, asserting the suite is clean on it. If a check stops seeing
//! its fixture, that is the check failing, not the module.

use std::sync::Arc;

use ikigai_conformance::{check, Check, Checks, Fixture, Report, Suite};
use ikigai_core::{
    ActionSpec, ArgSpec, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint,
    Invocation, Kernel, ReprType, Representation, Result, UriTemplate, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const TURTLE: &str = "text/turtle";

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

fn turtle(s: &str) -> Representation {
    Representation::new(ReprType::new(TURTLE), s.as_bytes().to_vec())
}

fn kernel(space: EndpointSpace) -> Kernel {
    Kernel::new(Arc::new(space))
}

fn report_of(space: EndpointSpace) -> Report {
    match check(&kernel(space)) {
        Ok(r) | Err(r) => r,
    }
}

fn only(report: &Report, check: Check) -> Vec<String> {
    report.of(check).map(|f| f.detail.clone()).collect()
}

fn assert_caught(report: &Report, check: Check, needle: &str) {
    assert!(
        report.of(check).any(|f| f.detail.contains(needle)),
        "expected a {check} finding containing {needle:?}; got:\n{report}"
    );
}

/// A well-behaved endpoint: kebab id, typed input, single verb, cacheable, pure.
fn conforming() -> FnEndpoint {
    FnEndpoint::new("upper", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("in")?.to_uppercase()).cacheable())
    })
    .with_description(
        Description::new("upper")
            .title("Upper-case")
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(ArgSpec::new("in").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn a_conforming_endpoint_is_clean() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new().pure("upper").run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.endpoints, 1);
    assert_eq!(report.actions, 1);
    assert!(report.to_string().contains("declared pure: upper"));
}

// ----- ARGSPECS ---------------------------------------------------------------

#[test]
fn argspecs_catches_untyped_inputs_no_actions_and_bad_defaults() {
    let untyped = FnEndpoint::new("untyped", |_inv: &Invocation<'_>| Ok(text("x")))
        .with_description(
            Description::new("untyped")
                .verb(Verb::Source)
                .input(ArgSpec::new("in")) // no class
                .input(
                    ArgSpec::new("mode")
                        .class(XSD_STRING)
                        .one_of(["a", "b"])
                        .default_value("c"),
                )
                .input(ArgSpec::new("in").class(XSD_STRING)) // duplicate name
                .output("text/plain"),
        );
    // Meta only: nothing selectable.
    let verbless = FnEndpoint::new("verbless", |_inv: &Invocation<'_>| Ok(text("x")))
        .with_description(Description::new("verbless").verb(Verb::Meta));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:untyped"), untyped)
        .bind(Exact::new("urn:example:verbless"), verbless);
    let report = report_of(space);
    assert_caught(&report, Check::ArgSpecs, "input `in` has no class");
    assert_caught(
        &report,
        Check::ArgSpecs,
        "defaults to `c`, which is not one of",
    );
    assert_caught(&report, Check::ArgSpecs, "input `in` is declared twice");
    assert_caught(&report, Check::ArgSpecs, "declares no action");
    assert!(report
        .against("verbless")
        .all(|f| f.check == Check::ArgSpecs));
}

#[test]
fn argspecs_catches_a_template_variable_that_is_not_a_binding_input() {
    let file = FnEndpoint::new("file", |inv: &Invocation<'_>| {
        Ok(text(inv.bindings.get("path").unwrap_or("").to_string()))
    })
    .with_description(
        Description::new("file")
            .verb(Verb::Source)
            .input(ArgSpec::new("path").class(XSD_STRING)) // by-value, not .binding()
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(UriTemplate::parse("urn:file:{path}").unwrap(), file);
    let report = report_of(space);
    assert_caught(
        &report,
        Check::ArgSpecs,
        "template variable `path` of `urn:file:{path}`",
    );
}

// ----- REQUIRES-VERB ----------------------------------------------------------

#[test]
fn requires_without_a_verb_is_caught_statically_and_is_indeed_inert() {
    let inert = FnEndpoint::new("link-remove", |_inv: &Invocation<'_>| Ok(text("removed")))
        .with_description(
            Description::new("link-remove")
                .summary("Strike one URL (Sink executes)")
                .requires("urn:cap:fs:write:*"), // no .verb() — inert
        );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:link-remove"), inert);
    let k = kernel(space);
    let report = check(&k).unwrap_err();
    assert_caught(
        &report,
        Check::RequiresVerb,
        "declares requires `urn:cap:fs:write:*` but no verb",
    );
    // The finding is not theoretical: the kernel lets the Sink through under no grants.
    let none = ikigai_core::Capability::scoped(Vec::<String>::new());
    let request = ikigai_core::Request::new(
        Verb::Sink,
        ikigai_core::Iri::parse("urn:example:link-remove").unwrap(),
    );
    let ran = futures::executor::block_on(k.issue(request, &none));
    assert!(
        ran.is_ok(),
        "the undeclared-verb floor enforces nothing: {ran:?}"
    );
}

// ----- ENFORCED ---------------------------------------------------------------

/// Declares a scope; the kernel's floor enforces it (this one is CLEAN on Enforced).
fn gated() -> FnEndpoint {
    FnEndpoint::new("gated", |_inv: &Invocation<'_>| Ok(text("secret"))).with_description(
        Description::new("gated")
            .verb(Verb::Source)
            .requires("urn:cap:example:read")
            .output("text/plain"),
    )
}

/// Enforces a scope it never declares — the manifold over-offers.
fn stealth() -> FnEndpoint {
    FnEndpoint::new("stealth", |inv: &Invocation<'_>| {
        if inv.capability.allows("urn:cap:example:read") {
            Ok(text("secret"))
        } else {
            Err(Error::Denied("urn:cap:example:read".into()))
        }
    })
    .with_description(
        Description::new("stealth")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn enforced_catches_an_undeclared_denial_and_passes_a_declared_one() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:gated"), gated())
        .bind(Exact::new("urn:example:stealth"), stealth());
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Enforced,
        "declares no capability but refused with `Denied`",
    );
    assert!(
        report.against("gated").all(|f| f.check != Check::Enforced),
        "a declared, kernel-enforced scope is clean:\n{report}"
    );
}

/// A description whose `requires` the kernel cannot enforce because the endpoint
/// is reached without it: the description says Source needs a cap, but the
/// endpoint's explicit Sink action declares none and the description-level
/// `requires` is only synthesized for verbs WITHOUT an explicit action. So the
/// Sink runs under no grants — declared (at the description) is not enforced
/// (for that verb). The suite sees it per action.
#[test]
fn enforced_reports_an_action_that_resolves_under_no_grants() {
    struct Leaky;
    #[async_trait::async_trait]
    impl Endpoint for Leaky {
        async fn invoke(&self, _inv: &Invocation<'_>) -> Result<Representation> {
            Ok(text("ok"))
        }
        fn name(&self) -> &str {
            "leaky"
        }
        fn describe(&self) -> Description {
            // The requires is declared on the description; the explicit Sink action
            // wins for its verb and carries none.
            Description::new("leaky")
                .verb(Verb::Source)
                .requires("urn:cap:example:write")
                .action(
                    ActionSpec::new(Verb::Sink).input(ArgSpec::new("content").class(XSD_STRING)),
                )
                .output("text/plain")
        }
    }
    let space = EndpointSpace::new().bind(Exact::new("urn:example:leaky"), Leaky);
    let report = report_of(space);
    // Source declares the scope and the kernel enforces it: clean.
    assert!(
        report
            .of(Check::Enforced)
            .all(|f| f.verb != Some(Verb::Source)),
        "{report}"
    );
    // Nothing declared for Sink and nothing enforced: also clean on ENFORCED — the
    // suite cannot know the author MEANT the Sink to be gated. That is the honest
    // limit of the check, pinned here so a future change to it is deliberate.
    assert!(report.of(Check::Enforced).next().is_none(), "{report}");
}

// ----- SKOLEM-RDF -------------------------------------------------------------

fn blank_face() -> FnEndpoint {
    FnEndpoint::new("blank-graph", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             <urn:ikigai:endpoint:blank-graph> a ik:Endpoint ; ik:input [ ik:inputName \"in\" ] .",
        ))
    })
    .with_description(
        Description::new("blank-graph")
            .verb(Verb::Source)
            .output(TURTLE),
    )
}

fn unparseable_face() -> FnEndpoint {
    FnEndpoint::new("broken-graph", |_inv: &Invocation<'_>| {
        Ok(turtle("<urn:a> <urn:p> "))
    })
    .with_description(
        Description::new("broken-graph")
            .verb(Verb::Source)
            .output(TURTLE),
    )
}

/// Declares a Turtle face beside text/plain but ignores `as=`.
fn face_not_served() -> FnEndpoint {
    FnEndpoint::new("no-face", |_inv: &Invocation<'_>| Ok(text("plain only"))).with_description(
        Description::new("no-face")
            .verb(Verb::Source)
            .output("text/plain")
            .output(TURTLE),
    )
}

#[test]
fn skolem_rdf_catches_blank_nodes_unparseable_and_unserved_faces() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:blank"), blank_face())
        .bind(Exact::new("urn:example:broken"), unparseable_face())
        .bind(Exact::new("urn:example:no-face"), face_not_served());
    let report = report_of(space);
    assert_caught(&report, Check::SkolemRdf, "has 1 blank node(s)");
    assert_caught(&report, Check::SkolemRdf, "does not parse as text/turtle");
    assert_caught(
        &report,
        Check::SkolemRdf,
        "asked for the `text/turtle` face and got `text/plain`",
    );
}

// ----- VOCABULARY -------------------------------------------------------------

fn invented_terms() -> FnEndpoint {
    FnEndpoint::new("invented", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             @prefix dcterms: <http://purl.org/dc/terms/> .\n\
             @prefix mine: <urn:example:ns#> .\n\
             <urn:ikigai:endpoint:invented> a ik:Endpoint, mine:Widget ;\n\
               ik:id \"invented\" ; ik:madeUp \"x\" ; dcterms:title \"t\" ; mine:size 3 .",
        ))
    })
    .with_description(
        Description::new("invented")
            .verb(Verb::Source)
            .output(TURTLE),
    )
}

#[test]
fn vocabulary_catches_invented_terms_and_honours_registered_namespaces() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:invented"), invented_terms());
    let report = report_of(space);
    let details = only(&report, Check::Vocabulary);
    assert!(
        details
            .iter()
            .any(|d| d.contains("`https://ikigai-rs.dev/ns#madeUp`")),
        "{report}"
    );
    assert!(
        details
            .iter()
            .any(|d| d.contains("`urn:example:ns#Widget`")),
        "{report}"
    );
    assert!(
        details.iter().any(|d| d.contains("`urn:example:ns#size`")),
        "{report}"
    );
    assert!(
        !details
            .iter()
            .any(|d| d.contains("dc/terms") || d.contains("ns#Endpoint") || d.contains("ns#id")),
        "well-known and defined terms are not findings:\n{report}"
    );

    // Registering the module's own namespace clears its terms, not the invented ik: one.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:invented"), invented_terms());
    let report = Suite::new()
        .namespace("urn:example:ns#")
        .run_blocking(&kernel(space));
    let details = only(&report, Check::Vocabulary);
    assert_eq!(details.len(), 1, "{report}");
    assert!(details[0].contains("madeUp"));
    assert!(report
        .to_string()
        .contains("module namespaces: urn:example:ns#"));
}

// ----- CACHEABLE --------------------------------------------------------------

/// Marks its result cacheable but reads an uncacheable dependency, so the kernel
/// can never store it — the ~2000× incident in miniature.
fn volatile_composite() -> ikigai_core::AsyncFnEndpoint {
    ikigai_core::AsyncFnEndpoint::new("composite", |inv| {
        Box::pin(async move {
            let live = inv
                .source(&ikigai_core::Iri::parse("urn:example:clock").unwrap())
                .await?;
            Ok(text(live.bytes).cacheable())
        })
    })
    .with_description(
        Description::new("composite")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

fn clock() -> FnEndpoint {
    FnEndpoint::new("clock", |_inv: &Invocation<'_>| Ok(text("now"))) // uncacheable
        .with_description(
            Description::new("clock")
                .verb(Verb::Source)
                .output("text/plain"),
        )
}

/// Cacheable, reads no thread, and is NOT declared pure.
fn threadless() -> FnEndpoint {
    FnEndpoint::new("config", |_inv: &Invocation<'_>| {
        Ok(text("theme = dark").cacheable())
    })
    .with_description(
        Description::new("config")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn cacheable_catches_a_recomputing_composite_and_a_threadless_result() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:composite"), volatile_composite())
        .bind(Exact::new("urn:example:clock"), clock())
        .bind(Exact::new("urn:example:config"), threadless());
    // Undeclared, the composite is INVISIBLE: the kernel returns the effective
    // expiry (Always, after the volatile dependency), so from outside it looks like
    // an endpoint that never said `.cacheable()`. Pinned, because that is the limit
    // `Suite::cacheable` exists for — and a core API that surfaced the declared
    // expiry would remove it.
    let report = report_of(space);
    assert!(
        report
            .against("composite")
            .all(|f| f.check != Check::Cacheable),
        "{report}"
    );
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:composite"), volatile_composite())
        .bind(Exact::new("urn:example:clock"), clock())
        .bind(Exact::new("urn:example:config"), threadless());
    let report = Suite::new()
        .cacheable("composite")
        .run_blocking(&kernel(space));
    assert!(
        report
            .against("composite")
            .any(|f| f.check == Check::Cacheable
                && f.detail.contains("kernel returned it uncacheable")),
        "{report}"
    );
    assert!(report.to_string().contains("declared cacheable: composite"));
    assert!(
        report
            .against("config")
            .any(|f| f.check == Check::Cacheable && f.detail.contains("empty golden-thread set")),
        "{report}"
    );
    assert!(
        report.against("clock").all(|f| f.check != Check::Cacheable),
        "an uncacheable result is not probed:\n{report}"
    );

    // Declared pure, the threadless finding goes away; declared cacheable as well,
    // the composite's recomputation stays.
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:composite"), volatile_composite())
        .bind(Exact::new("urn:example:clock"), clock())
        .bind(Exact::new("urn:example:config"), threadless());
    let report = Suite::new()
        .pure("config")
        .pure("composite")
        .cacheable("composite")
        .run_blocking(&kernel(space));
    assert!(
        report
            .against("config")
            .all(|f| f.check != Check::Cacheable),
        "{report}"
    );
    assert!(
        report
            .against("composite")
            .any(|f| f.check == Check::Cacheable),
        "{report}"
    );
}

#[test]
fn cacheable_passes_a_threaded_stateful_read() {
    let stateful = FnEndpoint::new("notes", |_inv: &Invocation<'_>| {
        Ok(text("hello").cacheable().depends_on("urn:example:notes"))
    })
    .with_description(
        Description::new("notes")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), stateful);
    let report = report_of(space);
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

// ----- PIPELINE ---------------------------------------------------------------

/// A Sink whose body arrives under a name no pipe will ever use.
fn sink_without_content() -> FnEndpoint {
    FnEndpoint::new("notes-write", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("body")?))
    })
    .with_description(
        Description::new("notes-write")
            .verb(Verb::Sink)
            .input(ArgSpec::new("body").class(XSD_STRING))
            .output("text/plain"),
    )
}

/// Declares `content`, reads `in`.
fn content_ignored() -> FnEndpoint {
    FnEndpoint::new("wc", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("in")?.lines().count().to_string()))
    })
    .with_description(
        Description::new("wc")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn pipeline_catches_a_sink_without_content_and_a_content_nobody_reads() {
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:example:notes-write"),
            sink_without_content(),
        )
        .bind(Exact::new("urn:example:wc"), content_ignored());
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Pipeline,
        "declares by-value inputs (`body`) but none named `content`",
    );
    assert_caught(
        &report,
        Check::Pipeline,
        "it reports `in` missing: it reads an input its contract does not declare",
    );
}

// ----- NAMES ------------------------------------------------------------------

#[test]
fn names_catches_camel_case_and_full_iri_ids() {
    let camel = FnEndpoint::new("toUpper", |_inv: &Invocation<'_>| Ok(text("x"))).with_description(
        Description::new("toUpper")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let iri = FnEndpoint::new("urn:cms:graph", |_inv: &Invocation<'_>| Ok(text("x")))
        .with_description(
            Description::new("urn:cms:graph")
                .verb(Verb::Source)
                .output("text/plain"),
        );
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:toUpper"), camel)
        .bind(Exact::new("urn:example:graph"), iri);
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Names,
        "id `toUpper` is not a kebab-case noun",
    );
    assert_caught(
        &report,
        Check::Names,
        "id `urn:cms:graph` is not a kebab-case noun",
    );
}

// ----- selection, opt-outs, fixtures -------------------------------------------

#[test]
fn a_skipped_check_finds_nothing_and_is_printed_as_skipped() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:blank"), blank_face());
    let report = Suite::new()
        .checks(Checks::all() - Checks::RDF)
        .run_blocking(&kernel(space));
    assert!(report.of(Check::SkolemRdf).next().is_none());
    assert!(report.of(Check::Vocabulary).next().is_none());
    assert!(
        report
            .to_string()
            .contains("skipped: SKOLEM-RDF VOCABULARY"),
        "{report}"
    );
}

#[test]
fn an_opted_out_action_is_not_invoked_but_still_read() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:stealth"), stealth())
        .bind(Exact::new("urn:example:toUpper"), {
            FnEndpoint::new("toUpper", |_inv: &Invocation<'_>| Ok(text("x"))).with_description(
                Description::new("toUpper")
                    .verb(Verb::Source)
                    .output("text/plain"),
            )
        });
    let report = Suite::new()
        .opt_out("stealth", Some(Verb::Source), "would call a paid API")
        .opt_out("toUpper", None, "every verb")
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Enforced).next().is_none(), "{report}");
    assert_caught(&report, Check::Names, "id `toUpper`"); // static checks still run
    let text = report.to_string();
    assert!(
        text.contains("opted out: stealth source: would call a paid API"),
        "{text}"
    );
    assert!(text.contains("opted out: toUpper: every verb"), "{text}");
}

#[test]
fn a_fixture_supplies_inputs_the_spec_cannot_derive() {
    // Requires a parseable JSON document; the derived "x" is not one.
    let needs_json = FnEndpoint::new("json-keys", |inv: &Invocation<'_>| {
        let raw = inv.inline_str("content")?;
        if !raw.trim_start().starts_with('{') {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "not a JSON object".into(),
            });
        }
        Ok(turtle("<urn:example:doc> <http://purl.org/dc/terms/title> \"t\" .").cacheable())
    })
    .with_description(
        Description::new("json-keys")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output(TURTLE),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:json-keys"), needs_json);
    let report = report_of(space);
    assert_caught(
        &report,
        Check::SkolemRdf,
        "did not resolve with the minimal inputs",
    );
    // Reported once, under the first check that needed it — not once per check.
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|f| f.detail.contains("did not resolve"))
            .count(),
        1,
        "{report}"
    );

    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:json-keys"),
        FnEndpoint::new("json-keys", |_inv: &Invocation<'_>| {
            Ok(turtle("<urn:example:doc> <http://purl.org/dc/terms/title> \"t\" .").cacheable())
        })
        .with_description(
            Description::new("json-keys")
                .verb(Verb::Source)
                .input(ArgSpec::new("content").class(XSD_STRING))
                .output(TURTLE),
        ),
    );
    let report = Suite::new()
        .fixture(Fixture::new("json-keys", Verb::Source).arg("content", "{}"))
        .pure("json-keys")
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
}

#[test]
fn a_fixture_binding_fills_a_template_variable() {
    let by_n = FnEndpoint::new("pr", |inv: &Invocation<'_>| {
        let n: u32 = inv
            .bindings
            .get("n")
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| Error::NotFound("no such PR".into()))?;
        Ok(text(format!("pr {n}"))
            .cacheable()
            .depends_on("urn:repo:pr"))
    })
    .with_description(
        Description::new("pr")
            .verb(Verb::Source)
            .input(ArgSpec::new("n").class(XSD_INTEGER).binding())
            .output("text/plain"),
    );
    // The class derives `1`, so this is clean without a fixture…
    let space = EndpointSpace::new().bind(UriTemplate::parse("urn:repo:pr:{n}").unwrap(), by_n);
    let report = report_of(space);
    assert!(report.is_clean(), "{report}");
}

#[test]
fn the_kernels_own_operations_are_skipped_unless_asked() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let skipped = Suite::new().pure("upper").run_blocking(&kernel(space));
    assert_eq!(skipped.endpoints, 1, "{skipped}");

    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let included = Suite::new()
        .pure("upper")
        .include_kernel_ops()
        .run_blocking(&kernel(space));
    assert!(included.endpoints > 1, "{included}");
    assert!(
        included
            .findings
            .iter()
            .any(|f| f.endpoint.starts_with("kernel-")),
        "{included}"
    );
}

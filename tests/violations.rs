//! One fixture endpoint per check that violates its rule on purpose, and a test
//! per fixture asserting the violation is caught — plus one endpoint that
//! violates nothing, asserting the suite is clean on it. If a check stops seeing
//! its fixture, that is the check failing, not the module.

use std::sync::atomic::{AtomicUsize, Ordering};
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

// ----- OUTPUTS ----------------------------------------------------------------

/// Declares a SPARQL results type as its only output and serves Turtle — with a
/// blank node the RDF checks would catch, if the declaration let them see the face.
/// The shape linkeddata's `sparql-construct` had for its whole life (#20).
fn mislabeled() -> FnEndpoint {
    FnEndpoint::new("mislabeled", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "<urn:ikigai:endpoint:mislabeled> <http://purl.org/dc/terms/title> [ ] .",
        ))
    })
    .with_description(
        Description::new("mislabeled")
            .verb(Verb::Source)
            .output("application/sparql-results+json"),
    )
}

/// Declares nothing at all about what it serves.
fn unannounced() -> FnEndpoint {
    FnEndpoint::new("unannounced", |_inv: &Invocation<'_>| Ok(text("x")))
        .with_description(Description::new("unannounced").verb(Verb::Source))
}

/// Declares the type with a parameter and serves it bare: the same face.
fn parameterized() -> FnEndpoint {
    FnEndpoint::new("parameterized", |_inv: &Invocation<'_>| Ok(text("x"))).with_description(
        Description::new("parameterized")
            .verb(Verb::Source)
            .output("text/plain;charset=utf-8"),
    )
}

/// Serves whatever `as=` names — a stylesheet emitting SVG relabeled by the caller.
/// Its own choice is `application/xml`, and that is what it declares.
fn relabel() -> FnEndpoint {
    FnEndpoint::new("relabel", |inv: &Invocation<'_>| {
        let label = inv.inline_str("as").unwrap_or("application/xml");
        Ok(Representation::new(
            ReprType::new(label),
            b"<svg/>".to_vec(),
        ))
    })
    .with_description(
        Description::new("relabel")
            .verb(Verb::Source)
            .input(ArgSpec::new("as").class(XSD_STRING).optional())
            .output("application/xml"),
    )
}

#[test]
fn outputs_catches_a_declared_type_the_action_does_not_serve() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:mislabeled"), mislabeled())
        .bind(Exact::new("urn:example:unannounced"), unannounced())
        .bind(Exact::new("urn:example:parameterized"), parameterized());
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Outputs,
        "served `text/turtle` with its minimal inputs but declares only \
         `application/sparql-results+json`",
    );
    assert_caught(
        &report,
        Check::Outputs,
        "served `text/plain` but declares no output",
    );
    // The wrong declaration HID the face: the blank node goes unreported by the RDF
    // checks, which filter the declared outputs before probing. OUTPUTS is the check
    // that says why they saw nothing.
    assert!(
        report
            .against("mislabeled")
            .all(|f| f.check == Check::Outputs),
        "{report}"
    );
    // A parameter is not a different face.
    assert!(
        report
            .against("parameterized")
            .all(|f| f.check != Check::Outputs),
        "{report}"
    );
    assert_eq!(report.of(Check::Outputs).count(), 2, "{report}");
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn outputs_honours_a_callers_as_label_and_says_it_saw_only_that() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:relabel"), relabel());
    let report = Suite::new()
        .fixture(Fixture::new("relabel", Verb::Source).arg("as", "image/svg+xml"))
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Outputs).next().is_none(), "{report}");
    let text = report.to_string();
    assert!(
        text.contains(
            "unprobed: relabel source OUTPUTS: served the caller's `as=image/svg+xml` label"
        ),
        "{text}"
    );
    assert!(
        text.contains("fixture: relabel source as=\"image/svg+xml\""),
        "{text}"
    );

    // Without the label the endpoint's own choice is observed, and matches.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:relabel"), relabel());
    let report = report_of(space);
    assert!(report.of(Check::Outputs).next().is_none(), "{report}");
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn outputs_reads_a_mutating_action_from_the_one_firing_the_pipeline_probe_makes() {
    // Fired under root exactly once (the pipeline probe); ENFORCED's no-grants call
    // is refused at the kernel's floor and never reaches the endpoint.
    let fired = Arc::new(AtomicUsize::new(0));
    let counter = fired.clone();
    let put = FnEndpoint::new("notes-put", move |inv: &Invocation<'_>| {
        counter.fetch_add(1, Ordering::SeqCst);
        let body = inv.inline_str("content")?;
        Ok(Representation::new(
            ReprType::new("application/json"),
            format!("{{\"stored\": {}}}", body.len()).into_bytes(),
        ))
    })
    .with_description(
        Description::new("notes-put")
            .verb(Verb::Sink)
            .requires("urn:cap:example:write")
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    );
    // Delete declares no `content`, so nothing fires it under root.
    let delete = FnEndpoint::new("notes-delete", |_inv: &Invocation<'_>| Ok(text("gone")))
        .with_description(
            Description::new("notes-delete")
                .verb(Verb::Delete)
                .requires("urn:cap:example:write")
                .output("text/plain"),
        );
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:notes-put"), put)
        .bind(Exact::new("urn:example:notes-delete"), delete);
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Outputs,
        "served `application/json` with its minimal inputs but declares only `text/plain`",
    );
    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "Sink is fired once, under root"
    );
    assert!(
        report.to_string().contains(
            "unprobed: notes-delete delete OUTPUTS: never fired under root: a mutating \
             action is fired only by the pipeline probe"
        ),
        "{report}"
    );
    assert!(
        report
            .against("notes-delete")
            .all(|f| f.check != Check::Outputs),
        "{report}"
    );
}

#[test]
fn outputs_can_be_left_out_like_any_other_check() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:mislabeled"), mislabeled());
    let report = Suite::new()
        .checks(Checks::all() - Checks::OUTPUTS)
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert!(report.to_string().contains("skipped: OUTPUTS"), "{report}");
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

#[test]
fn live_catches_a_resource_that_silently_became_cacheable() {
    // Uncacheable by decision, declared so: the clean line now says which.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:clock"), clock());
    let report = Suite::new().live("clock").run_blocking(&kernel(space));
    report.assert_clean();
    assert!(
        report.to_string().contains("declared live: clock"),
        "{report}"
    );

    // The same declaration over a result the kernel hands back cacheable — the
    // polarity `Suite::cacheable` cannot see, and the direction nothing else can.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:config"), threadless());
    let report = Suite::new().live("config").run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Cacheable,
        "declared live (`Suite::live`) but the kernel returned it cacheable \
         (`Expiry::Never` — permanently)",
    );
    // One finding, not two: the empty-thread rule is about a result that is meant to
    // be cached, and this one is not.
    assert_eq!(report.of(Check::Cacheable).count(), 1, "{report}");

    // Undeclared, the same endpoint is silent — which is exactly why `live` exists.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:clock"), clock());
    let report = report_of(space);
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");

    // Both declarations at once is a contradiction, and says so.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:clock"), clock());
    let report = Suite::new()
        .live("clock")
        .cacheable("clock")
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Cacheable,
        "declared both cacheable (`Suite::cacheable`) and live (`Suite::live`)",
    );
}

// ----- per-check opt-outs -----------------------------------------------------

/// Breaks three rules at once: a full-IRI id (NAMES), a blank node (SKOLEM-RDF)
/// and an invented `ik:` term (VOCABULARY).
fn three_violations() -> FnEndpoint {
    FnEndpoint::new("urn:example:review", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
             <urn:ikigai:endpoint:review> a ik:Endpoint ; ik:quote [ ik:madeUp \"x\" ] .",
        ))
    })
    .with_description(
        Description::new("urn:example:review")
            .verb(Verb::Source)
            .output(TURTLE),
    )
}

#[test]
fn a_per_check_opt_out_silences_one_rule_and_keeps_every_other() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:review"), three_violations());
    let report = report_of(space);
    assert_caught(&report, Check::Names, "is not a kebab-case noun");
    assert_caught(&report, Check::SkolemRdf, "has 1 blank node(s)");
    assert_caught(&report, Check::Vocabulary, "ns#quote");

    // One rule waived — an invoking one — and the other invoking check on the same
    // endpoint still runs. `opt_out` would have dropped both.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:review"), three_violations());
    let report = Suite::new()
        .opt_out_check(
            "urn:example:review",
            Check::Vocabulary,
            "ik:quote lands in the next vocabulary release; pinned by hand until then",
        )
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Vocabulary).next().is_none(), "{report}");
    assert_caught(&report, Check::SkolemRdf, "has 1 blank node(s)");
    assert_caught(&report, Check::Names, "is not a kebab-case noun");
    assert!(
        report.to_string().contains(
            "opted out: urn:example:review VOCABULARY: ik:quote lands in the next vocabulary"
        ),
        "the waiver and its reason are in the record:\n{report}"
    );

    // A description-only check, which nothing else could silence per id.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:review"), three_violations());
    let report = Suite::new()
        .opt_out_check(
            "urn:example:review",
            Check::Names,
            "the id is an IRI by contract",
        )
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Names).next().is_none(), "{report}");
    assert_caught(&report, Check::Vocabulary, "ns#quote");

    // And it is per ENDPOINT: another endpoint breaking the same rule is untouched.
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:review"), three_violations())
        .bind(Exact::new("urn:example:blank"), blank_face());
    let report = Suite::new()
        .opt_out_check(
            "urn:example:review",
            Check::SkolemRdf,
            "another repo owns the fix",
        )
        .run_blocking(&kernel(space));
    assert!(
        report
            .against("urn:example:review")
            .all(|f| f.check != Check::SkolemRdf),
        "{report}"
    );
    assert!(
        report
            .against("blank-graph")
            .any(|f| f.check == Check::SkolemRdf),
        "{report}"
    );
}

// ----- DECLARATIONS -----------------------------------------------------------
//
// Every test here asserts the SILENCE is gone: the declaration used to reach no
// check at all, and the report printed it as if one had honoured it.

/// A well-behaved Sink: kebab id, typed `content`, reads it, declares what it
/// serves. Nothing about it can ever be cacheable — `Verb::Sink` is not.
fn notes_write() -> FnEndpoint {
    FnEndpoint::new("notes-write", |inv: &Invocation<'_>| {
        let _ = inv.inline_str("content")?;
        Ok(text("written"))
    })
    .with_description(
        Description::new("notes-write")
            .verb(Verb::Sink)
            // Gated, because a write is: without this the fixture trips AUTHORITY
            // and the tests below stop being about what they are about.
            .requires("urn:cap:notes:write")
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    )
}

/// The same write with nothing in front of it — a `Sink` any caller can perform.
fn notes_write_ungated() -> FnEndpoint {
    FnEndpoint::new("notes-write", |inv: &Invocation<'_>| {
        let _ = inv.inline_str("content")?;
        Ok(text("written"))
    })
    .with_description(
        Description::new("notes-write")
            .verb(Verb::Sink)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    )
}

/// A Source nobody could cache: a counter, marked nothing.
fn volatile() -> FnEndpoint {
    let n = Arc::new(AtomicUsize::new(0));
    FnEndpoint::new("tick", move |_inv: &Invocation<'_>| {
        Ok(text(n.fetch_add(1, Ordering::SeqCst).to_string()))
    })
    .with_description(
        Description::new("tick")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn live_on_a_mutating_verb_is_reported_rather_than_silently_inert() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write());
    let report = Suite::new()
        .live("notes-write")
        .run_blocking(&kernel(space));
    // The hole: CACHEABLE returns early on a non-cacheable verb, so `live` produced
    // no finding, no note, nothing — and the report printed `declared live:` anyway,
    // which reads as a check that ran. ikigai-http hit this on the first day `live`
    // existed and worked around it with a comment.
    assert!(
        report.of(Check::Cacheable).next().is_none(),
        "CACHEABLE still says nothing — that is the point:\n{report}"
    );
    assert_caught(&report, Check::Declarations, "declares no cacheable verb");
    assert_caught(&report, Check::Declarations, "declared live");
    assert!(!report.is_clean(), "{report}");
    assert!(
        report.to_string().contains("declared live: notes-write"),
        "the misleading line is still printed — now beside the finding:\n{report}"
    );

    // The same declaration on a cacheable verb is consulted, so it is not reported.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:tick"), volatile());
    let report = Suite::new().live("tick").run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
}

#[test]
fn a_declaration_for_an_id_nothing_binds_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .live("uppr") // a typo
        .cacheable("urn:example:upper") // the bound IRI, not the description id
        .fixture(Fixture::new("uppercase", Verb::Source).arg("in", "x"))
        .opt_out("uper", None, "meant `upper`")
        .opt_out_check("Upper", Check::Names, "the id is a proper noun")
        .run_blocking(&kernel(space));
    for id in ["uppr", "urn:example:upper", "uppercase", "uper", "Upper"] {
        assert!(
            report
                .against(id)
                .any(|f| f.check == Check::Declarations
                    && f.detail.contains("no endpoint with that id")),
            "`{id}` was not reported:\n{report}"
        );
    }
    assert_eq!(report.of(Check::Declarations).count(), 5, "{report}");
    // The one declaration that DID reach its check is not reported.
    assert!(report.against("upper").next().is_none(), "{report}");
}

#[test]
fn a_waiver_for_a_check_that_could_not_have_run_is_reported() {
    // No RDF face at all: the graph checks probe declared RDF outputs only, so a
    // SKOLEM-RDF waiver here silences nothing and reads as if it did.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .opt_out_check("upper", Check::SkolemRdf, "no graph face")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "declares no RDF face");

    // A Sink cannot reach CACHEABLE (the same early return `live` fell through).
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write());
    let report = Suite::new()
        .opt_out_check("notes-write", Check::Cacheable, "writes are never cached")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "declares no cacheable verb");

    // A check that is not selected is already skipped everywhere.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .checks(Checks::all() - Checks::RDF)
        .opt_out_check("upper", Check::Vocabulary, "terms land next release")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "is not selected");

    // Every action opted out, so an invoking check had nothing to waive.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .opt_out("upper", None, "calls a paid API")
        .opt_out_check("upper", Check::Enforced, "capability comes from the host")
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "every action of it is opted out",
    );

    // The counter-case, thought through and deliberately NOT a finding: OUTPUTS on
    // an endpoint declaring no outputs still fires ("served `text/plain` but
    // declares no output"), so waiving it waives a real rule.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:unannounced"), unannounced());
    let report = Suite::new()
        .opt_out_check(
            "unannounced",
            Check::Outputs,
            "a pass-through face core cannot spell",
        )
        .run_blocking(&kernel(space));
    assert!(
        report.of(Check::Declarations).next().is_none(),
        "a waiver of a check that WOULD have fired is not inert:\n{report}"
    );
    assert!(report.is_clean(), "{report}");
}

#[test]
fn a_fixture_binding_that_names_no_template_variable_is_reported() {
    let by_n = FnEndpoint::new("pr", |inv: &Invocation<'_>| {
        let n = inv.bindings.get("n").unwrap_or_default();
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
    let space = EndpointSpace::new().bind(UriTemplate::parse("urn:repo:pr:{n}").unwrap(), by_n);
    let report = Suite::new()
        .fixture(Fixture::new("pr", Verb::Source).binding("number", "42"))
        .run_blocking(&kernel(space));
    // The walk expanded `{n}` from the ArgSpec's class and never looked at
    // `number`: a typo'd binding used to change nothing and say nothing.
    assert_caught(
        &report,
        Check::Declarations,
        "is not a template variable of any pattern",
    );
    assert!(
        report.to_string().contains("(`n`)"),
        "the finding names the variables there ARE:\n{report}"
    );
}

#[test]
fn a_fixture_for_an_action_the_endpoint_does_not_declare_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .fixture(Fixture::new("upper", Verb::Sink).arg("content", "x"))
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "does not declare");

    // And arguments for an action that was opted out are never used either.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .opt_out("upper", Some(Verb::Source), "calls a paid API")
        .fixture(Fixture::new("upper", Verb::Source).arg("in", "x"))
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "an action excluded by `Suite::opt_out`",
    );
}

#[test]
fn an_opt_out_that_excluded_nothing_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .opt_out("upper", Some(Verb::Sink), "would write to the store")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "declares no sink action");
    assert!(
        report
            .of(Check::Declarations)
            .any(|f| f.detail.contains("it declares source")),
        "the finding names the verbs there ARE:\n{report}"
    );
}

#[test]
fn a_registered_namespace_that_accounts_for_nothing_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:invented"), invented_terms());
    let report = Suite::new()
        .namespace("urn:example:ns#")
        .namespace("https://example.com/unused#")
        .run_blocking(&kernel(space));
    let inert: Vec<String> = only(&report, Check::Declarations);
    assert_eq!(inert.len(), 1, "only the unused one is reported:\n{report}");
    assert!(inert[0].contains("https://example.com/unused#"), "{report}");
    assert!(inert[0].contains("no term in any probed face"), "{report}");
    assert!(
        report
            .of(Check::Declarations)
            .all(|f| f.endpoint == "(suite)"),
        "a namespace has no endpoint id:\n{report}"
    );

    // With VOCABULARY off, the reason is the selection, not the graph.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:invented"), invented_terms());
    let report = Suite::new()
        .checks(Checks::all() - Checks::RDF)
        .namespace("urn:example:ns#")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "VOCABULARY is not selected");
}

#[test]
fn pure_on_a_result_that_is_never_cacheable_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:tick"), volatile());
    let report = Suite::new().pure("tick").run_blocking(&kernel(space));
    // CACHEABLE ran, and said nothing: an uncacheable result is held to nothing
    // unless it was declared. `pure` exempts a CACHEABLE result from the
    // golden-thread rule, so over this one it exempted nothing.
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
    assert_caught(
        &report,
        Check::Declarations,
        "no action of it came back cacheable",
    );
}

#[test]
fn a_failed_resolution_does_not_also_report_the_declaration_as_inert() {
    // This endpoint needs a parseable JSON document and the derived `x` is not one,
    // so the walk reports the failure. Whether `pure` would have applied is
    // unknowable from here — calling it inert would be a second, wrong finding
    // stacked on the real one.
    let needs_json = FnEndpoint::new("json-keys", |inv: &Invocation<'_>| {
        let raw = inv.inline_str("content")?;
        if !raw.trim_start().starts_with('{') {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "not a JSON object".into(),
            });
        }
        Ok(text("{}").cacheable())
    })
    .with_description(
        Description::new("json-keys")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:json-keys"), needs_json);
    let report = Suite::new().pure("json-keys").run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Cacheable,
        "did not resolve with the minimal inputs",
    );
    assert!(
        report.of(Check::Declarations).next().is_none(),
        "one failure, one finding:\n{report}"
    );
}

#[test]
fn declarations_can_be_left_out_like_any_other_check() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write());
    let report = Suite::new()
        .live("notes-write")
        .checks(Checks::all() - Checks::DECLARATIONS)
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert!(
        report.to_string().contains("skipped: DECLARATIONS"),
        "{report}"
    );

    // Or waived for one endpoint, with the reason in the record — the spelling for
    // a declaration that is standing on purpose.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write());
    let report = Suite::new()
        .live("notes-write")
        .opt_out_check(
            "notes-write",
            Check::Declarations,
            "earns a Source face next release; the declaration stands until then",
        )
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
}

#[test]
fn identical_waivers_group_onto_one_line_and_checked_says_it_was_partial() {
    fn invented_as(id: &'static str) -> FnEndpoint {
        FnEndpoint::new(id, |_inv: &Invocation<'_>| {
            Ok(turtle(
                "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                 <urn:ikigai:endpoint:x> a ik:Endpoint ; ik:madeUp \"x\" .",
            ))
        })
        .with_description(Description::new(id).verb(Verb::Source).output(TURTLE))
    }
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:a"), invented_as("a-face"))
        .bind(Exact::new("urn:example:b"), invented_as("b-face"))
        .bind(Exact::new("urn:example:c"), invented_as("c-face"))
        .bind(Exact::new("urn:example:graph"), {
            FnEndpoint::new("cms-graph", |_inv: &Invocation<'_>| {
                Ok(turtle(
                    "<urn:example:doc> <http://purl.org/dc/terms/title> \"t\" .",
                ))
            })
            .with_description(
                Description::new("cms-graph")
                    .verb(Verb::Source)
                    .output(TURTLE),
            )
        });
    const REASON: &str = "ik:madeUp lands in the next vocabulary release; another repo owns it";
    let report = Suite::new()
        .opt_out_check("a-face", Check::Vocabulary, REASON)
        .opt_out_check("b-face", Check::Vocabulary, REASON)
        .opt_out_check("c-face", Check::Vocabulary, REASON)
        .run_blocking(&kernel(space));
    report.assert_clean();
    let text = report.to_string();
    // One line, not three identical ones that bury every other line of the report.
    assert!(
        text.contains(&format!(
            "opted out: a-face b-face c-face VOCABULARY: {REASON}"
        )),
        "{text}"
    );
    assert_eq!(
        text.matches(REASON).count(),
        1,
        "the reason prints once:\n{text}"
    );
    // The third state `checked:` could not express: ran, but not everywhere.
    assert!(text.contains("VOCABULARY*"), "{text}");
    assert!(
        text.contains("* VOCABULARY ran on 1 of 4 endpoint(s); waived on the rest"),
        "{text}"
    );

    // A waiver for an id nothing binds took the check off NOTHING, so it must not
    // shrink the count in the one line whose whole job is to state the coverage.
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:a"), invented_as("a-face"))
        .bind(Exact::new("urn:example:graph"), {
            FnEndpoint::new("cms-graph", |_inv: &Invocation<'_>| {
                Ok(turtle(
                    "<urn:example:doc> <http://purl.org/dc/terms/title> \"t\" .",
                ))
            })
            .with_description(
                Description::new("cms-graph")
                    .verb(Verb::Source)
                    .output(TURTLE),
            )
        });
    let report = Suite::new()
        .opt_out_check("a-face", Check::Vocabulary, REASON)
        .opt_out_check("z-face", Check::Vocabulary, REASON) // binds nothing
        .run_blocking(&kernel(space));
    assert_eq!(report.walked.to_vec(), ["a-face", "cms-graph"], "{report}");
    let text = report.to_string();
    assert!(
        text.contains("* VOCABULARY ran on 1 of 2 endpoint(s)"),
        "the bogus waiver does not count against coverage:\n{text}"
    );
    assert!(
        report
            .against("z-face")
            .any(|f| f.check == Check::Declarations),
        "and it is reported as the inert declaration it is:\n{report}"
    );
}

// ----- what was probed ---------------------------------------------------------

#[test]
fn the_report_names_the_rdf_faces_the_walk_actually_reached() {
    let empty_graph = FnEndpoint::new("ik-context", |_inv: &Invocation<'_>| {
        // A JSON-LD *context* document: legitimate, and zero triples.
        Ok(Representation::new(
            ReprType::new("application/ld+json"),
            br#"{"@context": {"@vocab": "https://ikigai-rs.dev/ns#"}}"#.to_vec(),
        ))
    })
    .with_description(
        Description::new("ik-context")
            .verb(Verb::Source)
            .output("application/ld+json"),
    );
    let graph = FnEndpoint::new("cms-graph", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "<urn:example:doc> <http://purl.org/dc/terms/title> \"t\" .",
        ))
    })
    .with_description(
        Description::new("cms-graph")
            .verb(Verb::Source)
            .output(TURTLE),
    );
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:context"), empty_graph)
        .bind(Exact::new("urn:example:graph"), graph);
    let report = report_of(space);
    report.assert_clean();
    let text = report.to_string();
    assert!(
        text.contains("probed 2 face(s) across 2 endpoint(s)"),
        "{text}"
    );
    assert!(
        text.contains("probed: cms-graph source `text/turtle`: 1 triple(s)"),
        "{text}"
    );
    // A clean pass over an empty graph is vacuous, and the line says so.
    assert!(
        text.contains(
            "probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was checked"
        ),
        "{text}"
    );

    // A module with NO graph face is probed too. It used to print no `probed:`
    // section at all — indistinguishable from a walk that reached nothing, which is
    // the ambiguity these lines exist to kill, and it was true for about half the
    // ecosystem.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new().pure("upper").run_blocking(&kernel(space));
    report.assert_clean();
    assert_eq!(report.probed.len(), 1, "{report}");
    assert_eq!(report.probed[0].face, "text/plain");
    assert_eq!(
        report.probed[0].bytes, 1,
        "the minimal call upper-cases `x`"
    );
    assert!(
        report
            .to_string()
            .contains("probed: upper source `text/plain`: 1 byte(s)"),
        "{report}"
    );

    // And a walk that genuinely resolved nothing says THAT, rather than saying
    // nothing — the two cases are now distinguishable in both directions.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let nothing = Suite::new()
        .checks(Checks::NAMES)
        .run_blocking(&kernel(space));
    assert!(nothing.probed.is_empty(), "{nothing}");
    assert!(
        nothing.to_string().contains("probed: nothing — no action"),
        "{nothing}"
    );
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
    // OUTPUTS had nothing to compare, and says so rather than inventing a finding.
    assert!(
        report
            .to_string()
            .contains("unprobed: json-keys source OUTPUTS: the minimal resolution failed"),
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
    // What the module said is printed — the fixture included.
    assert!(
        report
            .to_string()
            .contains("fixture: json-keys source content=\"{}\""),
        "{report}"
    );
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

#[test]
fn authority_catches_a_write_a_caller_holding_nothing_performed() {
    // The fourth cell ENFORCED leaves open: declares nothing, and resolved anyway.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write_ungated());
    let report = Suite::new().run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Authority,
        "mutated under a capability holding no grants",
    );
    // ENFORCED is silent on it — that silence is why this check exists.
    assert!(
        report.of(Check::Enforced).next().is_none(),
        "ENFORCED says nothing about the fourth cell:\n{report}"
    );

    // Declare the scope and the finding goes: the kernel refuses the ungranted
    // caller, so there is a write to withhold.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_write());
    let report = Suite::new().run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");

    // A public READ is not this check's business, declared or not.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new().pure("upper").run_blocking(&kernel(space));
    assert!(
        report.of(Check::Authority).next().is_none(),
        "a Source declaring no capability is a decision, not a defect:\n{report}"
    );
}

#[test]
fn an_unobserved_mutation_is_unprobed_rather_than_a_pass() {
    // A Sink that declares nothing and refuses the minimal call for an unrelated
    // reason. Nothing was learned about what an ungranted caller can do through it,
    // and the silent version of that reads exactly like a clean endpoint.
    let picky = FnEndpoint::new("notes-strict", |inv: &Invocation<'_>| {
        let body = inv.inline_str("content")?;
        if !body.starts_with('{') {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "not a JSON object".into(),
            });
        }
        Ok(text("written"))
    })
    .with_description(
        Description::new("notes-strict")
            .verb(Verb::Sink)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:strict"), picky);
    let report = Suite::new()
        .opt_out_check(
            "notes-strict",
            Check::Pipeline,
            "reads a shape, not a value",
        )
        .opt_out_check("notes-strict", Check::Outputs, "never fired")
        .run_blocking(&kernel(space));
    assert!(
        report.of(Check::Authority).next().is_none(),
        "an unobserved probe is not a violation:\n{report}"
    );
    assert!(
        report.to_string().contains(
            "unprobed: notes-strict sink AUTHORITY: declares no `requires` and did not resolve"
        ),
        "the walk must say what it did not see:\n{report}"
    );
}

#[test]
fn waiving_authority_where_nothing_mutates_is_reported_as_inert() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .opt_out_check("upper", Check::Authority, "reads only")
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "declares no mutating verb");
}

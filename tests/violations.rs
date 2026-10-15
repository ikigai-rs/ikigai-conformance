//! One fixture endpoint per check that violates its rule on purpose, and a test
//! per fixture asserting the violation is caught — plus one endpoint that
//! violates nothing, asserting the suite is clean on it. If a check stops seeing
//! its fixture, that is the check failing, not the module.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ikigai_conformance::{check, Check, Checks, Finding, Fixture, Inherited, Report, Suite};
use ikigai_core::{
    space_iri, ActionSpec, ArgRef, ArgSpec, Capability, Description, Endpoint, EndpointSpace,
    Error, Exact, Expiry, FixedClock, FnEndpoint, Invocation, Iri, Kernel, ReprType,
    Representation, Request, Resolution, Result, Scope, Space, SpaceEntry, SpaceKind, Thread, Time,
    Topology, UriTemplate, Verb,
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
fn requires_without_a_verb_is_caught_statically_and_is_enforced_on_every_verb() {
    let verbless = FnEndpoint::new("link-remove", |_inv: &Invocation<'_>| Ok(text("removed")))
        .with_description(
            Description::new("link-remove")
                .summary("Strike one URL (Sink executes)")
                .requires("urn:cap:fs:write:*"), // no .verb()
        );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:link-remove"), verbless);
    let k = kernel(space);
    let report = check(&k).unwrap_err();
    assert_caught(
        &report,
        Check::RequiresVerb,
        "declares requires `urn:cap:fs:write:*` but no verb",
    );
    // Since ikigai-core 0.1.85 (ledger #750, A3) a verbless requires gates EVERY verb, so
    // the finding is about legibility, not enforcement: the kernel refuses the Sink under no
    // grants, but the catalog cannot say which actions the scope gates.
    let none = ikigai_core::Capability::scoped(Vec::<String>::new());
    let request = ikigai_core::Request::new(
        Verb::Sink,
        ikigai_core::Iri::parse("urn:example:link-remove").unwrap(),
    );
    let ran = futures::executor::block_on(k.issue(request, &none));
    assert!(
        matches!(ran, Err(ikigai_core::Error::Denied(_))),
        "a verbless requires must be enforced on every verb: {ran:?}"
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

/// A typed `Denied` from an action declaring nothing may be a STATE gate rather than an
/// undeclared capability (`urn:passkey:register` refuses until an enrollment window is
/// open), and the two have different fixes. The finding names both, so it no longer
/// sends a reader hunting for a capability that does not exist (ledger #362).
#[test]
fn enforced_names_both_an_undeclared_capability_and_a_state_gate() {
    let window = FnEndpoint::new("register", |_inv: &Invocation<'_>| {
        Err(Error::Denied("no enrollment window is open".into()))
    })
    .with_description(
        Description::new("register")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:register"), window));
    let enforced = only(&report, Check::Enforced);
    assert_eq!(enforced.len(), 1, "{report}");
    for needle in [
        "declares no capability but refused with `Denied` under no grants (no enrollment window is open)",
        "The gate is a capability or a state, and the fixes differ",
        "a capability it enforces is declared (`.requires(\"urn:cap:…\")` on the action)",
        "a state no capability expresses",
        "`Suite::opt_out_check(id, Check::Enforced, …)`",
    ] {
        assert!(enforced[0].contains(needle), "{needle:?}:\n{report}");
    }
    assert!(!enforced[0].contains("an enforced scope"), "{report}");
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
fn outputs_honors_a_callers_as_label_and_says_it_saw_only_that() {
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
fn vocabulary_catches_invented_terms_and_honors_registered_namespaces() {
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

/// Ledger #343: the W3C Web Annotation vocabulary (`oa:`, what ikigai-browse's
/// annotation overlay speaks) and Dublin Core Elements 1.1 (`dc:`, what a Zotero
/// export speaks) are published standards, not invented terms. One seeded annotation
/// used to produce a VOCABULARY finding per term.
#[test]
fn vocabulary_knows_web_annotation_and_dublin_core_elements() {
    let annotations = FnEndpoint::new("annotations", |_inv: &Invocation<'_>| {
        Ok(turtle(
            "@prefix oa: <http://www.w3.org/ns/oa#> .\n\
             @prefix dc: <http://purl.org/dc/elements/1.1/> .\n\
             <urn:example:anno:1> a oa:Annotation ; oa:motivatedBy oa:commenting ;\n\
               oa:bodyValue \"a note\" ; oa:hasTarget <urn:example:anno:1:target> .\n\
             <urn:example:anno:1:target> oa:hasSelector <urn:example:anno:1:sel> .\n\
             <urn:example:anno:1:sel> a oa:TextQuoteSelector ; oa:exact \"x\" .\n\
             <urn:example:doc> dc:subject \"rdf\" ; dc:creator \"someone\" .",
        )
        .cacheable()
        .depends_on("urn:example:store"))
    })
    .with_description(
        Description::new("annotations")
            .verb(Verb::Source)
            .output(TURTLE),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:annotations"), annotations);
    let report = report_of(space);
    assert_eq!(
        only(&report, Check::Vocabulary),
        Vec::<String>::new(),
        "{report}"
    );
    assert!(report.is_clean(), "{report}");
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
        report.against("config").any(|f| f.check == Check::Cacheable
            && f.detail.contains("no golden thread but its own name")),
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
    // The thread names the STATE (`notes-store`), not the endpoint: a thread named
    // for the endpoint itself is the one the kernel adds anyway, and reads as no
    // declaration at all (`cacheable_reads_a_thread_named_for_the_endpoint_as_none`).
    let stateful = FnEndpoint::new("notes", |_inv: &Invocation<'_>| {
        Ok(text("hello")
            .cacheable()
            .depends_on("urn:example:notes-store"))
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

/// A read/write resource over one piece of state: `Source` serves it cacheable and
/// declares no thread, `Sink` replaces it. Correct since core 0.1.73 with no
/// `depends_on` at all: the kernel hangs the cached read on its own name, and the
/// `Sink` through that name cuts it.
fn notes_read_write(state: Arc<std::sync::Mutex<String>>) -> FnEndpoint {
    FnEndpoint::new("notes", move |inv: &Invocation<'_>| {
        match inv.request.verb {
            Verb::Sink => {
                *state.lock().unwrap() = inv.inline_str("content")?.to_string();
                Ok(text("written"))
            }
            _ => Ok(text(state.lock().unwrap().clone()).cacheable()),
        }
    })
    .with_description(
        Description::new("notes")
            .verb(Verb::Source)
            .action(
                ActionSpec::new(Verb::Sink)
                    .requires("urn:cap:notes:write")
                    .input(ArgSpec::new("content").class(XSD_STRING))
                    .output("text/plain"),
            )
            .output("text/plain"),
    )
}

/// The premise the purity rule now rests on, pinned against the core this crate
/// resolves: an empty thread set is UNOBSERVABLE on a cacheable read (core 0.1.73,
/// ledger #512 hole A), so the rule is "no thread but its own name" (ledger #549).
/// Before 0.1.73 the first assertion fails — and the rule spelled `is_empty()`
/// was right; after it, that spelling fires for nothing.
#[test]
fn the_kernel_hangs_every_cacheable_read_on_its_own_name() {
    let state = Arc::new(std::sync::Mutex::new("first".to_string()));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:config"), threadless())
        .bind(Exact::new("urn:example:notes"), notes_read_write(state));
    let kernel = kernel(space);
    let read = |iri: &str| Request::new(Verb::Source, Iri::parse(iri).unwrap());
    let root = Capability::root();

    let config =
        futures::executor::block_on(kernel.issue(read("urn:example:config"), &root)).unwrap();
    assert_eq!(
        config.threads().iter().collect::<Vec<_>>(),
        [&Thread::new("urn:example:config")],
        "a cacheable read declaring no thread carries exactly its own name"
    );

    // The exemption for a written resource: a Sink through the same name cuts it.
    let first =
        futures::executor::block_on(kernel.issue(read("urn:example:notes"), &root)).unwrap();
    assert_eq!(first.bytes, b"first");
    assert!(kernel.is_cached(&read("urn:example:notes"), &root));
    let write = Request::new(Verb::Sink, Iri::parse("urn:example:notes").unwrap())
        .with_arg("content", ArgRef::Inline(b"second".to_vec()));
    futures::executor::block_on(kernel.issue(write, &root)).unwrap();
    assert!(
        !kernel.is_cached(&read("urn:example:notes"), &root),
        "a Sink through the same name invalidates the cached read"
    );
    let second =
        futures::executor::block_on(kernel.issue(read("urn:example:notes"), &root)).unwrap();
    assert_eq!(second.bytes, b"second");
}

#[test]
fn cacheable_passes_a_read_written_through_its_own_name() {
    let state = Arc::new(std::sync::Mutex::new("hello".to_string()));
    let space = EndpointSpace::new().bind(Exact::new("urn:example:notes"), notes_read_write(state));
    let report = report_of(space);
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

/// Cacheable and hung from a piece of state no request names: the thread a
/// stateful read SHOULD carry, which makes a `pure` declaration on it false.
fn lookup() -> FnEndpoint {
    FnEndpoint::new("lookup", |_inv: &Invocation<'_>| {
        Ok(text("row 7").cacheable().depends_on("urn:example:db"))
    })
    .with_description(
        Description::new("lookup")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn pure_with_a_foreign_thread_is_reported() {
    // Ledger #1508: `pure` used to be taken on trust. A foreign thread is exactly
    // what silences the rule `pure` exempts from, so a false declaration passed.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:lookup"), lookup());
    let report = Suite::new().pure("lookup").run_blocking(&kernel(space));
    let found = only(&report, Check::Cacheable);
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains(
            "declared pure (`Suite::pure`) but carries a foreign thread `urn:example:db`"
        ),
        "{report}"
    );
    // The declaration WAS consulted — the claim was checked — so DECLARATIONS does
    // not also call it inert.
    assert!(report.of(Check::Declarations).next().is_none(), "{report}");

    // Additive: the same endpoint, undeclared, is the threaded stateful read the
    // rule asks for and stays clean.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:lookup"), lookup());
    assert!(report_of(space).is_clean());
}

#[test]
fn pure_names_every_foreign_thread_in_one_finding() {
    let two = FnEndpoint::new("join", |_inv: &Invocation<'_>| {
        Ok(text("x")
            .cacheable()
            .depends_on("urn:example:db")
            .depends_on("urn:example:cache"))
    })
    .with_description(
        Description::new("join")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:join"), two);
    let report = Suite::new().pure("join").run_blocking(&kernel(space));
    assert_eq!(
        only(&report, Check::Cacheable),
        [
            "declared pure (`Suite::pure`) but carries foreign threads `urn:example:cache`, \
          `urn:example:db`: neither its own name nor anything it read, so the answer hangs \
          from state that is not one of its inputs, and a pure function reads none. Drop the \
          `pure` declaration (the thread already answers \"what cuts this?\"), or drop the \
          dependency if the endpoint really is a function of its inputs"
        ],
        "{report}"
    );
}

/// A pure composite: it reads `source` (a resource another endpoint serves) and
/// transforms it. The kernel folds what it read into its answer — the input's
/// own-name thread and every thread the input carries — and those threads are the
/// input's, not state of the composite's.
fn shout(source: &'static str, extra: Option<&'static str>) -> ikigai_core::AsyncFnEndpoint {
    ikigai_core::AsyncFnEndpoint::new("shout", move |inv| {
        Box::pin(async move {
            let read = inv.source(&Iri::parse(source).unwrap()).await?;
            let out = text(String::from_utf8_lossy(&read.bytes).to_uppercase()).cacheable();
            Ok(match extra {
                Some(thread) => out.depends_on(thread),
                None => out,
            })
        })
    })
    .with_description(
        Description::new("shout")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn pure_accepts_the_threads_of_what_it_read() {
    let space = || {
        EndpointSpace::new()
            .bind(
                Exact::new("urn:example:shout"),
                shout("urn:example:lookup", None),
            )
            .bind(Exact::new("urn:example:lookup"), lookup())
    };
    // The premise, pinned: the composite's answer carries its input's own-name
    // thread AND the thread its input hangs from, neither of them its own.
    let read = Request::new(Verb::Source, Iri::parse("urn:example:shout").unwrap());
    let premise = kernel(space());
    let answer = futures::executor::block_on(premise.issue(read, &Capability::root())).unwrap();
    for thread in ["urn:example:lookup", "urn:example:db"] {
        assert!(
            answer.threads().contains(&Thread::new(thread)),
            "{:?}",
            answer.threads()
        );
    }
    let report = Suite::new().pure("shout").run_blocking(&kernel(space()));
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

#[test]
fn pure_accepts_the_bindings_thread_of_a_kernel_face_it_read() {
    // A kernel face (the topology, the catalog) rendered another way is a function of
    // what is bound: it hangs from `urn:kernel:bindings`, which is the kernel face's
    // thread, not state of the composite's (ikigai-web-demo's `catalog-rdf`).
    let space = || {
        EndpointSpace::new().bind(
            Exact::new("urn:example:shout"),
            shout("urn:kernel:topology", None),
        )
    };
    let read = Request::new(Verb::Source, Iri::parse("urn:example:shout").unwrap());
    let premise = kernel(space());
    let answer = futures::executor::block_on(premise.issue(read, &Capability::root())).unwrap();
    assert!(
        answer
            .threads()
            .contains(&Thread::new(ikigai_core::BINDINGS_THREAD)),
        "{:?}",
        answer.threads()
    );
    let report = Suite::new().pure("shout").run_blocking(&kernel(space()));
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

#[test]
fn pure_reports_a_foreign_thread_beside_the_threads_of_what_it_read() {
    // What it read accounts for its input's threads and nothing else: a thread the
    // composite names itself is still foreign.
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:example:shout"),
            shout("urn:example:config", Some("urn:example:session")),
        )
        .bind(Exact::new("urn:example:config"), threadless());
    let report = Suite::new()
        .pure("shout")
        .pure("config")
        .run_blocking(&kernel(space));
    let found: Vec<_> = report
        .against("shout")
        .filter(|f| f.check == Check::Cacheable)
        .collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0]
            .detail
            .contains("carries a foreign thread `urn:example:session`:"),
        "{report}"
    );
    assert!(report.against("config").next().is_none(), "{report}");
}

#[test]
fn cacheable_reads_a_thread_named_for_the_endpoint_as_none() {
    // Source only, so nothing writes through `urn:example:notes`: the one thread it
    // declares is the one the kernel adds, and from outside the two are the same
    // thread. The documented limit — name the STATE, or opt out with a reason.
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
    assert!(
        report.against("notes").any(|f| f.check == Check::Cacheable
            && f.detail.contains("no golden thread but its own name")),
        "{report}"
    );
}

/// A cacheable endpoint that names no thread at all: the kernel hangs it on its own
/// name and nothing else.
fn own_name_only(id: &str, verb: Verb) -> FnEndpoint {
    FnEndpoint::new(id, |_inv: &Invocation<'_>| Ok(text("x").cacheable()))
        .with_description(Description::new(id).verb(verb).output("text/plain"))
}

fn cacheable_only(space: Arc<dyn Space>) -> Report {
    Suite::new()
        .checks(Checks::CACHEABLE)
        .run_blocking(&Kernel::new(space))
}

fn own_name_finding(report: &Report, id: &str) -> bool {
    report.against(id).any(|f| {
        f.check == Check::Cacheable && f.detail.contains("no golden thread but its own name")
    })
}

/// Ledger #596 claimed the purity rule could never fire since core 0.1.73, because
/// the kernel hangs every cacheable answer on its own target's thread. That was true
/// of the rule spelled `threads().is_empty()` and stopped being true in 0.5.0
/// (ledger #549). Pinned here across the shapes a module binds: a Source, an Exists,
/// a template-bound name, a required by-value argument, a `Mount`. Each answer's
/// only thread is its own name, and each is caught; one real foreign thread is not.
#[test]
fn cacheable_fires_on_every_shape_with_no_thread_but_its_own_name() {
    let shapes: Vec<(&str, Arc<dyn Space>)> = vec![
        (
            "src",
            Arc::new(EndpointSpace::new().bind(
                Exact::new("urn:example:src"),
                own_name_only("src", Verb::Source),
            )),
        ),
        (
            "ask",
            Arc::new(EndpointSpace::new().bind(
                Exact::new("urn:example:ask"),
                own_name_only("ask", Verb::Exists),
            )),
        ),
        (
            "note",
            Arc::new(
                EndpointSpace::new().bind(
                    UriTemplate::parse("urn:example:note:{n}").unwrap(),
                    FnEndpoint::new("note", |_inv: &Invocation<'_>| Ok(text("x").cacheable()))
                        .with_description(
                            Description::new("note")
                                .verb(Verb::Source)
                                .input(ArgSpec::new("n").class(XSD_INTEGER).binding())
                                .output("text/plain"),
                        ),
                ),
            ),
        ),
        (
            "echo",
            Arc::new(
                EndpointSpace::new().bind(
                    Exact::new("urn:example:echo"),
                    FnEndpoint::new("echo", |inv: &Invocation<'_>| {
                        Ok(text(inv.inline_str("in")?.to_string()).cacheable())
                    })
                    .with_description(
                        Description::new("echo")
                            .verb(Verb::Source)
                            .input(ArgSpec::new("in").class(XSD_STRING))
                            .output("text/plain"),
                    ),
                ),
            ),
        ),
        (
            "mounted",
            Arc::new(ikigai_core::Mount::new(
                "urn:example:m:",
                Arc::new(EndpointSpace::new().bind(
                    Exact::new("urn:example:m:mounted"),
                    own_name_only("mounted", Verb::Source),
                )),
            )),
        ),
    ];
    for (id, space) in shapes {
        let report = cacheable_only(space);
        assert!(own_name_finding(&report, id), "{id}:\n{report}");
    }

    let foreign = FnEndpoint::new("store-view", |_inv: &Invocation<'_>| {
        Ok(text("x").cacheable().depends_on("urn:example:store"))
    })
    .with_description(
        Description::new("store-view")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let report = cacheable_only(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:example:store-view"), foreign),
    ));
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

/// A space that LISTS a logical name and resolves it onto a backing one, reporting
/// the rewrite (`Resolved::canonical`). The kernel adopts the backing name as the
/// request's identity, so that is the thread it hangs the read on.
struct Renamed {
    logical: &'static str,
    backing: &'static str,
    inner: EndpointSpace,
}

impl Space for Renamed {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        if request.target.as_str() != self.logical {
            return Resolution::Miss;
        }
        let backing = Iri::parse(self.backing).unwrap();
        let mut rewritten = request.clone();
        rewritten.target = backing.clone();
        match self.inner.resolve(&rewritten, scope) {
            Resolution::Hit(hit) => Resolution::Hit(hit.with_canonical(backing)),
            other => other,
        }
    }
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        let id = self.inner.entries()?.into_iter().next()?.endpoint;
        Some(vec![SpaceEntry::new(self.logical, id)])
    }
}

/// "Its own name" is the CANONICAL target. Compared against the listed name alone,
/// the backing-name thread read as foreign and the rule was dead for every endpoint
/// behind a renaming space (the variant of ledger #596 that still held).
#[test]
fn cacheable_reads_a_reported_canonical_as_its_own_name() {
    let renamed = |endpoint: FnEndpoint| Renamed {
        logical: "urn:example:logical",
        backing: "urn:example:backing",
        inner: EndpointSpace::new().bind(Exact::new("urn:example:backing"), endpoint),
    };

    // The premise: the thread is the backing name, not the one that was asked for.
    let kernel = Kernel::new(Arc::new(renamed(own_name_only("renamed", Verb::Source))));
    let read = Request::new(Verb::Source, Iri::parse("urn:example:logical").unwrap());
    let answer = futures::executor::block_on(kernel.issue(read, &Capability::root())).unwrap();
    assert_eq!(
        answer.threads().iter().collect::<Vec<_>>(),
        [&Thread::new("urn:example:backing")]
    );

    let report = cacheable_only(Arc::new(renamed(own_name_only("renamed", Verb::Source))));
    assert!(own_name_finding(&report, "renamed"), "{report}");

    // And a real foreign thread behind the same rename is still not a finding.
    let foreign = FnEndpoint::new("renamed", |_inv: &Invocation<'_>| {
        Ok(text("x").cacheable().depends_on("urn:example:store"))
    })
    .with_description(
        Description::new("renamed")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let report = cacheable_only(Arc::new(renamed(foreign)));
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");
}

/// 2026-09-27T12:34:56Z, and the next minute boundary after it.
const NOW: u64 = 1_790_512_496_000;
const NEXT_MINUTE: u64 = 1_790_512_500_000;

/// A clock reading: cacheable until the next minute (`Expiry::At`) and threaded on
/// nothing, because a clock is not a resource anything could cut. The deadline is
/// the bound — the shape `ikigai-tz`'s `tz-now` has (ledger #1000).
fn minute_clock() -> FnEndpoint {
    FnEndpoint::new("minute", |_inv: &Invocation<'_>| {
        Ok(text("12:34").cacheable_until(Time::from_millis(NEXT_MINUTE)))
    })
    .with_description(
        Description::new("minute")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

/// The purity rule is about a result that would be served FOREVER: `Expiry::Never`
/// with nothing to cut it. An `Expiry::At` deadline bounds the answer in time, so it
/// is not that, and the rule does not ask it to be declared pure (ledger #1000).
#[test]
fn cacheable_reads_an_at_deadline_as_a_bound() {
    // A clocked kernel honors the deadline: the second read is a cache hit, and
    // nothing about the answer is "forever".
    let space = EndpointSpace::new().bind(Exact::new("urn:example:minute"), minute_clock());
    let clocked = kernel(space).with_clock(Arc::new(FixedClock::at(NOW)));
    let first = futures::executor::block_on(clocked.issue(
        Request::new(Verb::Source, Iri::parse("urn:example:minute").unwrap()),
        &Capability::root(),
    ))
    .unwrap();
    assert_eq!(
        first.expiry,
        Expiry::At(Time::from_millis(NEXT_MINUTE)),
        "the premise: the kernel hands the deadline back unchanged"
    );
    assert_eq!(
        first.threads().iter().collect::<Vec<_>>(),
        [&Thread::new("urn:example:minute")],
        "the premise: no golden thread but its own name"
    );
    let report = Suite::new().run_blocking(&clocked);
    assert!(report.of(Check::Cacheable).next().is_none(), "{report}");

    // A clockless kernel declines to cache a deadline, and THAT is a finding with
    // its true cause — but still not "served forever".
    let space = EndpointSpace::new().bind(Exact::new("urn:example:minute"), minute_clock());
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Cacheable,
        "the result carries `Expiry::At` on a clockless kernel",
    );
    assert!(
        report
            .of(Check::Cacheable)
            .all(|f| !f.detail.contains("no golden thread but its own name")),
        "{report}"
    );

    // The bound is the deadline, not the clock: the same endpoint answering
    // `Expiry::Never` on the same clocked kernel is still caught.
    let forever = FnEndpoint::new("minute", |_inv: &Invocation<'_>| {
        Ok(text("12:34").cacheable())
    })
    .with_description(
        Description::new("minute")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:minute"), forever);
    let clocked = kernel(space).with_clock(Arc::new(FixedClock::at(NOW)));
    let report = Suite::new().run_blocking(&clocked);
    assert_caught(
        &report,
        Check::Cacheable,
        "no golden thread but its own name",
    );
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
// check at all, and the report printed it as if one had honored it.

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

/// Ledger #340: a fixture argument naming no declared input was still SENT, and
/// nothing said so. `upper` declares `in`; the fixture's `inn` is a typo, the walk
/// derived `in` itself, and the endpoint ignored the typo, so the walk tested
/// something other than what the fixture says, under a clean report.
#[test]
fn a_fixture_argument_naming_no_declared_input_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .fixture(Fixture::new("upper", Verb::Source).arg("inn", "hello"))
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "sends `inn`");
    let inert = only(&report, Check::Declarations);
    assert_eq!(inert.len(), 1, "{report}");
    assert!(
        inert[0].contains("(it declares `in`)"),
        "the finding names the inputs there ARE:\n{report}"
    );

    // The declared name is not a finding, and neither is `as`, the universal conneg
    // selector, which no endpoint declares and every endpoint may be sent.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .fixture(
            Fixture::new("upper", Verb::Source)
                .arg("in", "hello")
                .arg("as", "text/plain"),
        )
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");

    // A template variable passed as an ARGUMENT is sent by value and never reaches
    // the IRI: the finding says to bind it instead.
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
        .fixture(Fixture::new("pr", Verb::Source).arg("n", "42"))
        .run_blocking(&kernel(space));
    assert_caught(&report, Check::Declarations, "`n` is a binding input");
    assert_caught(&report, Check::Declarations, ".binding(\"n\", …)");
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
        Check::Resolves,
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

/// A Sink whose by-value inputs are ALL optional and none is `content`: the shape
/// `urn:log:write` shipped with, reading the piped `content` as an undeclared fallback
/// for its optional `msg` (ledger #339).
fn all_optional_sink() -> FnEndpoint {
    FnEndpoint::new("log-write", |inv: &Invocation<'_>| {
        let msg = inv
            .inline_str("msg")
            .or_else(|_| inv.inline_str("content"))?;
        Ok(text(msg.to_string()))
    })
    .with_description(
        Description::new("log-write")
            .verb(Verb::Sink)
            .input(ArgSpec::new("msg").class(XSD_STRING).optional())
            .input(ArgSpec::new("level").class(XSD_STRING).optional())
            .output("text/plain"),
    )
}

#[test]
fn pipeline_catches_an_all_optional_sink_without_content() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:log-write"), all_optional_sink());
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Pipeline,
        "declares by-value inputs (`msg`, `level`) but none named `content`",
    );
    assert_caught(
        &report,
        Check::Pipeline,
        "Every one of them is optional, so a pipe alone is the likeliest way a caller drives it",
    );
    // Waivable per endpoint, for a Sink that genuinely takes no body.
    let waived = Suite::new()
        .opt_out_check("log-write", Check::Pipeline, "takes no body: a trigger")
        .run_blocking(&kernel(
            EndpointSpace::new().bind(Exact::new("urn:example:log-write"), all_optional_sink()),
        ));
    assert!(waived.of(Check::Pipeline).next().is_none(), "{waived}");
    // A Delete with only optional inputs keeps the old rule: a `reason` beside a
    // deletion is metadata, not a body (ikigai-ledger's `ledger-item` delete).
    let delete = FnEndpoint::new("item", |_inv: &Invocation<'_>| Ok(text("gone")))
        .with_description(
            Description::new("item")
                .verb(Verb::Delete)
                .input(ArgSpec::new("reason").class(XSD_STRING).optional())
                .output("text/plain"),
        );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:item"), delete));
    assert!(report.of(Check::Pipeline).next().is_none(), "{report}");
    // A Sink with no by-value input at all is not reported: nothing names its body.
    let bare = FnEndpoint::new("tick", |_inv: &Invocation<'_>| Ok(text("ok"))).with_description(
        Description::new("tick")
            .verb(Verb::Sink)
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:tick"), bare));
    assert!(report.of(Check::Pipeline).next().is_none(), "{report}");
}

/// A Sink whose `content` has a grammar (a TOML layer, say), with `content` OPTIONAL —
/// the shape `urn:log:config` took (ledger #1071). Only the pipeline probe puts a
/// `content` in the request, and the suite's generic sample is not a document.
/// `refuse` picks how it says no: naming `content`, or as an untyped endpoint error.
fn grammar_sink(id: &'static str, refuse: fn(&str) -> Error) -> FnEndpoint {
    FnEndpoint::new(id, move |inv: &Invocation<'_>| {
        match inv.inline_str("content") {
            Ok(body) if !body.contains('=') => Err(refuse(body)),
            _ => Ok(text("applied")),
        }
    })
    .with_description(
        Description::new(id)
            .verb(Verb::Sink)
            .input(ArgSpec::new("content").class(XSD_STRING).optional())
            .output("text/plain"),
    )
}

#[test]
fn a_refused_generic_content_sample_is_named_as_the_cause() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:config"),
        grammar_sink("toml-config", |body| Error::InvalidArgument {
            name: "content".into(),
            detail: format!("not a TOML layer: {body:?}"),
        }),
    );
    let report = report_of(space);
    // The finding is RESOLVES's (0.8.4; OUTPUTS before it, the first check that needed
    // the firing, which it names), and says the sample was the cause and what fixes it.
    assert_caught(
        &report,
        Check::Resolves,
        "did not resolve with the minimal inputs",
    );
    assert_caught(
        &report,
        Check::Resolves,
        "it refused `content`, and the `content` it was sent is the suite's generic sample \
         `x`, not a value from a Fixture",
    );
    assert_caught(
        &report,
        Check::Resolves,
        "`Fixture::new(\"toml-config\", Verb::Sink).arg(\"content\", …)`",
    );
    // A byte input is fixtured with `arg_bytes` (0.8.1), and the hint says so.
    assert_caught(
        &report,
        Check::Resolves,
        "(`.arg_bytes(\"content\", …)` for a byte input)",
    );
    assert_caught(&report, Check::Resolves, "Needed by OUTPUTS");
    assert!(report.of(Check::Outputs).next().is_none(), "{report}");
}

#[test]
fn an_untyped_refusal_of_a_generic_content_sample_names_it_as_the_likely_cause() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:config"),
        grammar_sink("toml-config", |_| {
            Error::Endpoint("expected `=` at line 1".into())
        }),
    );
    let report = report_of(space);
    assert_caught(
        &report,
        Check::Resolves,
        "the `content` it was sent is the suite's generic sample `x`, not a value from a \
         Fixture; if it parses `content`, that sample is the likely cause",
    );
    assert_caught(
        &report,
        Check::Resolves,
        "`Fixture::new(\"toml-config\", Verb::Sink).arg(\"content\", …)`",
    );
    // A byte input is fixtured with `arg_bytes` (0.8.1), and the hint says so.
    assert_caught(
        &report,
        Check::Resolves,
        "(`.arg_bytes(\"content\", …)` for a byte input)",
    );
}

#[test]
fn a_fixture_supplied_content_is_not_blamed_on_the_sample() {
    // The fixture's own `content` is refused too: the failure is real, and the
    // generic sample had nothing to do with it, so the hint must not appear.
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:config"),
        grammar_sink("toml-config", |body| Error::InvalidArgument {
            name: "content".into(),
            detail: format!("not a TOML layer: {body:?}"),
        }),
    );
    let report = Suite::new()
        .fixture(Fixture::new("toml-config", Verb::Sink).arg("content", "still not toml"))
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Resolves,
        "did not resolve with the minimal inputs",
    );
    assert!(
        !report.to_string().contains("generic sample"),
        "a fixture's value is not the suite's sample:\n{report}"
    );
}

#[test]
fn a_fixed_content_sample_clears_the_finding() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:config"),
        grammar_sink("toml-config", |body| Error::InvalidArgument {
            name: "content".into(),
            detail: format!("not a TOML layer: {body:?}"),
        }),
    );
    let report = Suite::new()
        .fixture(Fixture::new("toml-config", Verb::Sink).arg("content", "level = \"info\""))
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Outputs).next().is_none(), "{report}");
    assert!(report.of(Check::Resolves).next().is_none(), "{report}");
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

// ----- VALIDATE ---------------------------------------------------------------

/// An endpoint whose description is `description`, answering plain text.
fn described(description: Description) -> FnEndpoint {
    FnEndpoint::new(description.id.clone(), |_inv: &Invocation<'_>| {
        Ok(text("x"))
    })
    .with_description(description)
}

fn validate_report(space: EndpointSpace) -> Report {
    Suite::new()
        .checks(Checks::VALIDATE)
        .run_blocking(&kernel(space))
}

#[test]
fn validate_passes_a_clean_endpoint() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = validate_report(space);
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.endpoints, 1);
}

#[test]
fn validate_catches_an_id_holding_a_space_or_an_angle_bracket() {
    let space = EndpointSpace::new()
        .bind(
            Exact::new("urn:example:spaced"),
            described(
                Description::new("two words")
                    .verb(Verb::Source)
                    .output("text/plain"),
            ),
        )
        .bind(
            Exact::new("urn:example:angled"),
            described(
                Description::new("a>b")
                    .verb(Verb::Source)
                    .output("text/plain"),
            ),
        );
    let report = validate_report(space);
    assert_eq!(only(&report, Check::Validate).len(), 2, "{report}");
    // The endpoint, the field and the value, whichever error variant core used.
    let spaced: Vec<_> = report.against("two words").collect();
    assert_eq!(spaced.len(), 1, "{report}");
    assert!(spaced[0].detail.contains("id `two words`"), "{report}");
    let angled: Vec<_> = report.against("a>b").collect();
    assert_eq!(angled.len(), 1, "{report}");
    assert!(angled[0].detail.contains("id `a>b`"), "{report}");
    assert!(!report.is_clean());
}

#[test]
fn validate_catches_an_input_name_holding_a_c1_control() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:nel"),
        described(
            Description::new("nel")
                .verb(Verb::Source)
                .input(ArgSpec::new("in\u{85}put").class(XSD_STRING))
                .output("text/plain"),
        ),
    );
    let report = validate_report(space);
    let found = only(&report, Check::Validate);
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].contains("input name"), "{report}");
    assert!(found[0].contains("U+0085"), "{report}");
    // The refused code point is shown escaped, never raw on the report line.
    assert!(!report.to_string().contains('\u{85}'), "{report}");
}

#[test]
fn validate_never_prints_a_refused_control_character_raw() {
    // A newline is refused as an IRIREF character. Core 0.1.100's refusal quotes the
    // value `escape_debug`ged (`two\nlines`); 0.1.99 quoted it raw, and the report's own
    // escaping (`two\u{a}lines`) is what kept the finding one line then. Either way the
    // finding is one line and names the value.
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:split"),
        described(
            Description::new("two\nlines")
                .verb(Verb::Source)
                .output("text/plain"),
        ),
    );
    let report = validate_report(space);
    let found = only(&report, Check::Validate);
    assert_eq!(found.len(), 1, "{report}");
    assert!(!found[0].contains('\n'), "{report}");
    assert!(found[0].contains("two\\nlines"), "{report}");
}

#[test]
fn a_control_character_in_an_endpoint_id_never_splits_a_report_line() {
    // The finding DETAIL was escaped and the ID column was not, so an id holding a
    // newline printed `two` on one line and `lines  VALIDATE …` on the next (ledger
    // #1392). One finding is one line, whatever the id holds.
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:split"),
        described(
            Description::new("two\nlines")
                .verb(Verb::Source)
                .output("text/plain"),
        ),
    );
    let report = validate_report(space);
    assert_eq!(report.findings.len(), 1, "{report}");
    // The struct keeps the id as the endpoint declared it, so `against` still finds it.
    assert_eq!(report.against("two\nlines").count(), 1, "{report}");
    let text = report.to_string();
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[0].starts_with("two\\u{a}lines  VALIDATE  "),
        "the id column is escaped:\n{text}"
    );
    assert!(
        lines[1].starts_with("1 finding(s) across"),
        "one finding, one line:\n{text}"
    );
}

#[test]
fn validate_catches_a_name_declared_on_an_action_spec() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:notes"),
        described(
            Description::new("notes").action(
                ActionSpec::new(Verb::Sink)
                    .input(ArgSpec::new("con tent").class(XSD_STRING))
                    .output("text/plain"),
            ),
        ),
    );
    let report = validate_report(space);
    assert_caught(&report, Check::Validate, "input name `con tent`");
}

#[test]
fn validate_is_waivable_per_endpoint_and_selectable_alone() {
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:spaced"),
        described(
            Description::new("two words")
                .verb(Verb::Source)
                .output("text/plain"),
        ),
    );
    let report = Suite::new()
        .checks(Checks::VALIDATE)
        .opt_out_check(
            "two words",
            Check::Validate,
            "a legacy id, renamed next release",
        )
        .run_blocking(&kernel(space));
    assert!(report.of(Check::Validate).next().is_none(), "{report}");
}

/// An id that is a full IRI with an IPv6 host. `validate` admits `[` and `]` (they are
/// legal in an IRI), and from core 0.1.100 the emitters ENCODE them in a name
/// (`encode_name`, ledger #1361, #1385), so the check passes AND the Turtle parses. On
/// core 0.1.99 the emitters spelled them raw and the document did not parse, which is
/// why the floor is 0.1.100.
#[test]
fn validate_passes_an_ipv6_iri_id_and_its_turtle_parses() {
    let id = "http://[::1]/x";
    let description = Description::new(id).verb(Verb::Source).output("text/plain");
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:ipv6"),
        described(description.clone()),
    );
    let report = validate_report(space);
    assert!(report.is_clean(), "{report}");
    let turtle = ikigai_vocab::to_turtle(&description);
    if let Err(e) = ikigai_conformance::rdf::parse(TURTLE, turtle.as_bytes()) {
        panic!("the description's Turtle does not parse: {e}\n{turtle}");
    }
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
        Check::Resolves,
        "did not resolve with the minimal inputs",
    );
    // Reported once, under RESOLVES, naming the first check that needed it — not once
    // per check.
    assert_caught(&report, Check::Resolves, "Needed by SKOLEM-RDF");
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
        FnEndpoint::new("json-keys", |inv: &Invocation<'_>| {
            // Reads what it declares required, so REQUIRED is satisfied too.
            inv.inline_str("content")?;
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

// ----- FIRING IDENTITY --------------------------------------------------------
//
// What the invoking checks memoize on. `Description::id` is NOT it: two entries can
// share one description and be two instances over different state, so collapsing
// them by id either leaves the dangerous one unprobed under a green report or fires
// it and loses the safe one's coverage. The key is the REQUEST — verb, target IRI,
// arguments — which two entries can only share by being the same firing.
//
// Every test here COUNTS FIRINGS rather than asserting an outcome: an outcome test
// passes for the wrong reason the moment the endpoint's state is idempotent, which
// is exactly when a returning double-fire would stop being visible.

/// A space that lists the same bindings twice — the shape an overlay concatenating
/// its targets' entries produces (`ikigai-throttle`'s `Failover`: two spaces over ONE
/// state). The duplication is in the ENUMERATION, not in the descriptions, so nothing
/// keyed on a description can see it.
struct Doubled(EndpointSpace);

impl ikigai_core::Space for Doubled {
    fn resolve(
        &self,
        request: &ikigai_core::Request,
        scope: &ikigai_core::Scope,
    ) -> ikigai_core::Resolution {
        self.0.resolve(request, scope)
    }

    fn entries(&self) -> Option<Vec<ikigai_core::SpaceEntry>> {
        let entries = self.0.entries()?;
        let mut all = entries.clone();
        all.extend(entries);
        Some(all)
    }
}

/// A Sink that counts every invocation and stores nothing — the witness. Its id and
/// description are shared by every instance, which is the point.
fn counting_sink(counter: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("notes-write", move |inv: &Invocation<'_>| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(text(format!("stored {}", inv.inline_str("content")?.len())))
    })
    .with_description(
        Description::new("notes-write")
            .verb(Verb::Sink)
            .requires("urn:cap:example:write")
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn a_space_that_lists_one_binding_twice_fires_it_once() {
    let fired = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:notes-write"),
        counting_sink(fired.clone()),
    );
    let report = Suite::new().run_blocking(&Kernel::new(Arc::new(Doubled(space))));
    assert!(report.is_clean(), "{report}");
    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "two entries issuing the identical request are one firing:\n{report}"
    );
    // The counts follow the firings, and the report SAYS the second entry was the
    // first one again — a smaller action count with no explanation is what the
    // report could not say before.
    assert_eq!(report.endpoints, 1, "{report}");
    assert_eq!(report.actions, 1, "{report}");
    assert_eq!(report.collapsed.len(), 1, "{report}");
    assert_eq!(report.collapsed[0].endpoint, "notes-write");
    assert_eq!(report.collapsed[0].patterns.len(), 2);
    assert!(
        report.to_string().contains(
            "collapsed: notes-write sink <urn:example:notes-write>: 2 binding(s) issue the \
             identical request (`urn:example:notes-write`, `urn:example:notes-write`) — fired once"
        ),
        "{report}"
    );
}

#[test]
fn two_instances_sharing_one_description_are_both_fired() {
    // ★ The case that makes `Description::id` unsafe as the key. Two endpoints, one
    // description, DIFFERENT state behind each — `ikigai-cli` binds
    // `ikigai_fs::FileEndpoint` at `urn:file:{path}` (jailed to a scratch root) and
    // at `urn:orgfile:{path}` (jailed to whatever the config names, which with no
    // config at all is the process's working directory). Memoizing by id probes
    // whichever is walked first and reports green about the other.
    let safe = Arc::new(AtomicUsize::new(0));
    let hazardous = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:notes"), counting_sink(safe.clone()))
        .bind(
            Exact::new("urn:example:other-notes"),
            counting_sink(hazardous.clone()),
        );
    let report = Suite::new().run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert_eq!(
        (
            safe.load(Ordering::SeqCst),
            hazardous.load(Ordering::SeqCst)
        ),
        (1, 1),
        "each instance is its own firing; neither hides behind the other:\n{report}"
    );
    assert_eq!(report.endpoints, 1, "one description id:\n{report}");
    assert_eq!(report.actions, 2, "two firings:\n{report}");
    assert!(report.collapsed.is_empty(), "nothing collapsed:\n{report}");
}

#[test]
fn opt_out_at_excludes_one_binding_and_leaves_the_other_probed() {
    let safe = Arc::new(AtomicUsize::new(0));
    let hazardous = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:notes"), counting_sink(safe.clone()))
        .bind(
            Exact::new("urn:example:other-notes"),
            counting_sink(hazardous.clone()),
        );
    let report = Suite::new()
        .opt_out_at(
            "urn:example:other-notes",
            None,
            "writes outside the scratch root",
        )
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert_eq!(
        (
            safe.load(Ordering::SeqCst),
            hazardous.load(Ordering::SeqCst)
        ),
        (1, 0),
        "the named binding is excluded; the other keeps its coverage:\n{report}"
    );
    assert!(
        report
            .to_string()
            .contains("opted out at: `urn:example:other-notes`: writes outside the scratch root"),
        "{report}"
    );
    // `opt_out("notes-write", …)` would have taken both: the id is the same.
    let safe = Arc::new(AtomicUsize::new(0));
    let hazardous = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:notes"), counting_sink(safe.clone()))
        .bind(
            Exact::new("urn:example:other-notes"),
            counting_sink(hazardous.clone()),
        );
    let report = Suite::new()
        .opt_out("notes-write", None, "writes outside the scratch root")
        .run_blocking(&kernel(space));
    assert_eq!(
        (
            safe.load(Ordering::SeqCst),
            hazardous.load(Ordering::SeqCst)
        ),
        (0, 0),
        "the coarse lever is why the fine one exists:\n{report}"
    );
}

#[test]
fn an_opt_out_at_that_excluded_nothing_is_reported() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .opt_out_at("urn:example:typo", None, "a pattern nothing binds")
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "opted out of the invoking checks at `urn:example:typo` (a pattern nothing binds) but \
         excluded nothing: the walk reached no binding with that pattern",
    );
    assert!(
        report
            .of(Check::Declarations)
            .any(|f| f.detail.contains("The walk reached: `urn:example:upper`")),
        "{report}"
    );
}

#[test]
fn a_template_pattern_is_matched_verbatim_not_expanded() {
    let space = EndpointSpace::new().bind(
        UriTemplate::parse("urn:example:notes/{name}").expect("valid template"),
        counting_sink(Arc::new(AtomicUsize::new(0))),
    );
    // The expanded IRI is not the pattern: this one names nothing.
    let report = Suite::new()
        .opt_out_at("urn:example:notes/x", None, "the expanded form")
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "the walk reached no binding with that pattern (it is matched verbatim",
    );
}

// ----- SPACE-NAME ---------------------------------------------------------------

/// One door, an echo, at `urn:example:{name}`: the smallest space a constructor
/// can build.
fn one_door(name: &str) -> EndpointSpace {
    EndpointSpace::new().bind(
        Exact::new(format!("urn:example:{name}")),
        ikigai_core::builtins::echo(),
    )
}

/// The space check alone, so a fixture's other findings never mask it.
fn spaces_only() -> Suite {
    Suite::new().checks(Checks::SPACE_NAME | Checks::DECLARATIONS)
}

/// A hand-built space that claims whatever it is told to, in `id()`, in its
/// topology and in its listing separately — the three places a name or a door set
/// can be read, made to disagree on purpose.
struct Says {
    id: Option<Iri>,
    top: Topology,
    entries: Option<Vec<SpaceEntry>>,
}

impl Space for Says {
    fn resolve(&self, _request: &Request, _scope: &Scope) -> Resolution {
        Resolution::Miss
    }
    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        self.entries.clone()
    }
    fn id(&self) -> Option<Iri> {
        self.id.clone()
    }
    fn topology(&self) -> Topology {
        self.top.clone()
    }
}

fn space_report(suite: Suite) -> Report {
    suite.run_blocking(&kernel(
        EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming()),
    ))
}

#[test]
fn space_name_catches_a_self_named_space_that_claims_nothing() {
    fn anonymous() -> EndpointSpace {
        one_door("echo")
    }
    let report = space_report(spaces_only().self_named_space("example", anonymous));
    assert_caught(
        &report,
        Check::SpaceName,
        "claims no name, but it is declared self-named: a configuration-free space names \
         itself — end the constructor with `.named(ikigai_core::space_iri(\"example\"))`, \
         which is `urn:iki:space:example`",
    );
    let finding = report.of(Check::SpaceName).next().expect("one finding");
    assert_eq!(finding.endpoint, "urn:iki:space:example", "{report}");
    assert!(
        finding.detail.contains("anonymous"),
        "names the constructor:\n{report}"
    );
    assert_eq!(report.of(Check::SpaceName).count(), 1, "{report}");
}

#[test]
fn space_name_catches_a_name_outside_the_prefix() {
    // The pre-#987 spelling: `urn:ikigai:space:`, not `urn:iki:space:`.
    fn legacy() -> EndpointSpace {
        one_door("echo").named(Iri::parse("urn:ikigai:space:example").unwrap())
    }
    let report = space_report(spaces_only().self_named_space("example", legacy));
    assert_caught(
        &report,
        Check::SpaceName,
        "claims `urn:ikigai:space:example`, outside `urn:iki:space:`: a module's \
         configuration-free space is named `ikigai_core::space_iri(\"example\")`, which is \
         `urn:iki:space:example`",
    );
    assert_eq!(report.of(Check::SpaceName).count(), 1, "{report}");
}

#[test]
fn space_name_catches_a_name_other_than_the_one_declared() {
    fn other() -> EndpointSpace {
        one_door("echo").named(space_iri("other"))
    }
    let report = space_report(spaces_only().self_named_space("example", other));
    assert_caught(
        &report,
        Check::SpaceName,
        "claims `urn:iki:space:other`, but it is declared as `urn:iki:space:example`",
    );
}

#[test]
fn space_name_catches_a_host_named_constructor_that_names_itself() {
    // Parameterized, so the doors depend on what it was handed; naming it anyway
    // claims one name for every instance.
    fn space(name: &str) -> EndpointSpace {
        one_door(name).named(space_iri("example"))
    }
    let report = space_report(spaces_only().host_named_space("space(name)", space("echo")));
    assert_caught(
        &report,
        Check::SpaceName,
        "claims `urn:iki:space:example`, but it is declared host-named: its doors depend on \
         what it was handed, so only the host knows which instance it is. Drop `.named(..)` \
         and let the host name it — a name is a claim: same name, same doors",
    );
    assert_eq!(
        report.of(Check::SpaceName).next().unwrap().endpoint,
        "space(name)",
        "{report}"
    );
}

#[test]
fn space_name_catches_a_host_named_space_whose_topology_names_it() {
    let says = Says {
        id: None,
        top: Topology::new(SpaceKind::Fallback).with_id(Some(space_iri("example"))),
        entries: Some(vec![]),
    };
    let report = space_report(spaces_only().host_named_space("says", says));
    assert_caught(
        &report,
        Check::SpaceName,
        "`id()` claims nothing, but the topology's root node names `urn:iki:space:example`",
    );
}

#[test]
fn space_name_catches_a_self_named_space_whose_two_calls_hold_different_doors() {
    // The shape of a constructor that reads configuration while building: every
    // call answers to the same name over doors of its own.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn drifting() -> EndpointSpace {
        let n = CALLS.fetch_add(1, Ordering::SeqCst);
        one_door(&format!("echo-{n}")).named(space_iri("example"))
    }
    let report = space_report(spaces_only().self_named_space("example", drifting));
    assert_caught(
        &report,
        Check::SpaceName,
        "hold different doors (first call: `/ door 0 `urn:example:echo-0` (exact) -> echo`; \
         second call: `/ door 0 `urn:example:echo-1` (exact) -> echo`): a name is a claim — \
         same name, same doors — and every call answers to `urn:iki:space:example`",
    );
    assert_caught(
        &report,
        Check::SpaceName,
        "declare it with `Suite::host_named_space`",
    );
    assert_eq!(report.of(Check::SpaceName).count(), 1, "{report}");
}

#[test]
fn space_name_catches_a_self_named_space_whose_two_calls_claim_different_names() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn renaming() -> EndpointSpace {
        let n = CALLS.fetch_add(1, Ordering::SeqCst);
        let name = if n == 0 { "example" } else { "example:again" };
        one_door("echo").named(space_iri(name))
    }
    let report = space_report(spaces_only().self_named_space("example", renaming));
    assert_caught(
        &report,
        Check::SpaceName,
        "claim different names (`urn:iki:space:example` then `urn:iki:space:example:again`)",
    );
}

#[test]
fn space_name_catches_doors_only_the_listing_can_see() {
    // An opaque topology hides every door; `entries()` is the second witness.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn opaque() -> Says {
        let n = CALLS.fetch_add(1, Ordering::SeqCst);
        Says {
            id: Some(space_iri("example")),
            top: Topology::opaque(Some(space_iri("example"))),
            entries: Some(vec![SpaceEntry::new(format!("urn:example:{n}"), "echo")]),
        }
    }
    let report = space_report(spaces_only().self_named_space("example", opaque));
    assert_caught(
        &report,
        Check::SpaceName,
        "hold different doors (first call: `entry `urn:example:0` -> echo`; second call: \
         `entry `urn:example:1` -> echo`)",
    );
    // And the evidence line says the topology compared nothing behind the node.
    let text = report.to_string();
    assert!(
        text.contains(
            "two calls, 0 door(s) compared; 1 opaque node(s), whose doors only `entries()` can see"
        ),
        "{text}"
    );
}

#[test]
fn space_name_catches_a_topology_root_that_does_not_carry_the_name() {
    fn half_named() -> Says {
        Says {
            id: Some(space_iri("example")),
            top: Topology::new(SpaceKind::Fallback),
            entries: Some(vec![]),
        }
    }
    let report = space_report(spaces_only().self_named_space("example", half_named));
    assert_caught(
        &report,
        Check::SpaceName,
        "the topology's root node names no name while `id()` claims `urn:iki:space:example`",
    );
}

#[test]
fn space_name_catches_two_declarations_of_one_name_over_different_doors() {
    // Each passes alone: the part claims the whole's name.
    fn whole() -> EndpointSpace {
        one_door("echo")
            .bind(
                Exact::new("urn:example:part"),
                ikigai_core::builtins::to_upper(),
            )
            .named(space_iri("example"))
    }
    fn part() -> EndpointSpace {
        one_door("echo").named(space_iri("example"))
    }
    let report = space_report(
        spaces_only()
            .self_named_space("example", whole)
            .self_named_space("example", part),
    );
    let findings: Vec<_> = report.of(Check::SpaceName).collect();
    assert_eq!(findings.len(), 1, "{report}");
    assert!(
        findings[0].detail.contains(
            "are both declared as `urn:iki:space:example` but hold \
             different doors (first call: `/ door 1 `urn:example:part` (exact) -> toUpper`; \
             second call: nothing)"
        ),
        "{report}"
    );
    assert!(
        findings[0]
            .detail
            .contains("`space_iri(\"<module>:<part>\")`"),
        "{report}"
    );
}

#[test]
fn a_space_declared_while_space_name_is_not_selected_is_inert() {
    fn named() -> EndpointSpace {
        one_door("echo").named(space_iri("example"))
    }
    let report = space_report(
        Suite::new()
            .checks(Checks::DECLARATIONS)
            .self_named_space("example", named),
    );
    assert_caught(
        &report,
        Check::Declarations,
        "declared self-named (`Suite::self_named_space`) but SPACE-NAME is not selected",
    );
    assert!(
        report
            .to_string()
            .contains("space: urn:iki:space:example self-named by"),
        "{report}"
    );
    assert!(report.to_string().contains(": not compared"), "{report}");
}

#[test]
fn a_space_name_waiver_is_per_declared_space() {
    fn legacy() -> EndpointSpace {
        one_door("echo").named(Iri::parse("urn:ikigai:space:example").unwrap())
    }
    let report = space_report(
        spaces_only()
            .self_named_space("example", legacy)
            .opt_out_check(
                "urn:iki:space:example",
                Check::SpaceName,
                "renamed in the module wave",
            )
            .opt_out_check(
                "urn:iki:space:typo",
                Check::SpaceName,
                "a label nothing declares",
            ),
    );
    assert_eq!(report.of(Check::SpaceName).count(), 0, "waived:\n{report}");
    assert_caught(
        &report,
        Check::Declarations,
        "waived SPACE-NAME (a label nothing declares) but that check could not have run for \
         it anyway: no space is declared under that label",
    );
    assert_eq!(report.of(Check::Declarations).count(), 1, "{report}");
    assert!(
        report
            .to_string()
            .contains("* SPACE-NAME ran on 0 of 1 space(s); waived on the rest"),
        "{report}"
    );
}

#[test]
fn a_report_with_no_space_declared_says_so() {
    let report = space_report(spaces_only());
    assert!(report.is_clean(), "{report}");
    assert!(
        report.to_string().contains("space: none declared"),
        "{report}"
    );
    // Not selected, not mentioned.
    let report = space_report(Suite::new().checks(Checks::NAMES));
    assert!(!report.to_string().contains("space: "), "{report}");
}

#[test]
fn space_name_runs_when_the_kernel_cannot_be_walked() {
    fn named_wrong() -> EndpointSpace {
        one_door("echo")
    }
    let unlisted = Says {
        id: None,
        top: Topology::opaque(None),
        entries: None,
    };
    let report = spaces_only()
        .self_named_space("example", named_wrong)
        .run_blocking(&Kernel::new(Arc::new(unlisted)));
    assert_caught(&report, Check::SpaceName, "claims no name");
}

#[test]
#[should_panic(expected = "drop the crate's `ikigai-` prefix")]
fn a_crate_name_in_place_of_a_module_name_is_refused_where_it_is_written() {
    fn space() -> EndpointSpace {
        one_door("echo")
    }
    let _ = Suite::new().self_named_space("ikigai-example", space);
}

// ----- REQUIRED ---------------------------------------------------------------

/// Declares `name` REQUIRED and does without it: `unwrap_or("friend")`, the
/// runbook's `urn:action:greet` for eleven releases (ledger #329).
fn greet_defaulting() -> FnEndpoint {
    FnEndpoint::new("greet", |inv: &Invocation<'_>| {
        let name = inv.inline_str("name").unwrap_or("friend");
        Ok(text(format!("hello, {name}")))
    })
    .with_description(
        Description::new("greet")
            .verb(Verb::Source)
            .input(ArgSpec::new("name").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn required_catches_an_input_declared_required_that_the_endpoint_does_without() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:greet"), greet_defaulting());
    let report = report_of(space);
    let found = only(&report, Check::Required);
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains("declares `name` required, but resolved without it"),
        "{report}"
    );
    // Nothing else sees it: every other check is satisfied by the minimal call,
    // which supplies every required input.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:greet"), greet_defaulting());
    let report = Suite::new()
        .checks(Checks::all() - Checks::REQUIRED)
        .run_blocking(&kernel(space));
    assert!(report.is_clean(), "{report}");
    assert!(report.to_string().contains("skipped: REQUIRED"), "{report}");

    // Refusing the absence is the fix.
    let strict = FnEndpoint::new("greet", |inv: &Invocation<'_>| {
        Ok(text(format!("hello, {}", inv.inline_str("name")?)))
    })
    .with_description(
        Description::new("greet")
            .verb(Verb::Source)
            .input(ArgSpec::new("name").class(XSD_STRING))
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:greet"), strict));
    assert!(report.is_clean(), "{report}");
}

#[test]
fn required_reports_an_untyped_refusal_of_a_missing_input() {
    let untyped = FnEndpoint::new("greet", |inv: &Invocation<'_>| {
        let name = inv
            .inline_str("name")
            .map_err(|_| Error::Endpoint("greet: no name given".into()))?;
        Ok(text(format!("hello, {name}")))
    })
    .with_description(
        Description::new("greet")
            .verb(Verb::Source)
            .input(ArgSpec::new("name").class(XSD_STRING))
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:greet"), untyped));
    assert_caught(&report, Check::Required, "not as a typed `MissingArgument`");
    assert_caught(&report, Check::Required, "greet: no name given");
}

#[test]
fn required_does_not_report_a_default_twice_or_an_alternative_intake_left_unsent() {
    // A default beside `required` is ARGSPECS's finding; the call succeeds on the
    // default by design, so REQUIRED stays out of it. (`default_value` makes an
    // input optional, so the contradiction has to be written out by hand.)
    let mut name = ArgSpec::new("name")
        .class(XSD_STRING)
        .default_value("friend");
    name.required = true;
    let defaulted = FnEndpoint::new("greet", |inv: &Invocation<'_>| {
        Ok(text(format!(
            "hello, {}",
            inv.inline_str("name").unwrap_or("friend")
        )))
    })
    .with_description(
        Description::new("greet")
            .verb(Verb::Source)
            .input(name)
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:greet"), defaulted));
    assert_caught(
        &report,
        Check::ArgSpecs,
        "has a default but is marked required",
    );
    assert!(report.of(Check::Required).next().is_none(), "{report}");

    // `in` required, `content` an optional alternative (ikigai-encrypt's shape):
    // dropping `in` from the minimal call sends no `content` either, so the call is
    // refused and the declaration holds, as it does for the engine's routing.
    let either = FnEndpoint::new("seal", |inv: &Invocation<'_>| {
        let body = inv
            .inline_str("in")
            .or_else(|_| inv.inline_str("content"))?;
        Ok(text(body.to_uppercase()))
    })
    .with_description(
        Description::new("seal")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").class(XSD_STRING))
            .input(ArgSpec::new("content").class(XSD_STRING).optional())
            .output("text/plain"),
    );
    let report = report_of(EndpointSpace::new().bind(Exact::new("urn:example:seal"), either));
    assert!(report.is_clean(), "{report}");
}

/// A `Sink` declaring `content` and `title` required that defaults `title`, and
/// counts its firings.
fn note_write(fired: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("note-write", move |inv: &Invocation<'_>| {
        fired.fetch_add(1, Ordering::SeqCst);
        let body = inv.inline_str("content")?;
        let title = inv.inline_str("title").unwrap_or("untitled");
        Ok(text(format!("{title}: {body}")))
    })
    .with_description(
        Description::new("note-write")
            .verb(Verb::Sink)
            .requires("urn:cap:notes:write")
            .input(ArgSpec::new("content").class(XSD_STRING))
            .input(ArgSpec::new("title").class(XSD_STRING))
            .output("text/plain"),
    )
}

#[test]
fn required_fires_a_mutating_action_only_when_the_module_says_so() {
    // Default: a Sink is held to nothing here, and fired exactly once (the
    // pipeline probe; ENFORCED is refused by the kernel before the endpoint runs).
    let fired = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:note-write"),
        note_write(fired.clone()),
    );
    let report = report_of(space);
    assert!(report.of(Check::Required).next().is_none(), "{report}");
    assert_eq!(fired.load(Ordering::SeqCst), 1, "{report}");

    // Opted in: measured against the pipeline probe's firing, then one firing per
    // required input. `title` is caught; `content` is refused, typed.
    let fired = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:note-write"),
        note_write(fired.clone()),
    );
    let report = Suite::new()
        .required_on_mutating()
        .run_blocking(&kernel(space));
    let found = only(&report, Check::Required);
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].contains("declares `title` required"), "{report}");
    assert_eq!(fired.load(Ordering::SeqCst), 3, "{report}");
    assert!(
        report
            .to_string()
            .contains("declared: REQUIRED fires `Sink` and `Delete` actions too"),
        "{report}"
    );
}

#[test]
fn required_lists_an_action_whose_call_did_not_resolve_as_unprobed() {
    // Needs a JSON document; the suite's `x` is not one, and no fixture says so.
    let needs_json = FnEndpoint::new("json-keys", |inv: &Invocation<'_>| {
        let raw = inv.inline_str("content")?;
        if !raw.starts_with('{') {
            return Err(Error::InvalidArgument {
                name: "content".into(),
                detail: "not a JSON object".into(),
            });
        }
        Ok(text("keys"))
    })
    .with_description(
        Description::new("json-keys")
            .verb(Verb::Source)
            .input(ArgSpec::new("content").class(XSD_STRING))
            .output("text/plain"),
    );
    let report =
        report_of(EndpointSpace::new().bind(Exact::new("urn:example:json-keys"), needs_json));
    assert!(report.of(Check::Required).next().is_none(), "{report}");
    assert!(
        report.to_string().contains(
            "unprobed: json-keys source REQUIRED: the call each required input is dropped from \
             did not resolve"
        ),
        "{report}"
    );
}

#[test]
fn a_required_declaration_that_reached_nothing_is_reported() {
    // Nothing mutating declares a required input: the opt-in reached nothing.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .pure("upper")
        .required_on_mutating()
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "`Suite::required_on_mutating` reached nothing",
    );

    // A waiver of REQUIRED for an endpoint with no required input waives nothing.
    let optional_only = FnEndpoint::new("stats", |_inv: &Invocation<'_>| Ok(text("0")))
        .with_description(
            Description::new("stats")
                .verb(Verb::Source)
                .input(ArgSpec::new("since").class(XSD_STRING).optional())
                .output("text/plain"),
        );
    let report = Suite::new()
        .opt_out_check("stats", Check::Required, "defaults its window")
        .run_blocking(&kernel(
            EndpointSpace::new().bind(Exact::new("urn:example:stats"), optional_only),
        ));
    assert_caught(
        &report,
        Check::Declarations,
        "no action of it REQUIRED probes declares a required by-value input",
    );
}

// ----- WHAT A WALK FIRES (ledger #348) ------------------------------------------

/// A `Source` that, in a real host, runs a subprocess: it counts a run once its
/// input is read, the way `urn:system:exec` spawns once it has a command.
fn exec_like(runs: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("exec", move |inv: &Invocation<'_>| {
        let cmd = inv.inline_str("cmd")?;
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(text(format!("ran {cmd}")))
    })
    .with_description(
        Description::new("exec")
            .verb(Verb::Source)
            .input(ArgSpec::new("cmd").class(XSD_STRING))
            .output("text/plain"),
    )
}

/// A `Sink` declaring no capability and no input, that in a real host sends a
/// message: ENFORCED's ungated branch resolves it under no grants, for real.
fn notify_like(runs: Arc<AtomicUsize>) -> FnEndpoint {
    FnEndpoint::new("notify", move |_inv: &Invocation<'_>| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(text("sent"))
    })
    .with_description(
        Description::new("notify")
            .verb(Verb::Sink)
            .output("text/plain"),
    )
}

#[test]
fn a_default_walk_runs_what_the_endpoints_run() {
    let (exec, notify) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:exec"), exec_like(exec.clone()))
        .bind(
            Exact::new("urn:example:notify"),
            notify_like(notify.clone()),
        );
    let report = report_of(space);
    // The hazard, counted. The subprocess ran TWICE: once under root with the
    // suite's `x` (the minimal resolution), once more under a capability holding no
    // grants (ENFORCED's probe; it declares nothing, so nothing refused it). The
    // message went out once, under no grants, and AUTHORITY says so after the fact.
    assert_eq!(exec.load(Ordering::SeqCst), 2, "{report}");
    assert_eq!(notify.load(Ordering::SeqCst), 1, "{report}");
    assert_caught(
        &report,
        Check::Authority,
        "mutated under a capability holding no grants",
    );
}

#[test]
fn a_static_walk_reads_every_description_and_runs_nothing() {
    let (exec, notify) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:exec"), exec_like(exec.clone()))
        .bind(
            Exact::new("urn:example:notify"),
            notify_like(notify.clone()),
        )
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        );
    let report = Suite::new()
        .checks(Checks::STATIC)
        .run_blocking(&kernel(space));
    assert_eq!(exec.load(Ordering::SeqCst), 0, "{report}");
    assert_eq!(notify.load(Ordering::SeqCst), 0, "{report}");
    // Every description was still read: the fix list is in hand before anything ran.
    assert_eq!(&*report.walked, ["exec", "notify", "toUpper"]);
    assert_caught(&report, Check::Names, "id `toUpper`");
    assert_caught(&report, Check::ArgSpecs, "input `in` has no class");
    let text = report.to_string();
    assert!(
        text.contains("probed: nothing — no action was resolved"),
        "{text}"
    );
    assert!(
        text.contains(
            "skipped: ENFORCED AUTHORITY OUTPUTS SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE REQUIRED"
        ),
        "{text}"
    );
}

// ----- Suite::own (ledger #328) -------------------------------------------------

#[test]
fn without_own_the_only_lever_on_a_dependency_keeps_its_static_findings() {
    // What three composing hosts worked around by hand: `opt_out` drops the
    // invoking checks and keeps the static ones, so a dependency's findings stay in
    // the module's report.
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        );
    let report = Suite::new()
        .pure("upper")
        .opt_out("toUpper", None, "a dependency's endpoint, not ours")
        .run_blocking(&kernel(space));
    assert!(report.against("toUpper").count() >= 2, "{report}");
    assert!(report.against("upper").next().is_none(), "{report}");
}

#[test]
fn own_scopes_the_walk_and_reports_the_rest_as_inherited() {
    let exec = Arc::new(AtomicUsize::new(0));
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        )
        .bind(Exact::new("urn:example:exec"), exec_like(exec.clone()));
    let report = Suite::new()
        .own("upper")
        .pure("upper")
        .run_blocking(&kernel(space));
    report.assert_clean();
    assert_eq!(&*report.walked, ["upper"]);
    assert_eq!(report.declared.inherited, ["toUpper", "exec"]);
    assert_eq!(report.endpoints, 1);
    // Inherited means NOT walked: the dependency's subprocess never ran.
    assert_eq!(exec.load(Ordering::SeqCst), 0, "{report}");
    let text = report.to_string();
    assert!(text.contains("own: upper (1 endpoint(s) walked)"), "{text}");
    assert!(text.contains("inherited: toUpper exec — "), "{text}");

    // A prefix ends in `*`.
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        );
    let report = Suite::new()
        .own("up*")
        .pure("upper")
        .run_blocking(&kernel(space));
    report.assert_clean();
    assert_eq!(report.declared.inherited, ["toUpper"]);
}

#[test]
fn an_own_that_matches_nothing_is_a_finding_no_selection_removes() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .checks(Checks::NAMES)
        .opt_out_check("(suite)", Check::Declarations, "trying to waive it")
        .own("uper")
        .run_blocking(&kernel(space));
    assert!(!report.is_clean(), "{report}");
    assert_eq!(report.endpoints, 0, "{report}");
    assert_caught(
        &report,
        Check::Declarations,
        "`Suite::own(\"uper\")` matched no endpoint",
    );
    assert_caught(&report, Check::Declarations, "The kernel binds: `upper`");
}

#[test]
fn a_declaration_for_an_inherited_endpoint_says_it_is_inherited() {
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        );
    let report = Suite::new()
        .own("upper")
        .pure("upper")
        .fixture(Fixture::new("toUpper", Verb::Source).arg("in", "x"))
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "it is inherited (no `Suite::own` matches it)",
    );
}

// ----- Suite::inherit, Suite::own_all, Finding::is_suite_wide (ledger #1497) ---------

/// `upper` is the module's; `toUpper` and `exec` are bound beside it.
fn composed(exec: Arc<AtomicUsize>) -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        )
        .bind(Exact::new("urn:example:exec"), exec_like(exec))
}

#[test]
fn inherit_pins_the_inherited_list_and_prints_each_reason() {
    let exec = Arc::new(AtomicUsize::new(0));
    // Declared in the other order from the catalog: the pin compares as a set.
    let report = Suite::new()
        .own("upper")
        .pure("upper")
        .inherit("exec", "a subprocess seam the host owns")
        .inherit("toUpper", "ikigai-core's builtin, covered by its own tests")
        .run_blocking(&kernel(composed(exec.clone())));
    report.assert_clean();
    assert_eq!(report.declared.inherited, ["toUpper", "exec"]);
    assert_eq!(exec.load(Ordering::SeqCst), 0, "{report}");
    assert_eq!(
        report.inherited_because(),
        [
            Inherited {
                endpoint: "exec".into(),
                reason: "a subprocess seam the host owns".into(),
            },
            Inherited {
                endpoint: "toUpper".into(),
                reason: "ikigai-core's builtin, covered by its own tests".into(),
            },
        ]
    );
    let text = report.to_string();
    for line in [
        "inherited because: exec: a subprocess seam the host owns\n",
        "inherited because: toUpper: ikigai-core's builtin, covered by its own tests\n",
    ] {
        assert!(text.contains(line), "missing {line:?}:\n{text}");
    }
}

#[test]
fn an_inherited_endpoint_no_inherit_names_is_a_finding_no_selection_removes() {
    let exec = Arc::new(AtomicUsize::new(0));
    let report = Suite::new()
        .checks(Checks::NAMES)
        .opt_out_check("exec", Check::Declarations, "trying to waive it")
        .own("upper")
        .inherit("toUpper", "ikigai-core's builtin")
        .run_blocking(&kernel(composed(exec)));
    let pinned: Vec<&Finding> = report
        .of(Check::Declarations)
        .filter(|f| f.detail.contains("no `Suite::inherit` gives the reason"))
        .collect();
    assert_eq!(pinned.len(), 1, "{report}");
    assert_eq!(pinned[0].endpoint, "exec");
    assert!(
        pinned[0]
            .detail
            .contains("`Suite::inherit(\"exec\", \"<why it is not the module's own>\")`"),
        "{report}"
    );
    // Without any `inherit`, the inherited list is not pinned (0.8.0 behavior).
    let report = Suite::new()
        .own("upper")
        .pure("upper")
        .run_blocking(&kernel(composed(Arc::new(AtomicUsize::new(0)))));
    report.assert_clean();
}

#[test]
fn an_inherit_for_an_id_that_is_not_inherited_is_a_finding() {
    let exec = Arc::new(AtomicUsize::new(0));
    let reasons = |report: &Report, id: &str| -> Vec<String> {
        report
            .of(Check::Declarations)
            .filter(|f| f.endpoint == id && f.detail.contains("for an id that is not inherited"))
            .map(|f| f.detail.clone())
            .collect()
    };
    // Owned by a pattern: walked, so not inherited.
    let report = Suite::new()
        .checks(Checks::NAMES)
        .own("upper")
        .inherit("toUpper", "a dependency")
        .inherit("exec", "a seam")
        .inherit("upper", "a stale reason")
        .run_blocking(&kernel(composed(exec.clone())));
    let found = reasons(&report, "upper");
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains("`Suite::own(\"upper\")` matches it, so it is the module's own"),
        "{report}"
    );
    // No `own` at all: nothing is inherited.
    let report = Suite::new()
        .checks(Checks::NAMES)
        .inherit("toUpper", "a dependency")
        .run_blocking(&kernel(composed(exec.clone())));
    let found = reasons(&report, "toUpper");
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].contains("no `Suite::own` is declared"), "{report}");
    // An id nothing binds.
    let report = Suite::new()
        .checks(Checks::NAMES)
        .own("upper")
        .inherit("toUpper", "a dependency")
        .inherit("exec", "a seam")
        .inherit("to-upper", "a typo")
        .run_blocking(&kernel(composed(exec)));
    let found = reasons(&report, "to-upper");
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains("the kernel binds no endpoint with that id"),
        "{report}"
    );
}

#[test]
fn an_opt_out_left_beside_own_is_a_finding_that_points_at_inherit() {
    // Migrating to `own` means deleting the opt-outs it replaces (ledger #1497).
    let space = EndpointSpace::new()
        .bind(Exact::new("urn:example:upper"), conforming())
        .bind(
            Exact::new("urn:example:toUpper"),
            ikigai_core::builtins::to_upper(),
        );
    let report = Suite::new()
        .own("upper")
        .pure("upper")
        .opt_out("toUpper", None, "a dependency's endpoint, not ours")
        .run_blocking(&kernel(space));
    assert_caught(
        &report,
        Check::Declarations,
        "opted out of the invoking checks (a dependency's endpoint, not ours) but excluded \
         nothing: it is inherited",
    );
    assert_caught(
        &report,
        Check::Declarations,
        "delete the declaration (an opt-out's reason belongs in `Suite::inherit`)",
    );
}

#[test]
fn own_all_is_own_for_each_id() {
    const OURS: [&str; 2] = ["upper", "exec"];
    let exec = Arc::new(AtomicUsize::new(0));
    let folded = OURS
        .iter()
        .fold(Suite::new().checks(Checks::NAMES), |suite, id| {
            suite.own(*id)
        })
        .run_blocking(&kernel(composed(exec.clone())));
    let all = Suite::new()
        .checks(Checks::NAMES)
        .own_all(OURS)
        .run_blocking(&kernel(composed(exec.clone())));
    assert_eq!(all.declared.own, OURS);
    assert_eq!(all.declared.own, folded.declared.own);
    assert_eq!(&*all.walked, ["upper", "exec"]);
    assert_eq!(all.declared.inherited, ["toUpper"]);
    // Owned Strings and a slice reference work too.
    let owned: Vec<String> = OURS.iter().map(|s| s.to_string()).collect();
    let report = Suite::new()
        .checks(Checks::NAMES)
        .own_all(&owned)
        .run_blocking(&kernel(composed(exec)));
    assert_eq!(report.declared.own, OURS);
}

#[test]
fn a_suite_wide_finding_says_so() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), conforming());
    let report = Suite::new()
        .checks(Checks::NAMES)
        .own("uper")
        .run_blocking(&kernel(space));
    let suite_wide: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|f| f.is_suite_wide())
        .collect();
    assert_eq!(suite_wide.len(), 1, "{report}");
    assert_eq!(suite_wide[0].endpoint, Finding::SUITE_WIDE);
    assert_eq!(Finding::SUITE_WIDE, "(suite)");
    assert!(
        report.findings.iter().all(|f| f.is_suite_wide()),
        "{report}"
    );
}

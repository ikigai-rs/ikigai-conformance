//! DATATYPES (ledger #356): a module's hand-written Turtle face and the JSON-LD
//! context's coercion of the same property are two independent decisions about one
//! literal, and only the context is generated from `rdfs:range`. Every literal in a
//! probed face is held to its property's declared range.
//!
//! Falsified both ways: the shapes that silently disagree with the context are
//! findings, and the shapes that agree (a bare number whose range IS `xsd:integer`, a
//! tagged literal, a JSON-LD face coerced by the context itself, core's own Meta face)
//! are not.

use std::sync::Arc;

use ikigai_conformance::rdf::{datatype_mismatches, parse};
use ikigai_conformance::{Check, Checks, Report, Suite};
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const PREFIXES: &str = "@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

/// A pure `Source` serving `body` as its one face.
fn face(id: &'static str, media_type: &'static str, body: String) -> FnEndpoint {
    FnEndpoint::new(id, move |_: &Invocation<'_>| {
        Ok(Representation::new(ReprType::new(media_type), body.clone().into_bytes()).cacheable())
    })
    .with_description(Description::new(id).verb(Verb::Source).output(media_type))
}

fn turtle(id: &'static str, triples: &str) -> FnEndpoint {
    face(id, "text/turtle", format!("{PREFIXES}{triples}"))
}

fn run(suite: Suite, endpoints: Vec<(&'static str, FnEndpoint)>) -> Report {
    let mut space = EndpointSpace::new();
    let mut suite = suite;
    for (id, endpoint) in endpoints {
        space = space.bind(Exact::new(format!("urn:example:{id}")), endpoint);
        suite = suite.pure(id);
    }
    suite.run_blocking(&Kernel::new(Arc::new(space)))
}

fn datatypes(report: &Report) -> Vec<String> {
    report
        .of(Check::Datatypes)
        .map(|f| f.detail.clone())
        .collect()
}

/// The founding instance: ikigai-llm's `ik:batchAt 2`, a bare `xsd:integer` where the
/// context (driven by `rdfs:range xsd:positiveInteger`) coerces to `xsd:positiveInteger`.
#[test]
fn a_bare_number_whose_range_the_shorthand_cannot_express_is_a_finding() {
    let report = run(
        Suite::new(),
        vec![(
            "backend",
            turtle(
                "backend",
                "<urn:llm:a> a ik:LlmBackend ; ik:batchAt 2 ; ik:context 8192 .",
            ),
        )],
    );
    let found = datatypes(&report);
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains("writes `ik:batchAt` as `xsd:integer`")
            && found[0].contains("rdfs:range is `xsd:positiveInteger`")
            && found[0].contains("Tag it `^^xsd:positiveInteger`")
            && found[0].contains("untagged Turtle literal"),
        "{report}"
    );
    assert_eq!(
        report.findings.len(),
        1,
        "nothing else is wrong with it:\n{report}"
    );
}

/// "Tag what the shorthand cannot express", not "tag everything".
#[test]
fn a_bare_number_whose_range_is_xsd_integer_and_a_tagged_literal_are_clean() {
    let report = run(
        Suite::new(),
        vec![(
            "backend",
            turtle(
                "backend",
                "<urn:llm:a> a ik:LlmBackend ; ik:context 8192 ; ik:jsonMode true ;\n\
                   ik:batchAt \"2\"^^xsd:positiveInteger .",
            ),
        )],
    );
    assert!(report.is_clean(), "{report}");
    assert!(
        report.to_string().contains("DATATYPES"),
        "it ran:\n{report}"
    );
}

/// A mismatch that is not a shorthand's: an explicit wrong tag, and a plain string
/// where the range is `xsd:dateTime`. Each (property, datatype) is one finding,
/// however many literals share it.
#[test]
fn an_explicit_wrong_tag_and_a_plain_string_are_findings_once_per_shape() {
    let report = run(
        Suite::new(),
        vec![(
            "plan",
            turtle(
                "plan",
                "<urn:x:a> ik:sizeBytes \"10\"^^xsd:integer ; ik:derivedAt \"2026-10-23T00:00:00Z\" .\n\
                 <urn:x:b> ik:sizeBytes \"20\"^^xsd:integer .",
            ),
        )],
    );
    let found = datatypes(&report);
    assert_eq!(found.len(), 2, "{report}");
    assert!(
        found
            .iter()
            .any(|d| d.contains("`ik:sizeBytes` as `xsd:integer`")
                && d.contains("`xsd:nonNegativeInteger`")),
        "{report}"
    );
    assert!(
        found
            .iter()
            .any(|d| d.contains("`ik:derivedAt` as `xsd:string`") && d.contains("`xsd:dateTime`")),
        "{report}"
    );
}

/// The JSON-LD face coerced by `ikigai_vocab::CONTEXT` agrees with the range by
/// construction; the same graph's Turtle face with a bare `2` is the one that
/// disagrees. Two faces of one graph, one finding, on the hand-written side.
#[test]
fn a_json_ld_face_coerced_by_the_vocab_context_agrees_and_its_bare_turtle_twin_does_not() {
    let context = vocab_context();
    let jsonld =
        format!(r#"{{"@context": {context}, "@id": "urn:llm:a", "batchAt": 2, "context": 8192}}"#);
    let coerced = parse("application/ld+json", jsonld.as_bytes()).unwrap();
    assert!(datatype_mismatches(&coerced).is_empty(), "{coerced:?}");
    let bare = parse(
        "text/turtle",
        format!("{PREFIXES}<urn:llm:a> ik:batchAt 2 ; ik:context 8192 .").as_bytes(),
    )
    .unwrap();
    assert_eq!(datatype_mismatches(&bare).len(), 1);

    let report = run(
        Suite::new(),
        vec![("jsonld", face("jsonld", "application/ld+json", jsonld))],
    );
    assert!(report.of(Check::Datatypes).next().is_none(), "{report}");
}

/// `ikigai_vocab::CONTEXT`'s inner `@context` object, as JSON text, so a face can
/// embed it inline and parse without a network fetch.
fn vocab_context() -> String {
    let ctx = ikigai_vocab::CONTEXT;
    let open = ctx.find("\"@context\"").expect("a @context key");
    let start = open + ctx[open..].find('{').expect("its object");
    let mut depth = 0usize;
    for (i, c) in ctx[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return ctx[start..=start + i].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("an unbalanced @context")
}

/// Core's own Meta face (through `ikigai_vocab::to_turtle`, the emitter a description's
/// Turtle comes from) tags what its ranges need: `ik:required`, `ik:enumerated`,
/// `ik:order` and the rest agree with the context.
#[test]
fn the_vocab_emitters_meta_face_agrees_with_every_range() {
    let description = Description::new("greet")
        .verb(Verb::Source)
        .verb(Verb::Meta)
        .input(ArgSpec::new("name").class(XSD_STRING))
        .input(
            ArgSpec::new("tone")
                .class(XSD_STRING)
                .optional()
                .one_of(["warm", "dry"]),
        )
        .output("text/plain");
    let face = parse(
        "text/turtle",
        ikigai_vocab::to_turtle(&description).as_bytes(),
    )
    .unwrap();
    assert!(face.len() > 5, "a face with something in it");
    assert_eq!(datatype_mismatches(&face), vec![]);
}

#[test]
fn it_is_waivable_per_endpoint_and_leaves_with_the_rdf_set() {
    let bad = || turtle("backend", "<urn:llm:a> ik:batchAt 2 .");

    let waived = run(
        Suite::new().opt_out_check(
            "backend",
            Check::Datatypes,
            "ikigai-llm tags it next release",
        ),
        vec![("backend", bad())],
    );
    assert!(waived.of(Check::Datatypes).next().is_none(), "{waived}");
    assert!(waived.is_clean(), "{waived}");

    let without = run(
        Suite::new().checks(Checks::all() - Checks::RDF),
        vec![("backend", bad())],
    );
    assert!(without.of(Check::Datatypes).next().is_none(), "{without}");
    assert!(
        without.to_string().contains("DATATYPES"),
        "listed as skipped:\n{without}"
    );
}

/// Selected alone, it still reports a face it could not read, under its own name, as
/// SKOLEM-RDF and VOCABULARY do: a report never loses the face to the selection.
#[test]
fn selected_alone_it_reports_an_unparseable_face_itself() {
    let report = run(
        Suite::new().checks(Checks::DATATYPES),
        vec![("broken", turtle("broken", "<urn:x:a> ik:batchAt "))],
    );
    let found = datatypes(&report);
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].contains("does not parse as text/turtle"),
        "{report}"
    );
}

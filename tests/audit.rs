//! Adversarial modules for ledger #3477. Each regression isolates one rule.
use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Suite};
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

fn face(media_type: &'static str, body: &'static str) -> FnEndpoint {
    FnEndpoint::new("graph", move |_: &Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new(media_type),
            body.as_bytes().to_vec(),
        ))
    })
    .with_description(
        Description::new("graph")
            .verb(Verb::Source)
            .output(media_type),
    )
}

#[test]
fn skolem_rdf_rejects_a_blank_graph_name() {
    for (media_type, body) in [
        (
            "application/n-quads",
            "<urn:s> <http://purl.org/dc/terms/title> \"x\" _:graph .",
        ),
        (
            "application/trig",
            "_:graph { <urn:s> <http://purl.org/dc/terms/title> \"x\" . }",
        ),
        (
            "application/ld+json",
            r#"{"@id":"_:graph","@graph":[{"@id":"urn:s","http://purl.org/dc/terms/title":[{"@value":"x"}]}]}"#,
        ),
    ] {
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:example:graph"), face(media_type, body)),
        ));
        let report = Suite::new().run_blocking(&kernel);
        let findings: Vec<_> = report.of(Check::SkolemRdf).collect();
        assert_eq!(findings.len(), 1, "{media_type}:\n{report}");
        assert_eq!(
            report.findings.len(),
            1,
            "only skolemization is wrong:\n{report}"
        );
        assert_eq!(findings[0].endpoint, "graph");
        assert_eq!(findings[0].verb, Some(Verb::Source));
        assert!(
            findings[0].detail.contains("has 1 blank node(s)"),
            "{report}"
        );
    }
}

#[test]
fn argspecs_checks_distinct_descriptions_even_when_their_ids_match() {
    let endpoint = |typed| {
        FnEndpoint::new("shared", |inv: &Invocation<'_>| {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                inv.inline_str("in")?.as_bytes().to_vec(),
            ))
        })
        .with_description(
            Description::new("shared")
                .verb(Verb::Source)
                .input(if typed {
                    ArgSpec::new("in").class(XSD_STRING)
                } else {
                    ArgSpec::new("in")
                })
                .output("text/plain"),
        )
    };
    for first_typed in [true, false] {
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new()
                .bind(Exact::new("urn:example:first"), endpoint(first_typed))
                .bind(Exact::new("urn:example:second"), endpoint(!first_typed)),
        ));
        let report = Suite::new().checks(Checks::ARGSPECS).run_blocking(&kernel);
        assert!(
            report
                .of(Check::ArgSpecs)
                .any(|f| f.detail.contains("input `in` has no class")),
            "a different description with the same id hid an untyped input:\n{report}"
        );
    }
}

#[test]
fn skolem_rdf_accepts_named_graphs_and_deduplicates_blank_nodes_across_positions() {
    for (body, expected) in [
        (
            "<urn:s> <http://purl.org/dc/terms/title> \"x\" <urn:graph> .",
            0,
        ),
        (
            "_:same <http://purl.org/dc/terms/relation> _:same _:same .",
            1,
        ),
    ] {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:example:graph"),
            face("application/n-quads", body),
        )));
        let report = Suite::new().run_blocking(&kernel);
        assert_eq!(report.of(Check::SkolemRdf).count(), expected, "{report}");
        if expected == 0 {
            report.assert_clean();
        } else {
            assert!(
                report.findings[0].detail.contains("has 1 blank node(s)"),
                "{report}"
            );
        }
    }
}

#[test]
fn identical_descriptions_at_two_bindings_report_a_static_violation_once() {
    let endpoint = || {
        FnEndpoint::new("shared", |_: &Invocation<'_>| {
            Ok(Representation::new(
                ReprType::new("text/plain"),
                b"x".to_vec(),
            ))
        })
        .with_description(
            Description::new("shared")
                .verb(Verb::Source)
                .input(ArgSpec::new("in")),
        )
    };
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:example:first"), endpoint())
            .bind(Exact::new("urn:example:second"), endpoint()),
    ));
    let report = Suite::new().checks(Checks::ARGSPECS).run_blocking(&kernel);
    assert_eq!(report.of(Check::ArgSpecs).count(), 1, "{report}");
    assert_eq!(report.endpoints, 1);
    assert_eq!(report.actions, 2);
}

#[test]
fn every_unselected_check_is_explicitly_skipped_in_the_report() {
    for selected in Check::ALL {
        let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(
            Exact::new("urn:example:graph"),
            face(
                "text/turtle",
                "<urn:s> <http://purl.org/dc/terms/title> \"x\" .",
            ),
        )));
        let report = Suite::new().checks(selected.into()).run_blocking(&kernel);
        let text = report.to_string();
        assert!(
            text.lines().any(|l| l == format!("checked: {selected}")),
            "{text}"
        );
        let skipped = text
            .lines()
            .find_map(|l| l.strip_prefix("skipped: "))
            .unwrap();
        let labels: Vec<_> = skipped.split_whitespace().collect();
        assert!(!labels.contains(&selected.label()), "{text}");
        for check in Check::ALL.into_iter().filter(|c| *c != selected) {
            assert!(
                labels.contains(&check.label()),
                "{check} silently skipped:\n{text}"
            );
        }
    }
}

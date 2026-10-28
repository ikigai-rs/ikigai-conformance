//! A reads_body declaration must hold for every supported transport (ledger #2457).
use ikigai_conformance::Suite;
use ikigai_core::{
    ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Kernel, ReprType, Representation, Verb,
};
use std::sync::Arc;

#[test]
fn false_body_reader_is_reported() {
    let endpoint = FnEndpoint::new("liar", |inv| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            inv.inline_str("payload")?.as_bytes().to_vec(),
        )
        .cacheable())
    })
    .with_description(
        Description::new("liar")
            .verb(Verb::Source)
            .input(
                ArgSpec::new("payload")
                    .class("http://www.w3.org/2001/XMLSchema#string")
                    .reads_body(),
            )
            .output("text/plain"),
    );
    let space = Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:liar"), endpoint));
    let store = Arc::new(MemoryContentStore::new(1024));
    let kernel = Kernel::new(space.clone()).with_content_store(store.clone());
    let report = Suite::new()
        .body_input_store(store)
        .body_space(space)
        .pure("liar")
        .run_blocking(&kernel);
    assert_eq!(report.of(Check::BodyInputs).count(), 3, "{report}");
    for transport in ["reference", "stored content id", "one-shot attached body"] {
        assert!(
            report
                .of(Check::BodyInputs)
                .any(|f| f.detail.contains("reads_body") && f.detail.contains(transport)),
            "{report}"
        );
    }
}

use ikigai_conformance::{Check, Checks, Fixture, Report};
use ikigai_core::{ArgRef, AsyncFnEndpoint, Capability, ContentId, Error, MemoryContentStore};
use std::sync::Mutex;

type Calls = Arc<Mutex<Vec<(Verb, String, &'static str)>>>;

#[derive(Clone, Copy)]
enum Behavior {
    Honest,
    WrongBytes,
    WrongMedia,
    Rejects,
    BaselineFails,
}

fn reader(
    behavior: Behavior,
    transport: &'static str,
    calls: Calls,
    verb: Verb,
) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("reader", move |inv| {
        let calls = calls.clone();
        Box::pin(async move {
            let mut output = Vec::new();
            let mut differs = false;
            for name in ["payload", "other"] {
                let arg = inv.request.args.get(name).unwrap();
                let form = match arg {
                    ArgRef::Inline(_) => "inline",
                    ArgRef::Reference(_) => "reference",
                    ArgRef::Content(id) if *id == ContentId::of(&sample(name)) => {
                        "stored content id"
                    }
                    ArgRef::Content(_) => "one-shot attached body",
                };
                calls
                    .lock()
                    .unwrap()
                    .push((inv.request.verb, name.to_string(), form));
                differs |= form == transport;
                if matches!(behavior, Behavior::BaselineFails)
                    || (differs && matches!(behavior, Behavior::Rejects))
                {
                    return Err(Error::Endpoint("reader refused".into()));
                }
                let body = inv.content_body(name).await?;
                output.extend_from_slice(&inv.collect(body).await?);
            }
            // Other arguments and the fixture's caller must survive every variant.
            output.extend_from_slice(inv.inline_arg("salt")?);
            assert!(!inv.capability.is_root());
            assert!(inv.capability.allows("urn:cap:reader"));
            let media = if differs && matches!(behavior, Behavior::WrongMedia) {
                "application/octet-stream"
            } else {
                "text/plain"
            };
            if differs && matches!(behavior, Behavior::WrongBytes) {
                output.push(0xff);
            }
            // The one-shot result is uncacheable even though the endpoint asks for caching.
            Ok(Representation::new(ReprType::new(media), output).cacheable())
        })
    })
    .with_description(
        Description::new("reader")
            .verb(verb)
            .input(ArgSpec::new("payload").reads_body())
            .input(ArgSpec::new("other").optional().reads_body())
            .input(ArgSpec::new("salt"))
            .output("text/plain"),
    )
}

fn sample(name: &str) -> Vec<u8> {
    if name == "payload" {
        vec![0, 0xff, 0x80, b'\n', 42]
    } else {
        Vec::new()
    }
}

fn run_reader(
    behavior: Behavior,
    transport: &'static str,
    verb: Verb,
    configure: impl FnOnce(Suite) -> Suite,
    provide_store: bool,
) -> (Report, Calls) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(MemoryContentStore::new(1024));
    let space = Arc::new(EndpointSpace::new().bind(
        Exact::new("urn:reader"),
        reader(behavior, transport, calls.clone(), verb),
    ));
    let kernel = Kernel::new(space).with_content_store(store.clone());
    let mut suite = Suite::new().checks(Checks::BODY_INPUTS).fixture(
        Fixture::new("reader", verb)
            .arg_bytes("payload", sample("payload"))
            .arg_bytes("other", sample("other"))
            .arg("salt", "pepper")
            .capability(Capability::scoped(["urn:cap:reader"])),
    );
    if provide_store {
        suite = suite.body_input_store(store.clone());
    }
    let report = configure(suite).run_blocking(&kernel);
    assert_eq!(store.held_bytes(), 0, "probe leaked its content handle");
    (report, calls)
}

#[test]
fn honest_reader_matches_all_transports_on_every_verb_and_argument() {
    for verb in [Verb::Source, Verb::Exists, Verb::Sink, Verb::Delete] {
        let (report, calls) = run_reader(Behavior::Honest, "", verb, |s| s, true);
        report.assert_clean();
        assert!(report.unprobed.is_empty(), "{report}");
        assert_eq!(report.declared.evidence.body_inputs.len(), 6, "{report}");
        assert!(report
            .to_string()
            .contains("one-shot attached body: identical to inline"));
        assert!(!report.to_string().contains("probed: nothing"));
        // Four independent calls per input, each reads both arguments.
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 16, "{calls:?}");
        for name in ["payload", "other"] {
            for transport in ["reference", "stored content id", "one-shot attached body"] {
                assert_eq!(
                    calls
                        .iter()
                        .filter(|(v, n, t)| *v == verb && n == name && *t == transport)
                        .count(),
                    1
                );
            }
        }
    }
}

#[test]
fn each_transport_is_held_to_bytes_media_type_and_success() {
    for transport in ["reference", "stored content id", "one-shot attached body"] {
        for behavior in [
            Behavior::WrongBytes,
            Behavior::WrongMedia,
            Behavior::Rejects,
        ] {
            let (report, _) = run_reader(behavior, transport, Verb::Source, |s| s, true);
            let findings: Vec<_> = report.of(Check::BodyInputs).collect();
            assert_eq!(findings.len(), 2, "{transport}: {report}");
            assert!(
                findings.iter().all(|f| f.detail.contains(transport)),
                "{report}"
            );
            for name in ["payload", "other"] {
                assert!(
                    findings
                        .iter()
                        .any(|f| f.detail.contains(&format!("`{name}`"))),
                    "{report}"
                );
            }
        }
    }
}

#[test]
fn missing_store_is_explicit_and_does_not_skip_other_transports() {
    let (report, _) = run_reader(Behavior::Honest, "", Verb::Source, |s| s, false);
    report.assert_clean();
    assert_eq!(report.declared.evidence.body_inputs.len(), 4);
    assert_eq!(report.unprobed.len(), 2);
    assert!(
        report
            .unprobed
            .iter()
            .all(|u| u.check == Check::BodyInputs && u.reason.contains("body_input_store")),
        "{report}"
    );
}

#[test]
fn failed_baselines_are_unprobed_and_no_variants_are_fired() {
    let (report, calls) = run_reader(Behavior::BaselineFails, "", Verb::Source, |s| s, true);
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert!(report.declared.evidence.body_inputs.is_empty());
    assert_eq!(report.unprobed.len(), 2, "{report}");
    assert!(report
        .unprobed
        .iter()
        .all(|u| u.reason.contains("inline baseline failed")));
}

#[test]
fn selection_and_action_opt_outs_prevent_probes() {
    assert!(Checks::all().contains(Check::BodyInputs));
    assert!(Checks::INVOKING.contains(Check::BodyInputs));
    for configure in [
        (|s: Suite| s.checks(Checks::STATIC)) as fn(Suite) -> Suite,
        |s| s.checks(Checks::all() - Checks::INVOKING),
        |s| s.opt_out("reader", None, "host owns the writes"),
        |s| s.opt_out_at("urn:reader", None, "host owns this binding"),
    ] {
        let (report, calls) = run_reader(Behavior::Honest, "", Verb::Sink, configure, true);
        assert!(calls.lock().unwrap().is_empty(), "{report}");
        assert!(report.of(Check::BodyInputs).next().is_none(), "{report}");
    }
}

#[test]
fn mutating_waivers_never_fire_for_idle_waiver_accounting() {
    let (report, calls) = run_reader(
        Behavior::Honest,
        "",
        Verb::Sink,
        |s| s.opt_out_check("reader", Check::BodyInputs, "requires fresh isolated state"),
        true,
    );
    assert!(calls.lock().unwrap().is_empty(), "{report}");
    assert!(report.declared.evidence.idle_waivers.is_empty(), "{report}");
}

#[test]
fn endpoint_without_reads_body_is_not_fired() {
    let endpoint = FnEndpoint::new("plain", |_| panic!("no body-input probe should fire"))
        .with_description(
            Description::new("plain")
                .verb(Verb::Source)
                .input(ArgSpec::new("content")),
        );
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:plain"), endpoint),
    ));
    let report = Suite::new()
        .checks(Checks::BODY_INPUTS)
        .run_blocking(&kernel);
    report.assert_clean();
    assert!(report.declared.evidence.body_inputs.is_empty());
    assert!(report.unprobed.is_empty());
}

#[test]
fn optional_body_inputs_are_probed_even_without_a_fixture() {
    for default in [None, Some("default value")] {
        let store = Arc::new(MemoryContentStore::new(1024));
        let mut input = ArgSpec::new("payload").optional().reads_body();
        if let Some(value) = default {
            input = input.default_value(value);
        }
        let endpoint = AsyncFnEndpoint::new("optional", |inv| {
            Box::pin(async move {
                let body = inv.content_body("payload").await?;
                Ok(Representation::new(
                    ReprType::new("text/plain"),
                    inv.collect(body).await?.to_vec(),
                )
                .cacheable())
            })
        })
        .with_description(Description::new("optional").verb(Verb::Source).input(input));
        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:optional"), endpoint),
        ))
        .with_content_store(store.clone());
        let report = Suite::new()
            .checks(Checks::BODY_INPUTS)
            .body_input_store(store)
            .run_blocking(&kernel);
        report.assert_clean();
        assert!(report.unprobed.is_empty(), "{report}");
        assert_eq!(report.declared.evidence.body_inputs.len(), 3);
        assert_eq!(report.probed[0].bytes, default.unwrap_or("x").len());
    }
}

#[test]
fn different_store_handle_cannot_produce_a_false_pass() {
    let different_store = Arc::new(MemoryContentStore::new(1024));
    let (report, _) = run_reader(
        Behavior::Honest,
        "",
        Verb::Source,
        |s| s.body_input_store(different_store),
        true,
    );
    assert_eq!(report.of(Check::BodyInputs).count(), 2, "{report}");
    assert!(
        report
            .of(Check::BodyInputs)
            .all(|f| f.detail.contains("stored content id failed")),
        "{report}"
    );
}

#[test]
fn read_waiver_suppresses_findings_and_is_not_reported_idle() {
    let (report, _) = run_reader(
        Behavior::WrongMedia,
        "reference",
        Verb::Source,
        |s| s.opt_out_check("reader", Check::BodyInputs, "deliberate fixture defect"),
        true,
    );
    report.assert_clean();
    assert!(report.declared.evidence.body_inputs.is_empty());
    assert!(report.declared.evidence.idle_waivers.is_empty(), "{report}");
}

//! Direct endpoint agreement and bounded collection (ledger #2249).
use std::sync::Arc;

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_conformance::{Checks, Suite};
use ikigai_core::{
    Body, Capability, Description, Endpoint, EndpointSpace, Exact, Invocation, Iri, Kernel,
    ReprType, Representation, Request, Result, Verb,
};

struct Disagrees;

#[async_trait]
impl Endpoint for Disagrees {
    async fn invoke(&self, _: &Invocation<'_>) -> Result<Representation> {
        Ok(Representation::new(ReprType::new("text/plain"), b"first".to_vec()).cacheable())
    }

    async fn invoke_body(&self, _: &Invocation<'_>) -> Result<Representation<Body>> {
        Ok(
            Representation::new(ReprType::new("text/plain"), b"other".to_vec())
                .cacheable()
                .into_body(),
        )
    }

    fn name(&self) -> &str {
        "disagrees"
    }

    fn describe(&self) -> Description {
        Description::new("disagrees")
            .verb(Verb::Source)
            .output("text/plain")
    }
}

#[test]
fn kernel_comparison_cannot_see_the_method_disagreement() {
    let space = Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:body"), Disagrees));
    let kernel = Kernel::new(space);
    let request = Request::new(Verb::Source, Iri::parse("urn:example:body").unwrap());
    let bytes = block_on(kernel.issue(request.clone(), &Capability::root())).unwrap();
    let body = block_on(kernel.issue_body(request, &Capability::root())).unwrap();
    assert_eq!(
        bytes.bytes,
        block_on(body.bytes.collect(1024)).unwrap().as_ref()
    );
    assert_eq!(bytes.bytes, b"other");
}

#[test]
fn endpoint_method_disagreement_is_reported() {
    let space = Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:body"), Disagrees));
    let kernel = Kernel::new(space.clone());
    let report = Suite::new()
        .body_space(space)
        .pure("disagrees")
        .checks(Checks::all())
        .run_blocking(&kernel);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.detail.contains("invoke_body") && f.detail.contains("offset 0")),
        "{report}"
    );
}

#[derive(Clone, Copy)]
enum Behavior {
    Honest,
    Truncates,
    OtherError,
    WrongType,
    Short,
    Volatile,
}

struct StreamEndpoint(Behavior);

#[async_trait]
impl Endpoint for StreamEndpoint {
    async fn invoke(&self, _: &Invocation<'_>) -> Result<Representation> {
        let bytes = if matches!(self.0, Behavior::Short) {
            &b"x"[..]
        } else {
            &b"four"[..]
        };
        Ok(Representation::new(ReprType::new("text/plain"), bytes.to_vec()).cacheable())
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<Body>> {
        let small = inv.memory_budget().is_some_and(|b| b.remaining() < 4);
        if small && matches!(self.0, Behavior::OtherError) {
            return Err(ikigai_core::Error::Endpoint("broken reader".into()));
        }
        let bytes = if matches!(self.0, Behavior::Short) {
            &b"x"[..]
        } else if small && matches!(self.0, Behavior::Truncates) {
            &b"fou"[..]
        } else {
            &b"four"[..]
        };
        let media = if matches!(self.0, Behavior::WrongType) {
            "application/octet-stream"
        } else {
            "text/plain"
        };
        let body = Body::once(futures::stream::iter([
            Ok(ikigai_core::Bytes::copy_from_slice(&bytes[..1])),
            Ok(ikigai_core::Bytes::copy_from_slice(&bytes[1..])),
        ]));
        let repr = Representation::from_body(ReprType::new(media), body);
        Ok(if matches!(self.0, Behavior::Volatile) {
            repr
        } else {
            repr.cacheable()
        })
    }

    fn name(&self) -> &str {
        "stream"
    }

    fn describe(&self) -> Description {
        Description::new("stream")
            .verb(Verb::Source)
            .output("text/plain")
    }
}

fn walk(endpoint: impl Endpoint + 'static, suite: Suite) -> ikigai_conformance::Report {
    let space = Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:body"), endpoint));
    let kernel = Kernel::new(space.clone());
    suite
        .body_space(space)
        .checks(Checks::BODIES)
        .run_blocking(&kernel)
}

#[test]
fn a_stream_that_stops_at_the_budget_is_reported_as_truncated() {
    let report = walk(StreamEndpoint(Behavior::Truncates), Suite::new());
    assert!(
        report
            .of(ikigai_conformance::Check::Bodies)
            .any(|f| f.detail.contains("truncated") && f.detail.contains("3 of 4")),
        "{report}"
    );
}

#[test]
fn an_honest_stream_passes_both_probes() {
    let report = walk(StreamEndpoint(Behavior::Honest), Suite::new());
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn a_default_body_wrapper_passes() {
    let endpoint = ikigai_core::FnEndpoint::new("buffer", |_| {
        Ok(Representation::new(ReprType::new("text/plain"), b"four".to_vec()).cacheable())
    })
    .with_description(
        Description::new("buffer")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let report = walk(endpoint, Suite::new());
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn a_volatile_answer_is_skipped_with_a_reason() {
    let report = walk(StreamEndpoint(Behavior::Volatile), Suite::new());
    report.assert_clean();
    assert!(
        report
            .unprobed
            .iter()
            .any(|u| u.check == ikigai_conformance::Check::Bodies && u.reason.contains("volatile")),
        "{report}"
    );
}

#[test]
fn a_media_type_disagreement_is_reported() {
    let report = walk(StreamEndpoint(Behavior::WrongType), Suite::new());
    assert!(
        report.of(ikigai_conformance::Check::Bodies).any(|f| f
            .detail
            .contains("media types `text/plain` versus `application/octet-stream`")),
        "{report}"
    );
}

#[test]
fn another_error_under_the_bound_is_named() {
    let report = walk(StreamEndpoint(Behavior::OtherError), Suite::new());
    assert!(report.of(ikigai_conformance::Check::Bodies).any(|f|
        f.detail.contains("expected OverBudget") && f.detail.contains("broken reader")), "{report}");
}

#[test]
fn a_one_byte_answer_only_needs_the_agreement_probe() {
    let report = walk(StreamEndpoint(Behavior::Short), Suite::new());
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn bodies_is_default_and_subtractable_and_waivable_with_a_reason() {
    use ikigai_conformance::Check;
    assert!(Checks::all().contains(Check::Bodies));
    assert!(!(Checks::all() - Checks::BODIES).contains(Check::Bodies));
    assert!(Checks::INVOKING.contains(Check::Bodies));
    let report = walk(
        Disagrees,
        Suite::new().opt_out_check(
            "disagrees",
            Check::Bodies,
            "fixture deliberately violates the body contract",
        ),
    );
    report.assert_clean();
    assert!(
        report
            .to_string()
            .contains("opted out: disagrees BODIES: fixture deliberately violates"),
        "{report}"
    );
}

#[test]
fn missing_module_space_is_explicitly_unprobed() {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:body"), Disagrees);
    let report = Suite::new()
        .checks(Checks::BODIES)
        .run_blocking(&Kernel::new(Arc::new(space)));
    report.assert_clean();
    assert!(
        report
            .unprobed
            .iter()
            .any(|u| u.check == ikigai_conformance::Check::Bodies
                && u.reason.contains("Suite::body_space")),
        "{report}"
    );
}

struct Composite;

#[async_trait]
impl Endpoint for Composite {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        assert_eq!(inv.bindings.get("name"), Some("chosen"));
        assert_eq!(inv.inline_str("suffix")?, "fixture");
        assert_eq!(inv.capability.principal(), Some("urn:example:alice"));
        let mut answer = inv.source(&Iri::parse("urn:example:part").unwrap()).await?;
        answer
            .bytes
            .extend_from_slice(inv.inline_str("suffix")?.as_bytes());
        Ok(answer.cacheable())
    }

    fn name(&self) -> &str {
        "composite"
    }

    fn describe(&self) -> Description {
        use ikigai_core::ArgSpec;
        Description::new("composite")
            .verb(Verb::Source)
            .requires("urn:cap:read")
            .input(ArgSpec::new("name").binding())
            .input(ArgSpec::new("suffix"))
            .output("text/plain")
    }
}

fn composite_report(volatile: bool) -> ikigai_conformance::Report {
    use ikigai_conformance::Fixture;
    use ikigai_core::{FnEndpoint, UriTemplate};
    let part = FnEndpoint::new("part", move |inv| {
        assert!(inv.capability.allows("urn:cap:read"));
        let repr = Representation::new(ReprType::new("text/plain"), b"part-".to_vec());
        Ok(if volatile { repr } else { repr.cacheable() })
    })
    .with_description(
        Description::new("part")
            .verb(Verb::Source)
            .requires("urn:cap:read")
            .output("text/plain"),
    );
    let space = Arc::new(
        EndpointSpace::new()
            .bind(
                UriTemplate::parse("urn:example:composite/{name}").unwrap(),
                Composite,
            )
            .bind(Exact::new("urn:example:part"), part),
    );
    let kernel = Kernel::new(space.clone());
    let caller = Capability::scoped(["urn:cap:read", "urn:cap:principal:urn:example:alice"]);
    Suite::new()
        .body_space(space)
        .checks(Checks::BODIES)
        .fixture(
            Fixture::new("composite", Verb::Source)
                .binding("name", "chosen")
                .arg("suffix", "fixture")
                .capability(caller),
        )
        .opt_out(
            "part",
            Some(Verb::Source),
            "exercised through the composite",
        )
        .run_blocking(&kernel)
}

#[test]
fn direct_invocations_keep_bindings_fixture_args_caller_and_subrequest_issuer() {
    let report = composite_report(false);
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn a_volatile_dependency_skips_the_composite_with_a_reason() {
    let report = composite_report(true);
    report.assert_clean();
    assert!(
        report.unprobed.iter().any(|u| u.endpoint == "composite"
            && u.check == ikigai_conformance::Check::Bodies
            && u.reason.contains("volatile")),
        "{report}"
    );
}

#[test]
fn direct_calls_receive_the_same_declared_defaults_as_kernel_calls() {
    use ikigai_core::{ArgSpec, FnEndpoint};
    let endpoint = FnEndpoint::new("defaults", |inv| {
        assert_eq!(inv.bindings.get("region"), Some("west"));
        assert_eq!(inv.inline_str("style")?, "plain");
        Ok(Representation::new(ReprType::new("text/plain"), b"four".to_vec()).cacheable())
    })
    .with_description(
        Description::new("defaults")
            .verb(Verb::Source)
            .input(
                ArgSpec::new("region")
                    .binding()
                    .optional()
                    .default_value("west"),
            )
            .input(ArgSpec::new("style").optional().default_value("plain"))
            .output("text/plain"),
    );
    let report = walk(endpoint, Suite::new());
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn default_injection_does_not_replace_a_fixture_argument() {
    use ikigai_conformance::Fixture;
    use ikigai_core::{ArgSpec, FnEndpoint};
    let endpoint = FnEndpoint::new("defaults", |inv| {
        assert_eq!(inv.inline_str("style")?, "fixture");
        Ok(Representation::new(ReprType::new("text/plain"), b"four".to_vec()).cacheable())
    })
    .with_description(
        Description::new("defaults")
            .verb(Verb::Source)
            .input(ArgSpec::new("style").optional().default_value("plain"))
            .output("text/plain"),
    );
    let report = walk(
        endpoint,
        Suite::new().fixture(Fixture::new("defaults", Verb::Source).arg("style", "fixture")),
    );
    report.assert_clean();
    assert!(report.unprobed.is_empty(), "{report}");
}

#[test]
fn byte_mismatches_name_the_first_offset_and_the_end_of_a_prefix() {
    struct Prefix;
    #[async_trait]
    impl Endpoint for Prefix {
        async fn invoke(&self, _: &Invocation<'_>) -> Result<Representation> {
            Ok(Representation::new(ReprType::new("text/plain"), b"four".to_vec()).cacheable())
        }
        async fn invoke_body(&self, _: &Invocation<'_>) -> Result<Representation<Body>> {
            Ok(
                Representation::new(ReprType::new("text/plain"), b"fou".to_vec())
                    .cacheable()
                    .into_body(),
            )
        }
        fn name(&self) -> &str {
            "prefix"
        }
        fn describe(&self) -> Description {
            Description::new("prefix")
                .verb(Verb::Source)
                .output("text/plain")
        }
    }
    let report = walk(Prefix, Suite::new());
    assert!(
        report
            .of(ikigai_conformance::Check::Bodies)
            .any(|f| f.detail.contains("endpoint `prefix`") && f.detail.contains("offset 3")),
        "{report}"
    );
}

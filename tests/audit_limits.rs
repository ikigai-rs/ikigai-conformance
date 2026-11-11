//! Ledger #3477: executable evidence for recipe gaps that need a policy decision.
//! These tests pin the observations, not a claim that these modules are conforming.
//! Do not widen the default gates here without deciding the adopter contract.
use std::sync::Arc;

use futures::executor::block_on;
use ikigai_conformance::{Check, Fixture, Suite};
use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, Description, EndpointSpace, Error, Exact,
    FailureClass, FnEndpoint, Invocation, Iri, Kernel, ReprType, Representation, Request, Verb,
};

const STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const TARGET: &str = "urn:example:audit";

fn text(body: &str) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.as_bytes().to_vec())
}

fn kernel(endpoint: FnEndpoint) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new(TARGET), endpoint),
    ))
}

fn request() -> Request {
    Request::new(Verb::Source, Iri::parse(TARGET).unwrap())
}

#[test]
fn a_one_of_the_endpoint_does_not_enforce_is_not_probed() {
    let endpoint = FnEndpoint::new("enumeration", |inv: &Invocation<'_>| {
        Ok(text(inv.inline_str("mode")?))
    })
    .with_description(
        Description::new("enumeration")
            .verb(Verb::Source)
            .input(ArgSpec::new("mode").class(STRING).one_of(["allowed"]))
            .output("text/plain"),
    );
    let kernel = kernel(endpoint);
    let bad = request().with_arg("mode", ArgRef::Inline(b"outside".to_vec()));
    assert_eq!(
        block_on(kernel.issue(bad, &Capability::root()))
            .unwrap()
            .bytes,
        b"outside"
    );
    let report = Suite::new().run_blocking(&kernel);
    report.assert_clean();
    // REQUIRED drops the input, but never supplies a value outside the enumeration.
    assert_eq!(report.required_probes().len(), 1);
}

#[test]
fn the_kernel_applies_a_default_without_an_endpoint_fallback() {
    let endpoint = FnEndpoint::new("default", |inv: &Invocation<'_>| {
        // No default handling in the module: the kernel supplies it before dispatch.
        Ok(text(inv.inline_str("mode")?))
    })
    .with_description(
        Description::new("default")
            .verb(Verb::Source)
            .input(
                ArgSpec::new("mode")
                    .class(STRING)
                    .optional()
                    .default_value("declared"),
            )
            .output("text/plain"),
    );
    let kernel = kernel(endpoint);
    assert_eq!(
        block_on(kernel.issue(request(), &Capability::root()))
            .unwrap()
            .bytes,
        b"declared"
    );
    Suite::new().run_blocking(&kernel).assert_clean();
}

#[test]
fn a_flat_requires_shadowed_by_an_explicit_read_action_is_not_a_gate() {
    let endpoint = FnEndpoint::new("shadowed", |_: &Invocation<'_>| Ok(text("public")))
        .with_description(
            Description::new("shadowed")
                .requires("urn:cap:example:read")
                .action(ActionSpec::new(Verb::Source).output("text/plain")),
        );
    let kernel = kernel(endpoint);
    let none = Capability::scoped(Vec::<String>::new());
    assert!(block_on(kernel.issue(request(), &none)).is_ok());
    Suite::new().run_blocking(&kernel).assert_clean();
}

#[test]
fn a_source_with_no_required_input_is_allowed_to_be_named_only() {
    let endpoint = FnEndpoint::new("named", |_: &Invocation<'_>| Ok(text("named only")))
        .with_description(
            Description::new("named")
                .verb(Verb::Source)
                .output("text/plain"),
        );
    Suite::new().run_blocking(&kernel(endpoint)).assert_clean();
}

#[test]
fn an_uncacheable_threadless_answer_is_allowed() {
    let endpoint = FnEndpoint::new("live", |_: &Invocation<'_>| Ok(text("live"))).with_description(
        Description::new("live")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let kernel = kernel(endpoint);
    let answer = block_on(kernel.issue(request(), &Capability::root())).unwrap();
    assert!(answer.threads().is_empty());
    assert_eq!(answer.expiry, ikigai_core::Expiry::Always);
    Suite::new()
        .live("live")
        .run_blocking(&kernel)
        .assert_clean();
}

#[test]
fn a_mistyped_capability_refusal_needs_an_explicit_probe() {
    let endpoint = FnEndpoint::new("wrong-error", |inv: &Invocation<'_>| {
        if !inv.capability.allows("urn:cap:example:read") {
            return Err(Error::InvalidArgument {
                name: "authority".into(),
                detail: "no grant".into(),
            });
        }
        Ok(text("private"))
    })
    .with_description(
        Description::new("wrong-error")
            .verb(Verb::Source)
            .output("text/plain"),
    );
    let kernel = kernel(endpoint);
    let none = Capability::scoped(Vec::<String>::new());
    assert!(matches!(
        block_on(kernel.issue(request(), &none)),
        Err(Error::InvalidArgument { .. })
    ));
    Suite::new().run_blocking(&kernel).assert_clean();
    let report = Suite::new()
        .enforced_under(Fixture::new("wrong-error", Verb::Source).capability(none))
        .run_blocking(&kernel);
    assert!(report.of(Check::Enforced).next().is_some(), "{report}");
}

#[test]
fn an_untyped_error_on_an_unvisited_input_needs_an_explicit_probe() {
    let endpoint = FnEndpoint::new("opaque", |inv: &Invocation<'_>| {
        let input = inv.inline_str("in")?;
        if input == "bad" {
            return Err(Error::Endpoint("untyped refusal".into()));
        }
        Ok(text(input))
    })
    .with_description(
        Description::new("opaque")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").class(STRING))
            .output("text/plain"),
    );
    let kernel = kernel(endpoint);
    assert!(matches!(
        block_on(kernel.issue(
            request().with_arg("in", ArgRef::Inline(b"bad".to_vec())),
            &Capability::root()
        )),
        Err(Error::Endpoint(_))
    ));
    Suite::new().run_blocking(&kernel).assert_clean();
    let report = Suite::new()
        .fails_under(
            Fixture::new("opaque", Verb::Source).arg("in", "bad"),
            FailureClass::Permanent,
        )
        .run_blocking(&kernel);
    assert!(report.of(Check::Failures).next().is_some(), "{report}");
}

#[test]
fn optional_content_can_be_declared_without_being_read() {
    let endpoint = FnEndpoint::new("ignores-content", |_: &Invocation<'_>| Ok(text("ignored")))
        .with_description(
            Description::new("ignores-content")
                .verb(Verb::Source)
                .input(ArgSpec::new("content").class(STRING).optional())
                .output("text/plain"),
        );
    let kernel = kernel(endpoint);
    assert_eq!(
        block_on(kernel.issue(
            request().with_arg("content", ArgRef::Inline(b"sent".to_vec())),
            &Capability::root()
        ))
        .unwrap()
        .bytes,
        b"ignored"
    );
    // PIPELINE detects a MissingArgument for another input, not argument access.
    Suite::new().run_blocking(&kernel).assert_clean();
}

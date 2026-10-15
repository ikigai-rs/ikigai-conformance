//! `Suite::enforced_under` (ledger #331): ENFORCED's no-grants probe proves the
//! KERNEL's floor, because a declared `urn:cap:net:*` with no grant under that prefix
//! is refused before the endpoint runs. A module's own rule (per host, per name, per
//! prefix) is reached only by a caller holding SOME grant under the prefix but not
//! the one for this target, and the module must refuse that caller with a typed
//! `Denied`. ikigai-llm lost the type on that path (`Error::Endpoint`) and ikigai-lisp
//! flattened it into a string; both walked clean.

use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Fixture, RefusedBy, Report, Suite};
use ikigai_core::{
    ArgSpec, AsyncFnEndpoint, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint,
    Invocation, Iri, Kernel, ReprType, Representation, UriTemplate, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const NET: &str = "urn:cap:net:*";

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

/// How a `fetch` endpoint's per-host rule answers a caller without the host's grant.
#[derive(Clone, Copy)]
enum Rule {
    /// `Error::Denied`: the typed refusal.
    Typed,
    /// `Error::Endpoint`: the type is lost (the ikigai-llm defect).
    Untyped,
    /// Checks only that SOME net grant is held: the rule lets every host through.
    Open,
}

fn fetch(rule: Rule) -> FnEndpoint {
    FnEndpoint::new("fetch", move |inv: &Invocation<'_>| {
        let host = inv.inline_str("host")?;
        let granted = inv.capability.allows(&format!("urn:cap:net:{host}"));
        match rule {
            Rule::Typed if !granted => Err(Error::Denied(format!("no grant for {host}"))),
            Rule::Untyped if !granted => Err(Error::Endpoint(format!("could not fetch {host}"))),
            _ => Ok(text("ok")),
        }
    })
    .with_description(
        Description::new("fetch")
            .verb(Verb::Source)
            .input(ArgSpec::new("host").class(XSD_STRING))
            .output("text/plain")
            .requires(NET),
    )
}

fn kernel_with(endpoint: FnEndpoint) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:example:fetch"), endpoint),
    ))
}

/// A caller holding a grant under the prefix, for another host.
fn elsewhere() -> Capability {
    Capability::scoped(["urn:cap:net:example.com"])
}

fn loopback_probe() -> Fixture {
    Fixture::new("fetch", Verb::Source)
        .arg("host", "127.0.0.1")
        .capability(elsewhere())
}

fn enforced(suite: Suite, kernel: &Kernel) -> Report {
    suite
        .checks(Checks::ENFORCED | Checks::DECLARATIONS)
        .run_blocking(kernel)
}

#[test]
fn the_no_grants_probe_alone_never_reaches_the_module_rule() {
    // The premise: every one of the three rules walks clean without a probe, because
    // the kernel's floor refuses an empty capability before any of them runs.
    for rule in [Rule::Typed, Rule::Untyped, Rule::Open] {
        let report = enforced(Suite::new(), &kernel_with(fetch(rule)));
        report.assert_clean();
        assert!(report.enforced_under_probes().is_empty());
    }
}

#[test]
fn a_typed_refusal_by_the_module_rule_is_clean_and_recorded() {
    let report = enforced(
        Suite::new().enforced_under(loopback_probe()),
        &kernel_with(fetch(Rule::Typed)),
    );
    report.assert_clean();
    let probe = &report.enforced_under_probes()[0];
    assert_eq!(probe.endpoint, "fetch");
    assert_eq!(probe.holding, ["urn:cap:net:example.com"]);
    assert_eq!(probe.refusal.as_deref(), Some("denied"));
    assert_eq!(probe.refused_by, Some(RefusedBy::Module));
    assert!(
        report.to_string().contains(
            "enforced under: fetch source host=\"127.0.0.1\" holding `urn:cap:net:example.com`: \
             refused denied by the endpoint itself\n"
        ),
        "{report}"
    );
}

#[test]
fn a_refusal_that_lost_its_type_is_caught() {
    let report = enforced(
        Suite::new().enforced_under(loopback_probe()),
        &kernel_with(fetch(Rule::Untyped)),
    );
    let found: Vec<_> = report.of(Check::Enforced).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0].detail.contains(
            "under the `enforced_under` caller holding `urn:cap:net:example.com` \
             (`fetch source host=\"127.0.0.1\"`) expected a typed `Denied` from the module's \
             own rule, got `endpoint`"
        ),
        "{report}"
    );
    assert_eq!(report.enforced_under_probes()[0].refused_by, None);
}

#[test]
fn a_rule_that_lets_the_caller_through_is_caught() {
    let report = enforced(
        Suite::new().enforced_under(loopback_probe()),
        &kernel_with(fetch(Rule::Open)),
    );
    assert!(
        report.of(Check::Enforced).any(|f| f
            .detail
            .contains("resolved under the `enforced_under` caller")),
        "{report}"
    );
    assert_eq!(report.enforced_under_probes()[0].refusal, None);
}

#[test]
fn a_probe_the_kernel_floor_refuses_reached_nothing() {
    // A caller with no grant under the declared prefix is refused by the floor: the
    // probe tested what the no-grants probe already does.
    let floored = Fixture::new("fetch", Verb::Source)
        .arg("host", "127.0.0.1")
        .capability(Capability::scoped(["urn:cap:fs:read:/tmp"]));
    let report = enforced(
        Suite::new().enforced_under(floored),
        &kernel_with(fetch(Rule::Open)),
    );
    let found: Vec<_> = report.of(Check::Declarations).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(
        found[0]
            .detail
            .contains("was refused by the kernel's floor before the endpoint ran"),
        "{report}"
    );
    assert!(report.of(Check::Enforced).next().is_none(), "{report}");
    assert!(
        matches!(
            &report.enforced_under_probes()[0].refused_by,
            Some(RefusedBy::Floor { lacking }) if lacking == NET
        ),
        "{report}"
    );
}

#[test]
fn a_probe_without_a_caller_or_an_action_is_inert() {
    let rootless = Fixture::new("fetch", Verb::Source).arg("host", "127.0.0.1");
    let unknown = Fixture::new("fetsch", Verb::Source).capability(elsewhere());
    let sink = Fixture::new("fetch", Verb::Sink).capability(elsewhere());
    let report = enforced(
        Suite::new()
            .enforced_under(rootless)
            .enforced_under(unknown)
            .enforced_under(sink),
        &kernel_with(fetch(Rule::Typed)),
    );
    let found: Vec<String> = report
        .of(Check::Declarations)
        .map(|f| f.detail.clone())
        .collect();
    assert_eq!(found.len(), 3, "{report}");
    assert!(found[0].contains("names no caller"), "{report}");
    assert!(
        found[1].contains("probe `fetsch source` never ran"),
        "{report}"
    );
    assert!(
        found[2].contains("never ran: `fetch` declares no sink action"),
        "{report}"
    );
    assert!(report.enforced_under_probes().is_empty(), "{report}");
}

#[test]
fn a_probe_where_enforced_does_not_run_never_ran() {
    let report = Suite::new()
        .checks(Checks::DECLARATIONS)
        .enforced_under(loopback_probe())
        .run_blocking(&kernel_with(fetch(Rule::Open)));
    assert!(
        report
            .of(Check::Declarations)
            .any(|f| f.detail.contains("never ran: ENFORCED is not selected")),
        "{report}"
    );
}

/// `relay` reaches the per-host gate through a SUB-REQUEST to `urn:example:net/{host}`,
/// passing the refusal through (`?`) or flattening it into prose.
fn relay(flatten: bool) -> AsyncFnEndpoint {
    AsyncFnEndpoint::new("relay", move |inv| {
        Box::pin(async move {
            let host = inv.inline_str("host")?.to_string();
            let target = Iri::parse(format!("urn:example:net/{host}")).unwrap();
            match inv.source(&target).await {
                Ok(r) => Ok(text(r.bytes)),
                Err(e) if flatten => Err(Error::Endpoint(format!("relay failed: {e}"))),
                Err(e) => Err(e),
            }
        })
    })
    .with_description(
        Description::new("relay")
            .verb(Verb::Source)
            .input(ArgSpec::new("host").class(XSD_STRING))
            .output("text/plain")
            .requires(NET),
    )
}

fn net() -> FnEndpoint {
    FnEndpoint::new("net", |inv: &Invocation<'_>| {
        let host = inv.bindings.get("host").unwrap_or_default();
        if !inv.capability.allows(&format!("urn:cap:net:{host}")) {
            return Err(Error::Denied(format!("no grant for {host}")));
        }
        Ok(text("ok"))
    })
    .with_description(
        Description::new("net")
            .verb(Verb::Source)
            .input(ArgSpec::new("host").binding().class(XSD_STRING))
            .output("text/plain")
            .requires(NET),
    )
}

fn relay_kernel(flatten: bool) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:example:relay"), relay(flatten))
            .bind(UriTemplate::parse("urn:example:net/{host}").unwrap(), net()),
    ))
}

fn relay_probe() -> Fixture {
    Fixture::new("relay", Verb::Source)
        .arg("host", "127.0.0.1")
        .capability(elsewhere())
}

#[test]
fn a_gate_in_a_sub_resolution_is_reached_and_its_refusal_passed_through() {
    let report = enforced(
        Suite::new().enforced_under(relay_probe()),
        &relay_kernel(false),
    );
    report.assert_clean();
    assert_eq!(
        report.enforced_under_probes()[0].refused_by,
        Some(RefusedBy::SubRequest {
            target: "urn:example:net/127.0.0.1".to_string()
        }),
        "{report}"
    );
}

#[test]
fn a_sub_resolution_refusal_flattened_on_the_way_out_is_caught() {
    // The ikigai-lisp defect: the gate WAS reached and the type was lost.
    let report = enforced(
        Suite::new().enforced_under(relay_probe()),
        &relay_kernel(true),
    );
    assert!(
        report
            .of(Check::Enforced)
            .any(|f| f.detail.contains("got `endpoint`")),
        "{report}"
    );
}

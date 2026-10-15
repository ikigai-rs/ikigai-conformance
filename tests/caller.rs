//! An endpoint ABOUT ITS CALLER (ledger #1317): it answers for the principal a door
//! minted, and refuses a caller that names none, root included. The suite probes under
//! root by default, so without a way to name the caller every check that needs an
//! answer (OUTPUTS, SKOLEM-RDF, VOCABULARY, CACHEABLE) could only be waived, and the
//! endpoint could not be declared live (`urn:layers:me`, ikigai-rs/ikigai-layers PR 8).
//! [`Fixture::capability`] names the caller per action; these tests pin both halves.

use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Fixture, Report, Suite};
use ikigai_core::{
    Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Verb,
};

const TURTLE: &str = "text/turtle";
const READ: &str = "urn:cap:me:read";
const ALICE: &str = "urn:example:person:alice";
/// Core's `PRINCIPAL_SCOPE_PREFIX` (0.1.93), spelled out: a principal is a scope, so
/// these tests build and read one on any core this crate admits.
const PRINCIPAL: &str = "urn:cap:principal:";

/// The principal a capability names: its one well-formed `urn:cap:principal:` scope.
/// What `Capability::principal` answers on core 0.1.93+, for any capability these
/// tests build.
fn principal_of(cap: &Capability) -> Option<String> {
    let mut named = cap
        .scopes()?
        .iter()
        .filter_map(|s| s.strip_prefix(PRINCIPAL))
        .filter(|p| !p.contains('*'));
    let one = named.next()?;
    named.next().is_none().then(|| one.to_string())
}

/// The caller as a door would mint it: the endpoint's read scope plus a principal.
fn alice() -> Capability {
    Capability::scoped([READ.to_string(), format!("{PRINCIPAL}{ALICE}")])
}

/// How the test endpoint misbehaves, if at all, so a test can tell a check that RAN
/// as the caller from one that reported a refusal.
#[derive(Clone, Copy, Default)]
struct Shape {
    /// Emit a blank node (SKOLEM-RDF's business).
    blank: bool,
    /// Mark the answer cacheable (a `live` declaration's business).
    cacheable: bool,
    /// Declare no `requires` while still refusing an anonymous caller (ENFORCED's).
    undeclared: bool,
}

/// `urn:example:me`: who the caller is, as Turtle. Live by default (it is about the
/// caller), and a `Denied` refusal for a caller that names no principal, root included:
/// the shape of `urn:layers:me`.
fn me(shape: Shape) -> FnEndpoint {
    let endpoint = FnEndpoint::new("me", move |inv: &Invocation<'_>| {
        let Some(who) = principal_of(inv.capability) else {
            return Err(Error::Denied(
                "urn:example:me answers the principal the door minted, and this request \
                 names none"
                    .into(),
            ));
        };
        let body = if shape.blank {
            format!(
                "<urn:example:me> <http://xmlns.com/foaf/0.1/primaryTopic> \
                 [ <http://xmlns.com/foaf/0.1/name> \"{who}\" ] .\n"
            )
        } else {
            format!("<urn:example:me> <http://xmlns.com/foaf/0.1/primaryTopic> <{who}> .\n")
        };
        let repr = Representation::new(ReprType::new(TURTLE), body.into_bytes());
        Ok(if shape.cacheable {
            repr.cacheable()
        } else {
            repr
        })
    });
    let description = Description::new("me")
        .title("Who the caller is")
        .verb(Verb::Source)
        .output(TURTLE);
    endpoint.with_description(if shape.undeclared {
        description
    } else {
        description.requires(READ)
    })
}

fn kernel(endpoint: FnEndpoint) -> Kernel {
    Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:example:me"), endpoint),
    ))
}

fn details(report: &Report, check: Check) -> Vec<String> {
    report.of(check).map(|f| f.detail.clone()).collect()
}

/// The reproduction: under root (the default) the endpoint refuses every probe, so the
/// checks that need an answer report the refusal instead of judging the answer, and a
/// `live` declaration is held to nothing.
#[test]
fn under_root_an_endpoint_about_its_caller_can_only_be_waived() {
    let report = Suite::new()
        .live("me")
        .run_blocking(&kernel(me(Shape::default())));
    let refused = |check: Check| {
        details(&report, check)
            .iter()
            .any(|d| d.contains("did not resolve") && d.contains("names none"))
    };
    // The minimal resolution's failure is reported once, under RESOLVES, naming the
    // first check that needed it; OUTPUTS then says it saw nothing.
    assert!(refused(Check::Resolves), "{report}");
    assert!(!refused(Check::SkolemRdf), "{report}");
    assert!(
        details(&report, Check::Resolves)
            .iter()
            .any(|d| d.contains("Needed by SKOLEM-RDF")),
        "{report}"
    );
    // ...and the refusal under root names the way out.
    assert!(
        details(&report, Check::Resolves).iter().any(|d| d
            .contains("probed under root, which names no principal")
            && d.contains(r#"Fixture::new("me", Verb::Source).capability(…)"#)),
        "{report}"
    );
    assert!(
        report
            .unprobed
            .iter()
            .any(|u| u.endpoint == "me" && u.check == Check::Outputs),
        "{report}"
    );
    assert!(
        !report.probed.iter().any(|p| p.endpoint == "me"),
        "nothing was served, so nothing was probed:\n{report}"
    );
}

fn as_alice() -> Fixture {
    Fixture::new("me", Verb::Source).capability(alice())
}

/// The fix: named as a participant, the same endpoint is checked in full and clean, and
/// `live` is held to something.
#[test]
fn as_a_named_caller_every_check_judges_the_answer() {
    let report = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape::default())));
    assert!(report.is_clean(), "{report}");
    let text = report.to_string();
    assert!(
        text.contains("probed: me source `text/turtle`: 1 triple(s)"),
        "the face was resolved and parsed:\n{report}"
    );
    assert!(
        text.contains("fixture: me source as-caller=urn:example:person:alice (2 scopes)"),
        "the record says who it was probed as:\n{report}"
    );
    assert!(report.unprobed.is_empty(), "{report}");
}

/// Proof the checks ran AS the caller rather than passing vacuously: each misbehavior
/// the endpoint commits only for a named caller is caught.
#[test]
fn the_answer_judging_checks_resolve_as_the_named_caller() {
    // SKOLEM-RDF: the blank node only a named caller is ever shown.
    let blank = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape {
            blank: true,
            ..Shape::default()
        })));
    assert!(
        details(&blank, Check::SkolemRdf)
            .iter()
            .any(|d| d.contains("blank node")),
        "{blank}"
    );
    // CACHEABLE: `live` is held to the answer, which is cacheable here.
    let cached = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape {
            cacheable: true,
            ..Shape::default()
        })));
    assert!(
        details(&cached, Check::Cacheable)
            .iter()
            .any(|d| d.contains("declared live")),
        "{cached}"
    );
}

/// The cache probe's second resolution is made as the same caller (under root it would
/// be refused, and reported as a failed second resolution), and the kernel, which keys
/// its cache on the capability, serves it from the cache: `cacheable` holds.
#[test]
fn the_cache_probe_resolves_twice_as_the_same_caller() {
    let report = Suite::new()
        .cacheable("me")
        .pure("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape {
            cacheable: true,
            ..Shape::default()
        })));
    assert!(report.is_clean(), "{report}");
}

/// ENFORCED and AUTHORITY still ask what the action does under NO grants, which carry
/// no principal either: a fixture's caller does not reach them. An endpoint that refuses
/// an anonymous caller without declaring a gate is still an undeclared refusal.
#[test]
fn enforced_still_probes_under_no_grants() {
    let declared = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape::default())));
    assert_eq!(declared.of(Check::Enforced).count(), 0, "{declared}");
    let undeclared = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape {
            undeclared: true,
            ..Shape::default()
        })));
    assert!(
        details(&undeclared, Check::Enforced)
            .iter()
            .any(|d| d.contains("declares no capability but refused with `Denied`")),
        "{undeclared}"
    );
}

/// Root stays the default: a fixture with arguments and no capability changes nothing
/// about who the walk resolves as.
#[test]
fn a_fixture_without_a_capability_still_probes_under_root() {
    let fixture = Fixture::new("me", Verb::Source).arg("unused", "x");
    assert_eq!(fixture.caller(), None);
    let report = Suite::new()
        .live("me")
        .fixture(fixture)
        .run_blocking(&kernel(me(Shape::default())));
    assert!(
        details(&report, Check::Resolves)
            .iter()
            .any(|d| d.contains("probed under root")),
        "{report}"
    );
}

/// A caller is a declaration like any other: one that no probe resolved as is inert.
#[test]
fn a_caller_no_probe_resolved_as_is_inert() {
    // For a verb the endpoint does not declare.
    let wrong_verb = Suite::new()
        .live("me")
        .fixture(as_alice())
        .fixture(Fixture::new("me", Verb::Sink).capability(alice()))
        .run_blocking(&kernel(me(Shape::default())));
    assert!(
        details(&wrong_verb, Check::Declarations)
            .iter()
            .any(|d| d.contains("supplies a caller for a sink action `me` does not declare")),
        "{wrong_verb}"
    );
    // For an action whose every answer-judging check is unselected: nothing resolved it.
    let unselected = Suite::new()
        .checks(Checks::ARGSPECS | Checks::ENFORCED | Checks::DECLARATIONS)
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape::default())));
    assert!(
        details(&unselected, Check::Declarations)
            .iter()
            .any(|d| d.contains("names a caller no probe resolved as")),
        "{unselected}"
    );
    // And the clean case reports nothing inert.
    let used = Suite::new()
        .live("me")
        .fixture(as_alice())
        .run_blocking(&kernel(me(Shape::default())));
    assert_eq!(used.of(Check::Declarations).count(), 0, "{used}");
}

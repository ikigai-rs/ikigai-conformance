//! A module walked through a MOUNT: a peer kernel's catalog surfaced in this one, each
//! binding stamped with its origin, every request forwarded. Two ways the walk used to
//! read such a kernel wrongly: an alias carried the peer's kernel operations past the
//! `urn:kernel:` exclusion (ledger #335), and a peer with no Meta renderer collapsed
//! into one anonymous endpoint that read as small rather than broken (ledger #334).
//!
//! The mount here is a hand-built copy of what `ikigai-resolve` does, so the test needs
//! no transport: the catalog re-prefixed and stamped with an origin, a request
//! forwarded with its name restored, and a description read from the peer's JSON Meta
//! face or, when the peer cannot answer one, the anonymous action-less placeholder
//! `ikigai-resolve` falls back to.

use std::collections::BTreeMap;
use std::sync::Arc;

use ikigai_conformance::{Check, Suite};
use ikigai_core::{
    ArgRef, ArgSpec, Bindings, Capability, Description, Endpoint, EndpointSpace, Error, Exact,
    FnEndpoint, Invocation, Iri, Kernel, ReprType, Representation, Request, Resolution, Resolved,
    Result, Scope, Space, SpaceEntry, Verb,
};

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const ORIGIN: &str = "test://peer";

/// A conforming module endpoint, the one thing the peer serves of its own.
fn upper() -> FnEndpoint {
    FnEndpoint::new("upper", |inv: &Invocation<'_>| {
        Ok(Representation::new(
            ReprType::new("text/plain"),
            inv.inline_str("in")?.to_uppercase().into_bytes(),
        ))
    })
    .with_description(
        Description::new("upper")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").class(XSD_STRING))
            .output("text/plain"),
    )
}

fn module_space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(Exact::new("urn:demo:upper"), upper())
        .bind(Exact::new("urn:demo:shout"), upper())
}

/// A peer that answers `Meta` (it has a renderer), as a mount needs.
fn described_peer() -> Arc<Kernel> {
    Arc::new(Kernel::with_meta_renderer(
        Arc::new(module_space()),
        Arc::new(ikigai_vocab::TurtleRenderer),
    ))
}

/// A peer built with `Kernel::new`: no Meta renderer, so no contract over the wire.
fn renderer_less_peer() -> Arc<Kernel> {
    Arc::new(Kernel::new(Arc::new(module_space())))
}

/// The peer's contract for one of its exact bindings, as a mount reads it: the JSON
/// Meta face when the peer serves it, else `ikigai-resolve`'s placeholder, which keeps
/// the id `remote` and says why in its summary.
fn contract(peer: &Kernel, target: &Iri) -> Description {
    let meta = Request::new(Verb::Meta, target.clone())
        .with_arg("as", ArgRef::Inline(b"application/json".to_vec()));
    match futures::executor::block_on(peer.issue(meta, &Capability::root())) {
        Ok(_) => peer
            .describe(target)
            .unwrap_or_else(|| Description::new("remote")),
        Err(err) => Description::new("remote")
            .title("contract unavailable")
            .summary(format!(
                "{ORIGIN} did not answer with a contract for `{target}`: {err}. This row is a \
                 placeholder, not the peer's declaration"
            )),
    }
}

/// A mount of `peer`: under `prefix` as an ALIAS (`urn:` → `prefix`) when given, else
/// an override that lists the peer's own names.
struct Mount {
    prefix: Option<&'static str>,
    peer: Arc<Kernel>,
    /// The peer's contract for each of its exact bindings, read once up front (a
    /// `describe` is synchronous, and a nested executor cannot run inside the walk's).
    contracts: BTreeMap<String, Description>,
}

impl Mount {
    fn new(prefix: Option<&'static str>, peer: Arc<Kernel>) -> Self {
        let contracts = peer
            .entries()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| Iri::parse(&e.pattern).ok())
            .map(|iri| (iri.as_str().to_string(), contract(&peer, &iri)))
            .collect();
        Mount {
            prefix,
            peer,
            contracts,
        }
    }

    /// The peer's name for a local one, or `None` outside the mount.
    fn remote(&self, local: &str) -> Option<String> {
        match self.prefix {
            Some(prefix) => local.strip_prefix(prefix).map(|rest| format!("urn:{rest}")),
            None => local.starts_with("urn:demo:").then(|| local.to_string()),
        }
    }
}

impl Space for Mount {
    fn resolve(&self, request: &Request, _scope: &Scope) -> Resolution {
        let Some(remote) = self.remote(request.target.as_str()) else {
            return Resolution::Miss;
        };
        let Ok(target) = Iri::parse(&remote) else {
            return Resolution::Miss;
        };
        let description = self
            .contracts
            .get(&remote)
            .cloned()
            .unwrap_or_else(|| Description::new("remote"));
        Resolution::Hit(Resolved::new(
            Arc::new(Forward {
                peer: Arc::clone(&self.peer),
                target,
                description,
            }),
            Bindings::new(),
        ))
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        let entries = self.peer.entries()?.into_iter().filter_map(|e| {
            let pattern = match self.prefix {
                Some(prefix) => e
                    .pattern
                    .strip_prefix("urn:")
                    .map(|rest| format!("{prefix}{rest}"))?,
                // An override surfaces only its own namespace.
                None => e.pattern.starts_with("urn:demo:").then_some(e.pattern)?,
            };
            Some(SpaceEntry::new(pattern, e.endpoint).with_origin(ORIGIN))
        });
        Some(entries.collect())
    }
}

/// One forwarded binding: the request goes to the peer under the peer's name.
struct Forward {
    peer: Arc<Kernel>,
    target: Iri,
    description: Description,
}

#[async_trait::async_trait]
impl Endpoint for Forward {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let mut forwarded = inv.request.clone();
        forwarded.target = self.target.clone();
        self.peer.issue(forwarded, inv.capability).await
    }
    fn name(&self) -> &str {
        &self.description.id
    }
    fn describe(&self) -> Description {
        self.description.clone()
    }
}

fn kernel_over(mount: Mount) -> Kernel {
    Kernel::new(Arc::new(mount))
}

/// The premise, checked rather than assumed: an alias lists the peer's kernel
/// operations under the local prefix, where a `urn:kernel:` test does not see them.
#[test]
fn an_alias_lists_the_peers_kernel_operations_under_its_own_prefix() {
    let kernel = kernel_over(Mount::new(Some("urn:edge:"), described_peer()));
    let patterns: Vec<String> = kernel
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.origin.is_some())
        .map(|e| e.pattern)
        .collect();
    assert!(
        patterns.iter().any(|p| p == "urn:edge:kernel:catalog"),
        "{patterns:?}"
    );
    assert!(
        patterns.iter().all(|p| !p.starts_with("urn:kernel:")),
        "{patterns:?}"
    );
}

/// ledger #335: through an alias, the walk skips the peer's kernel operations as it
/// skips this kernel's own, so core's findings are not reported as the module's.
#[test]
fn an_alias_mount_does_not_walk_the_peers_kernel_operations() {
    let kernel = kernel_over(Mount::new(Some("urn:edge:"), described_peer()));
    let report = Suite::new().run_blocking(&kernel);
    assert_eq!(&*report.walked, ["upper"], "{report}");
    assert!(
        report
            .findings
            .iter()
            .all(|f| !f.endpoint.starts_with("kernel-")),
        "core's findings are not the module's:\n{report}"
    );
}

/// ledger #335, the other half: a peer that cannot describe its operations (no
/// renderer) has them recognized by their re-prefixed NAME, so they are not counted
/// among the mount's unreadable bindings either.
#[test]
fn a_renamed_kernel_operation_is_recognized_by_its_name_when_it_cannot_describe_itself() {
    let kernel = kernel_over(Mount::new(Some("urn:edge:"), renderer_less_peer()));
    let report = Suite::new().run_blocking(&kernel);
    let mount = report
        .findings
        .iter()
        .find(|f| f.detail.contains("mounted from"))
        .unwrap_or_else(|| panic!("{report}"));
    assert!(
        mount
            .detail
            .starts_with("2 binding(s) mounted from `test://peer`"),
        "{report}"
    );
    assert!(!mount.detail.contains("kernel:"), "{report}");
}

/// ledger #334: a peer with no Meta renderer gives every endpoint it serves one
/// anonymous, action-less placeholder. The walk says so, loudly, naming the renderer,
/// instead of counting one small endpoint with one "declares no action" line.
#[test]
fn a_mount_whose_peer_has_no_meta_renderer_fails_naming_the_renderer() {
    // The premise: the peer refuses the JSON Meta face a mount reads.
    let peer = renderer_less_peer();
    let meta = Request::new(Verb::Meta, Iri::parse("urn:demo:upper").unwrap())
        .with_arg("as", ArgRef::Inline(b"application/json".to_vec()));
    match futures::executor::block_on(peer.issue(meta, &Capability::root())) {
        Err(Error::Endpoint(msg)) => assert!(msg.contains("no Meta renderer"), "{msg}"),
        other => panic!("expected the missing renderer, got {other:?}"),
    }

    let report = Suite::new().run_blocking(&kernel_over(Mount::new(None, peer)));
    let mount: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.detail.contains("mounted from"))
        .collect();
    assert_eq!(mount.len(), 1, "one finding per mount:\n{report}");
    let finding = mount[0];
    assert_eq!(finding.endpoint, "remote");
    assert_eq!(finding.check, Check::ArgSpecs);
    for needle in [
        "2 binding(s) mounted from `test://peer` describe as `remote` with no action",
        "`urn:demo:upper`",
        "`urn:demo:shout`",
        "NO Meta renderer",
        "`Kernel::with_meta_renderer`",
        "no Meta renderer configured", // the mount's own reason, carried through
    ] {
        assert!(finding.detail.contains(needle), "{needle:?}:\n{report}");
    }
    // Not walked, and not counted as a small, healthy endpoint.
    assert_eq!(report.endpoints, 0, "{report}");
    assert!(report.walked.is_empty(), "{report}");
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.detail.starts_with("declares no action")),
        "{report}"
    );

    // Loud whatever the selection: a mount the walk could not read is a walk of nothing.
    let names_only = Suite::new()
        .checks(ikigai_conformance::Checks::NAMES)
        .run_blocking(&kernel_over(Mount::new(None, renderer_less_peer())));
    assert!(!names_only.is_clean(), "{names_only}");
}

/// The fix it names works: the same mount over a peer WITH a renderer is walked in
/// full and reports nothing about the mount.
#[test]
fn a_mount_whose_peer_has_a_renderer_is_walked_in_full() {
    let report = Suite::new().run_blocking(&kernel_over(Mount::new(None, described_peer())));
    assert_eq!(&*report.walked, ["upper"], "{report}");
    assert_eq!(report.actions, 2, "{report}");
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.detail.contains("mounted from")),
        "{report}"
    );
}

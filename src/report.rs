//! The report: every finding at once, one line each, endpoint id first — so a
//! failure is a checklist, not the first miss.

use std::collections::BTreeSet;
use std::fmt;

use ikigai_core::{FailureClass, Verb};

use crate::checks::{Check, Checks};
use crate::suite::Fixture;

/// One violation of one check by one endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The endpoint's [`Description::id`](ikigai_core::Description::id) — the catalog
    /// subject identity, the same name the MCP projection and `describe` use.
    pub endpoint: String,
    /// The action's verb, when the finding is about one action rather than the
    /// whole description.
    pub verb: Option<Verb>,
    /// The check that found it.
    pub check: Check,
    /// What is wrong, and — where the fix is not obvious from the rule — how to fix
    /// or opt out.
    pub detail: String,
}

impl Finding {
    /// The endpoint label of a finding about the suite itself rather than about one
    /// endpoint: an [`own`](crate::Suite::own) pattern that matched nothing,
    /// [`required_on_mutating`](crate::Suite::required_on_mutating) or a registered
    /// namespace that reached nothing. Prefer [`is_suite_wide`](Self::is_suite_wide)
    /// to comparing against it. (`(kernel)`, a root space that cannot be enumerated,
    /// is a different label.)
    pub const SUITE_WIDE: &'static str = "(suite)";

    /// Whether this finding is about the suite's own declarations rather than about
    /// one endpoint: its [`endpoint`](Self::endpoint) is [`SUITE_WIDE`](Self::SUITE_WIDE).
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks, Finding, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let space = EndpointSpace::new().bind(Exact::new("urn:example:echo"), builtins::echo());
    /// let report = Suite::new()
    ///     .checks(Checks::NAMES)
    ///     .own("ehco") // a typo: it matches nothing
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// let suite_wide: Vec<&Finding> = report.findings.iter().filter(|f| f.is_suite_wide()).collect();
    /// assert_eq!(suite_wide.len(), 1);
    /// assert_eq!(suite_wide[0].endpoint, "(suite)");
    /// assert_eq!(suite_wide[0].check, Check::Declarations);
    /// ```
    pub fn is_suite_wide(&self) -> bool {
        self.endpoint == Self::SUITE_WIDE
    }

    /// Who fixes this finding: `Some(endpoint)` for a finding about what an endpoint
    /// does or declares, `None` for one whose fix is in the SUITE's own configuration
    /// (ledger #341): every [`Check::Declarations`] finding, and every
    /// [suite-wide](Self::is_suite_wide) one.
    ///
    /// A `DECLARATIONS` finding still names the endpoint its declaration was about (a
    /// fixture for `cell`, a waiver for `upper`), because that says WHICH declaration;
    /// but the endpoint is not the owner. A composing host that partitions findings by
    /// owner (its own, inherited, the module's) and keys on `endpoint` files the suite
    /// author's mistake under the module's name, and a module that is blameless reads as
    /// the problem. Partition on this instead. (`(kernel)`, a root space that cannot be
    /// enumerated, is `Some`: the fix is in the kernel the test built.)
    ///
    /// ```
    /// use ikigai_conformance::{Check, Fixture, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel, Verb};
    /// use std::sync::Arc;
    ///
    /// let space = EndpointSpace::new().bind(Exact::new("urn:example:echo"), builtins::echo());
    /// let report = Suite::new()
    ///     // A fixture for an action `echo` does not declare: the suite's mistake.
    ///     .fixture(Fixture::new("echo", Verb::Sink).arg("content", "x"))
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// let declaration = report.of(Check::Declarations).next().unwrap();
    /// assert_eq!(declaration.endpoint, "echo"); // printed: it names WHICH declaration
    /// assert_eq!(declaration.owner(), None); // but the suite owns the fix
    /// // The builtin predates the recipe (an untyped input): that one is `echo`'s.
    /// let argspecs = report.of(Check::ArgSpecs).next().unwrap();
    /// assert_eq!(argspecs.owner(), Some("echo"));
    /// ```
    pub fn owner(&self) -> Option<&str> {
        if self.check == Check::Declarations || self.is_suite_wide() {
            None
        } else {
            Some(&self.endpoint)
        }
    }

    pub(crate) fn new(
        endpoint: impl Into<String>,
        verb: Option<Verb>,
        check: Check,
        detail: impl Into<String>,
    ) -> Self {
        Finding {
            endpoint: endpoint.into(),
            verb,
            check,
            detail: detail.into(),
        }
    }
}

/// One report line: `<endpoint>  <CHECK>  [<verb>:] <detail>`. The id is printed
/// with its control characters escaped: it is the endpoint's own string, and a
/// newline in it would otherwise split the line (ledger #1392).
impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}  ", visible(&self.endpoint), self.check.label())?;
        if let Some(verb) = self.verb {
            write!(f, "{}: ", verb_name(verb))?;
        }
        f.write_str(&self.detail)
    }
}

/// An action excluded from the invoking checks, with the reason the module gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptedOut {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The verb opted out, or every verb.
    pub verb: Option<Verb>,
    /// Why — printed in the report, because an opt-out list nobody can read is a
    /// gate that silently covers less than it looks like it does.
    pub reason: String,
}

/// An endpoint the module declared inherited, with the reason it gave
/// ([`Suite::inherit`](crate::Suite::inherit)): why an id bound beside the module is
/// not its own and is not walked. Printed as `inherited because: <id>: <reason>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inherited {
    /// The endpoint's description id.
    pub endpoint: String,
    /// Why it is not the module's own: the record an opt-out's reason used to carry.
    pub reason: String,
}

/// One BINDING excluded from the invoking checks, with the reason the module gave
/// ([`Suite::opt_out_at`](crate::Suite::opt_out_at)).
///
/// The scope [`OptedOut`] cannot express: an id is not a unique name for a thing
/// that can be fired, so two entries sharing one description — `urn:file:{path}`
/// jailed to a scratch root and `urn:orgfile:{path}` jailed to whatever the config
/// names — are excluded together or not at all. A pattern names exactly one place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptedOutAt {
    /// The bound pattern, exactly as [`SpaceEntry::pattern`](ikigai_core::SpaceEntry)
    /// reports it.
    pub pattern: String,
    /// The verb opted out, or every verb.
    pub verb: Option<Verb>,
    /// Why — printed in the report, for the same reason [`OptedOut`]'s is.
    pub reason: String,
}

/// One check excluded for one endpoint, with the reason the module gave — the
/// per-rule waiver ([`Suite::opt_out_check`](crate::Suite::opt_out_check)), as
/// opposed to [`OptedOut`], which drops every invoking check at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptedOutCheck {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The check that did not run for it.
    pub check: Check,
    /// Why — printed in the report, so the waived rule and its reason travel
    /// together instead of living in a comment.
    pub reason: String,
}

/// Which kind of space a module said its constructor builds
/// ([`Check::SpaceName`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpaceNaming {
    /// A configuration-free space that names itself
    /// [`space_iri`](ikigai_core::space_iri)`(module)`
    /// ([`Suite::self_named_space`](crate::Suite::self_named_space)).
    SelfNamed,
    /// An instance-built space the host names, so it claims nothing itself
    /// ([`Suite::host_named_space`](crate::Suite::host_named_space)).
    HostNamed,
}

/// One space a module declared to [`Check::SpaceName`], and what checking it saw.
///
/// The declaration half (`label`, `naming`, `constructor`) is what the module
/// said; `doors` and `opaque` are what the run compared, so a clean `space:` line
/// says how much the "same doors" claim was actually checked over. A report holds
/// them as `report.declared.spaces`, in declaration order:
///
/// ```
/// use ikigai_conformance::{Checks, SpaceNaming, Suite};
/// use ikigai_core::{builtins, space_iri, EndpointSpace, Exact, Kernel};
/// use std::sync::Arc;
///
/// fn space() -> EndpointSpace {
///     EndpointSpace::new()
///         .bind(Exact::new("urn:example:echo"), builtins::echo())
///         .named(space_iri("example"))
/// }
/// let report = Suite::new()
///     .checks(Checks::SPACE_NAME)
///     .self_named_space("example", space)
///     .run_blocking(&Kernel::new(Arc::new(space())));
/// assert_eq!(report.declared.spaces[0].naming, SpaceNaming::SelfNamed);
/// assert_eq!(report.declared.spaces[0].doors, Some(1));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeclaredSpace {
    /// What findings and waivers name it by: the claimed IRI for a self-named space
    /// (`urn:iki:space:text`), the label the module gave for a host-named one.
    pub label: String,
    /// Which kind the module said it is.
    pub naming: SpaceNaming,
    /// The constructor's Rust path for a self-named space
    /// ([`std::any::type_name`], so `ikigai_text::space` for a function, and a
    /// `{{closure}}` path for a closure), for locating it. Empty for a host-named
    /// one, which is declared by value.
    pub constructor: String,
    /// How many doors the two calls were compared over, counted through the whole
    /// topology (confined corridors included); `None` when no comparison ran (a
    /// host-named space, or the check not selected).
    pub doors: Option<usize>,
    /// How many [`Opaque`](ikigai_core::SpaceKind::Opaque) nodes the topology has:
    /// doors behind one are not in the topology comparison, and only `entries()`
    /// (when the space enumerates) still sees them.
    pub opaque: usize,
}

/// A vocabulary the module ships, as [`Suite::vocabulary`](crate::Suite::vocabulary)
/// read it: the namespaces its subjects name and how many terms it defines under
/// them. Printed `module vocabulary: https://example.org/log# (12 terms)`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeclaredVocabulary {
    /// The namespaces read off the graph's subjects, sorted.
    pub namespaces: Vec<String>,
    /// How many terms the graph defines under them.
    pub terms: usize,
}

/// One face that was reached and served — positive evidence of what a walk
/// actually looked at.
///
/// A clean report is otherwise indistinguishable from a never-probed one: an
/// endpoint whose face is undeclared, unreachable or opted out produces no finding
/// and no line, exactly like one whose face is perfect.
///
/// Every face is recorded, not only the RDF ones: a module with no graph face used
/// to get no `probed:` section at all, which is the ambiguity these lines exist to
/// kill. For an RDF face `triples` says whether the pass meant anything — a face
/// parsing to zero triples satisfies both RDF checks vacuously; for every other
/// face the evidence is `bytes`, and nothing about the content is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Probed {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The face's bare media type (`text/turtle`).
    pub face: String,
    /// How many triples it parsed to, for an RDF face ([`rdf::is_rdf_face`](crate::rdf::is_rdf_face)).
    /// Zero is a vacuous pass, not a clean one. Always zero for a non-RDF face,
    /// which is never parsed.
    pub triples: usize,
    /// How many bytes were served — the evidence for a face no check parses.
    pub bytes: usize,
}

/// Two or more bindings that would have issued the IDENTICAL request, fired once.
///
/// The invoking checks are memoized on the request they would issue — verb, target
/// IRI and arguments — so a space that lists one binding twice (an overlay that
/// concatenates its targets' entries) probes it once instead of firing every
/// mutating action twice against the state the first firing left. Without this line
/// the collapse is invisible: the report simply counted fewer actions than there
/// were bindings, and could not say the second five were the same five again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collapsed {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The target IRI every one of these bindings resolved the walk to.
    pub target: String,
    /// The bound patterns that produced it, in walk order — repeated verbatim when
    /// a space listed one pattern more than once, which is what that duplication is.
    pub patterns: Vec<String>,
}

/// One action a check could not observe, with the reason — printed so a clean
/// report says what it did NOT see, not only what it found. Distinct from an
/// opt-out (the module's decision) and from a finding (a violation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unprobed {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The check that could not observe it.
    pub check: Check,
    /// Why: a mutating action never fired under root, a minimal resolution that
    /// failed, a caller's `as=` label standing in for the endpoint's own choice.
    pub reason: String,
}

/// One input [`Check::Required`] dropped, and what the call without it got: the
/// evidence a clean `REQUIRED` stands on, printed as a `required:` line.
///
/// Without these a clean report read the same whether `REQUIRED` dropped twelve
/// inputs or none (ledger #1493): five adopters had to break their own code to learn
/// whether the check had bitten.
///
/// ```
/// use ikigai_conformance::{Check, Suite};
/// use ikigai_core::{
///     ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
///     Representation, Verb,
/// };
/// use std::sync::Arc;
///
/// const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
/// let greet = FnEndpoint::new("greet", |inv: &Invocation<'_>| {
///     let name = inv.inline_str("name")?; // a typed MissingArgument when absent
///     Ok(Representation::new(ReprType::new("text/plain"), format!("hello {name}")))
/// })
/// .with_description(
///     Description::new("greet")
///         .verb(Verb::Source)
///         .input(ArgSpec::new("name").class(XSD_STRING))
///         .output("text/plain"),
/// );
/// let root = EndpointSpace::new().bind(Exact::new("urn:example:greet"), greet);
/// let report = Suite::new().run_blocking(&Kernel::new(Arc::new(root)));
///
/// let probe = &report.required_probes()[0];
/// assert_eq!((probe.endpoint.as_str(), probe.input.as_str()), ("greet", "name"));
/// assert_eq!(probe.refusal.as_deref(), Some("missing-argument"));
/// assert!(
///     report.to_string().contains("required: greet source without `name`: refused missing-argument\n"),
///     "{report}"
/// );
/// assert_eq!(report.of(Check::Required).count(), 0);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequiredProbe {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The required input left out of the call; for a call about an input GROUP, the
    /// group's node segment (`group:exactly-one:content,in`, core's
    /// `InputGroup::node_segment`).
    pub input: String,
    /// The kind of refusal the call got, as
    /// [`Error::kind`](ikigai_core::Error::kind) names it (`missing-argument`,
    /// `invalid-argument`, the two the check expects of a call the contract refuses, or
    /// `endpoint`, `timeout`, …), or `None` when it RESOLVED: for a dropped required
    /// input, declared required, actually optional, which is a finding.
    pub refusal: Option<String>,
    /// For a call about an input group, its node segment (as `input`); `None` for a
    /// dropped required input.
    pub group: Option<String>,
    /// For a group call, the members it gave: none (every member dropped), one (that
    /// member alone), or two (a pair, where the group allows at most one).
    pub given: Vec<String>,
    /// Whether core's one checker (`ActionSpec::check`) refuses the call: the verdict the
    /// endpoint's outcome is held to. Always `true` for a dropped required input.
    pub contract_refuses: bool,
}

/// A `Source` or `Exists` action no pipe can reach (core 0.1.105, ledger #15): it
/// declares no required by-value input, and every input group that asks for a member
/// has no `pipe`, so the engine has no slot to route a piped, positional or batch-stdin
/// value into and calls it ambiguous. Information, never a finding: a named-only
/// endpoint is a legitimate choice, and this line is where it is seen. Printed as a
/// `pipeline:` line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NamedOnly {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The node segments of its pipe-less groups.
    pub groups: Vec<String>,
}

/// Where a [`Suite::enforced_under`](crate::Suite::enforced_under) probe's refusal
/// came from, read from the kernel's trace of the probe.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusedBy {
    /// The endpoint itself returned the `Denied`: its own rule was reached.
    Module,
    /// A sub-request the endpoint made was refused, by the kernel's floor or by the
    /// endpoint it reached, and the typed `Denied` came back on the module's path: the
    /// gate that lives in a sub-resolution (a remote stylesheet, a program's own
    /// request) was reached.
    SubRequest {
        /// The sub-request's target IRI.
        target: String,
    },
    /// The kernel's floor refused the probe before the endpoint ran, because the
    /// caller lacks a scope the action declares (`lacking`). The module's own rule
    /// was NOT reached: the probe's capability is the problem, and
    /// [`Check::Declarations`] says so.
    Floor {
        /// The declared scope the caller lacked, as the kernel named it.
        lacking: String,
    },
    /// No trace event says where (a path that records none, such as a peer across a
    /// mount): the `Denied` is typed, and its origin is unknown.
    Untraced,
}

/// One [`Suite::enforced_under`](crate::Suite::enforced_under) probe and what it got:
/// the evidence that a module's OWN access rule was reached, printed as an
/// `enforced under:` line.
///
/// ```
/// use ikigai_conformance::{Checks, Fixture, RefusedBy, Suite};
/// use ikigai_core::{
///     ArgSpec, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation,
///     Kernel, ReprType, Representation, Verb,
/// };
/// use std::sync::Arc;
///
/// // A per-name rule: some grant under `urn:cap:example:read:` passes the kernel's
/// // floor, and the endpoint answers only for the name the grant names.
/// let secret = FnEndpoint::new("secret", |inv: &Invocation<'_>| {
///     let name = inv.inline_str("name")?;
///     if !inv.capability.allows(&format!("urn:cap:example:read:{name}")) {
///         return Err(Error::Denied(format!("no grant on `{name}`")));
///     }
///     Ok(Representation::new(ReprType::new("text/plain"), b"s3cret".to_vec()))
/// })
/// .with_description(
///     Description::new("secret")
///         .verb(Verb::Source)
///         .input(ArgSpec::new("name").class("http://www.w3.org/2001/XMLSchema#string"))
///         .output("text/plain")
///         .requires("urn:cap:example:read:*"),
/// );
/// let space = EndpointSpace::new().bind(Exact::new("urn:example:secret"), secret);
/// let report = Suite::new()
///     .checks(Checks::ENFORCED)
///     .enforced_under(
///         Fixture::new("secret", Verb::Source)
///             .arg("name", "db")
///             .capability(Capability::scoped(["urn:cap:example:read:mail"])),
///     )
///     .run_blocking(&Kernel::new(Arc::new(space)));
/// report.assert_clean();
/// let probe = &report.enforced_under_probes()[0];
/// assert_eq!(probe.refusal.as_deref(), Some("denied"));
/// assert_eq!(probe.refused_by, Some(RefusedBy::Module));
/// assert!(report.to_string().contains(
///     "enforced under: secret source name=\"db\" holding `urn:cap:example:read:mail`: \
///      refused denied by the endpoint itself\n"
/// ), "{report}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EnforcedUnder {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The probe's bindings and arguments, as a fixture line prints them, without its
    /// caller (which is [`holding`](Self::holding)).
    pub probe: String,
    /// The scopes the probe's caller holds, its principal included, sorted.
    pub holding: Vec<String>,
    /// The kind of refusal the probe got, as
    /// [`Error::kind`](ikigai_core::Error::kind) names it (`denied` is the one the
    /// check expects), or `None` when it RESOLVED: the module's rule let a caller
    /// through that it should have refused, which is a finding.
    pub refusal: Option<String>,
    /// Where a `Denied` came from; `None` for any other outcome.
    pub refused_by: Option<RefusedBy>,
}

/// What the walk observed beyond findings and faces: `REQUIRED`'s evidence and the
/// waivers that suppressed nothing. Filled in by the walk, like
/// [`Declarations::inherited`]; read it through [`Report::required_probes`],
/// [`Report::required_nothing`] and [`Report::idle_waivers`].
///
/// It lives on [`Declarations`] because `Report` is the `Err` of
/// [`check`](crate::check), travels by value, and sits at clippy's 128-byte
/// large-error bar; and it is one `#[non_exhaustive]` struct so the next piece of
/// evidence is a field here rather than another field on `Declarations`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Evidence {
    /// Every input [`Check::Required`] dropped, in walk order (`required:` lines).
    pub required: Vec<RequiredProbe>,
    /// Why `REQUIRED` dropped nothing at all, when it is selected and did not: no
    /// required by-value input on the verbs it walks, mutating verbs off, every
    /// candidate opted out or unprobed. `None` when it dropped something, or is not
    /// selected. A clean `REQUIRED` with this set is a statement about declarations
    /// only.
    pub required_nothing: Option<String>,
    /// The [`Suite::opt_out_check`](crate::Suite::opt_out_check) waivers that
    /// suppressed nothing: the waived check ran with the waiver set aside, over every
    /// action and description it covers, and found nothing (`note:` lines). A NOTE,
    /// never a finding.
    pub idle_waivers: Vec<OptedOutCheck>,
    /// The reasons [`Suite::inherit`](crate::Suite::inherit) gave, in declaration
    /// order (`inherited because:` lines). A declaration rather than an observation,
    /// kept here so it is a field on this `#[non_exhaustive]` struct and not one more
    /// on [`Declarations`]; read it through [`Report::inherited_because`].
    pub inherited_because: Vec<Inherited>,
    /// Which run produced this report, as [`Suite::label`](crate::Suite::label) named
    /// it (`run:`, the report's first line). A declaration, kept here for the reason
    /// [`inherited_because`](Self::inherited_because) is; read it through
    /// [`Report::label`].
    pub label: Option<String>,
    /// Every [`Suite::enforced_under`](crate::Suite::enforced_under) probe that ran, in
    /// walk order (`enforced under:` lines): the evidence that a module's own access
    /// rule, not only the kernel's floor, was reached.
    pub enforced_under: Vec<EnforcedUnder>,
    /// Every cacheable result the forced-recomputation witness of
    /// [`Check::Recompute`] recomputed and found identical, in walk order
    /// (`recomputed:` lines).
    pub recomputed: Vec<Recomputed>,
    /// Every read action whose only intake is an input group with no `pipe` member, in
    /// walk order (`pipeline:` lines, under [`Check::Pipeline`]).
    pub named_only: Vec<NamedOnly>,
    /// Whether the module opted into [`Check::Failures`]
    /// ([`Suite::check_failures`](crate::Suite::check_failures)). A declaration, kept here
    /// for the reason [`inherited_because`](Self::inherited_because) is.
    pub check_failures: bool,
    /// What [`Check::Failures`] saw of every action it ran on, in walk order
    /// (`failures:` lines): the evidence a clean `FAILURES` stands on, and the actions
    /// that state no failure classes and so were compared with nothing.
    pub failures: Vec<FailuresSeen>,
    /// Every [`Suite::fails_under`](crate::Suite::fails_under) probe that ran, in walk
    /// order (`fails under:` lines).
    pub fails_under: Vec<FailsUnder>,
}

/// What [`Check::Failures`] saw of one action: what it declares and the classes of the
/// failures the walk observed from it. A stated declaration prints as a `failures:` line
/// of its own; the actions that state nothing share one.
///
/// ```
/// use ikigai_conformance::{Checks, Suite};
/// use ikigai_core::{
///     ArgSpec, Description, EndpointSpace, Error, Exact, FailureClass, FnEndpoint, Invocation,
///     Kernel, ReprType, Representation, Verb,
/// };
/// use std::sync::Arc;
///
/// // A bounded parser: past 8 bytes it refuses with `OverBudget`, and it says so.
/// let parse = FnEndpoint::new("parse", |inv: &Invocation<'_>| {
///     let text = inv.inline_str("content")?;
///     if text.len() > 8 {
///         return Err(Error::OverBudget { detail: format!("{} bytes, past 8", text.len()) });
///     }
///     Ok(Representation::new(ReprType::new("text/plain"), text.as_bytes().to_vec()))
/// })
/// .with_description(
///     Description::new("parse")
///         .verb(Verb::Source)
///         .input(ArgSpec::new("content").class("http://www.w3.org/2001/XMLSchema#string"))
///         .output("text/plain")
///         .fails(FailureClass::OverBudget),
/// );
/// let space = EndpointSpace::new().bind(Exact::new("urn:example:parse"), parse);
/// let report = Suite::new()
///     .checks(Checks::FAILURES)
///     .check_failures()
///     .run_blocking(&Kernel::new(Arc::new(space)));
/// report.assert_clean();
/// let seen = &report.failures_seen()[0];
/// assert_eq!(seen.declared.as_deref(), Some(&[FailureClass::OverBudget][..]));
/// assert!(seen.failed.is_empty()); // the walk's sample is short: it answered
/// assert!(report.to_string().contains(
///     "failures: parse source declares over-budget: 1 resolution(s), none failed\n"
/// ), "{report}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FailuresSeen {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The classes the action declares, in declaration order (an empty list says "only
    /// the implied ones"), or `None` when it states none and nothing was compared.
    pub declared: Option<Vec<FailureClass>>,
    /// How many resolutions of the action the walk observed, answers and failures both.
    pub resolutions: usize,
    /// The distinct classes ([`Error::class`](ikigai_core::Error::class)) of the
    /// failures among them, in the order first seen.
    pub failed: Vec<FailureClass>,
}

/// One [`Suite::fails_under`](crate::Suite::fails_under) probe and what it got, printed
/// as a `fails under:` line.
///
/// ```
/// use ikigai_conformance::{Checks, Fixture, Suite};
/// use ikigai_core::{
///     ArgSpec, Description, EndpointSpace, Error, Exact, FailureClass, FnEndpoint, Invocation,
///     Kernel, ReprType, Representation, Verb,
/// };
/// use std::sync::Arc;
///
/// let parse = FnEndpoint::new("parse", |inv: &Invocation<'_>| {
///     let text = inv.inline_str("content")?;
///     if text.len() > 8 {
///         return Err(Error::OverBudget { detail: format!("{} bytes, past 8", text.len()) });
///     }
///     Ok(Representation::new(ReprType::new("text/plain"), text.as_bytes().to_vec()))
/// })
/// .with_description(
///     Description::new("parse")
///         .verb(Verb::Source)
///         .input(ArgSpec::new("content").class("http://www.w3.org/2001/XMLSchema#string"))
///         .output("text/plain")
///         .fails(FailureClass::OverBudget),
/// );
/// let space = EndpointSpace::new().bind(Exact::new("urn:example:parse"), parse);
/// let report = Suite::new()
///     .checks(Checks::FAILURES)
///     .check_failures()
///     .fails_under(
///         Fixture::new("parse", Verb::Source).arg("content", "far too long"),
///         FailureClass::OverBudget,
///     )
///     .run_blocking(&Kernel::new(Arc::new(space)));
/// report.assert_clean();
/// let probe = &report.fails_under_probes()[0];
/// assert_eq!(probe.got, Some(FailureClass::OverBudget));
/// assert_eq!(probe.kind.as_deref(), Some("over-budget"));
/// assert!(report.to_string().contains(
///     "fails under: parse source content=\"far too long\" expecting over-budget: failed \
///      over-budget\n"
/// ), "{report}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FailsUnder {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// The probe as a fixture line prints it, its caller included.
    pub probe: String,
    /// The class the module said the probe must fail with.
    pub expected: FailureClass,
    /// The class it failed with, or `None` when it RESOLVED (a finding).
    pub got: Option<FailureClass>,
    /// The kind of the failure, as [`Error::kind`](ikigai_core::Error::kind) names it, or
    /// `None` when it resolved.
    pub kind: Option<String>,
}

/// One cacheable result [`Check::Recompute`] forced to recompute and found
/// byte-identical: the evidence a clean `RECOMPUTE` stands on, printed as a
/// `recomputed:` line.
///
/// The cacheable-twice probe's second resolution is the cache hit, so it can only
/// repeat the first. The witness cuts every thread of the first result, its own name
/// included, and everything it read, resolves again, and holds the bytes to the
/// first's, several times over (one recomputation agrees with the first by chance as
/// often as a two-entry `HashMap`'s order does); then cuts each thread an endpoint
/// DECLARED alone, and does the same once.
///
/// ```
/// use ikigai_conformance::{Checks, Suite};
/// use ikigai_core::{
///     ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
///     Representation, Verb,
/// };
/// use std::sync::Arc;
///
/// let upper = FnEndpoint::new("upper", |inv: &Invocation<'_>| {
///     Ok(Representation::new(ReprType::new("text/plain"),
///         inv.inline_str("in")?.to_uppercase().into_bytes()).cacheable())
/// })
/// .with_description(Description::new("upper").verb(Verb::Source)
///     .input(ArgSpec::new("in").class("http://www.w3.org/2001/XMLSchema#string"))
///     .output("text/plain"));
/// let space = EndpointSpace::new().bind(Exact::new("urn:example:upper"), upper);
/// let report = Suite::new()
///     .checks(Checks::CACHEABLE | Checks::RECOMPUTE)
///     .pure("upper")
///     .run_blocking(&Kernel::new(Arc::new(space)));
/// report.assert_clean();
/// assert_eq!(report.recomputed()[0].cut, 1); // its own name
/// assert_eq!(report.recomputed()[0].recomputations, 24);
/// assert!(report.to_string().contains(
///     "recomputed: upper source 24 times after a cut of 1 thread(s): identical\n"
/// ), "{report}");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recomputed {
    /// The endpoint's description id.
    pub endpoint: String,
    /// The action's verb.
    pub verb: Verb,
    /// How many times the answer was recomputed after the full cut, every
    /// computation byte-identical to the first.
    pub recomputations: usize,
    /// How many threads the forced recomputation cut: every thread of the first
    /// result and the name of everything it read.
    pub cut: usize,
    /// How many of them an endpoint declared (`depends_on`), each also cut alone and
    /// recomputed once more.
    pub declared: usize,
}

/// What one run of the suite found.
///
/// A `Report` is the `Err` of [`check`](crate::check) so a `#[test]` can `unwrap()`
/// it and read the checklist in the panic; it is also the `Ok`, so a clean run still
/// says what it covered. `Debug` prints the same text as `Display` for that reason.
///
/// ```
/// use ikigai_conformance::{check, Check};
/// use ikigai_core::builtins;
/// use ikigai_core::{EndpointSpace, Exact, Kernel};
/// use std::sync::Arc;
///
/// let root = EndpointSpace::new().bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
/// let kernel = Kernel::new(Arc::new(root));
/// let report = check(&kernel).unwrap_err(); // the builtins predate the recipe
///
/// // One line per finding, endpoint id first, then the check's label.
/// let first = report.findings[0].to_string();
/// assert!(first.starts_with("to-upper  "), "{first}");
/// assert!(report.findings.iter().any(|f| f.check == Check::ArgSpecs
///     && f.detail.contains("input `in` has no class")));
/// assert_eq!(report.endpoints, 1);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Report {
    /// Every finding, in walk order (endpoints in catalog order, checks in
    /// [`Check::ALL`] order within an endpoint).
    pub findings: Vec<Finding>,
    /// How many descriptions were walked: distinct description ids, so two
    /// patterns binding one endpoint (`urn:a11y:config` and
    /// `urn:a11y:config:{app}`) count once.
    pub endpoints: usize,
    /// How many actions were examined: one per distinct FIRING — the request the
    /// walk would issue (verb, target IRI, arguments) — so the two patterns above
    /// count their verbs twice (they resolve to different IRIs, each a place the
    /// action can be reached), while a space that lists one binding twice counts it
    /// once and says so in [`collapsed`](Self::collapsed).
    pub actions: usize,
    /// The checks that ran.
    pub checks: Checks,
    /// What the module declared to the suite — echoed so the report says what was
    /// waived, not only what was found. Boxed: `Report` is the `Err` of
    /// [`check`](crate::check) and must stay small enough to return by value.
    pub declared: Box<Declarations>,
    /// The actions a check could not observe, with reasons — the gaps in the
    /// walk's coverage, printed beside the findings (`unprobed: …`).
    ///
    /// Boxed for the same size reason [`walked`](Self::walked) gives — `Report` is
    /// the `Err` of [`check`](crate::check) and travels by value. It derefs to the
    /// `Vec`, so `push`, `len`, `is_empty` and `iter` read exactly as before; only a
    /// bare `for u in &report.unprobed` needs `.iter()`.
    pub unprobed: Box<Vec<Unprobed>>,
    /// The faces the walk actually reached and served (`probed: …`) — so a
    /// first-run clean report carries positive evidence of coverage rather than
    /// only the absence of findings.
    pub probed: Vec<Probed>,
    /// The description ids the walk reached, in walk order — what `endpoints`
    /// counts, by name. Walk order is CATALOG order, never sorted: the order
    /// [`Kernel::entries`](ikigai_core::Kernel::entries) lists the bindings (for an
    /// `EndpointSpace`, the order they were bound), each id where its first binding
    /// is. Compare a sorted copy when the test should not depend on bind order. A declaration naming an id that is not here reached no
    /// check at all ([`Check::Declarations`]), and a test can assert the walk
    /// covered exactly the endpoints the module means to bind.
    ///
    /// A boxed slice, not a `Vec`: `Report` is the `Err` of [`check`](crate::check)
    /// and travels by value, and its size is held under clippy's large-error bar
    /// (a `Vec` here puts it exactly AT the 128-byte threshold, which the lint
    /// rejects).
    pub walked: Box<[String]>,
    /// The bindings that would have issued the identical request, fired once —
    /// empty for a module no binding of which is reachable twice. A boxed slice for
    /// the reason [`walked`](Self::walked) gives.
    pub collapsed: Box<[Collapsed]>,
}

/// What a module declared when it configured the [`Suite`](crate::Suite),
/// echoed in the [`Report`]: every waiver is printed beside the findings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declarations {
    /// The actions the module opted out of the invoking checks, with reasons.
    pub opted_out: Vec<OptedOut>,
    /// The single BINDINGS the module opted out of the invoking checks, with
    /// reasons ([`Suite::opt_out_at`](crate::Suite::opt_out_at)) — the scope
    /// [`opted_out`](Self::opted_out) cannot express.
    pub opted_out_at: Vec<OptedOutAt>,
    /// The single checks the module waived per endpoint, with reasons
    /// ([`Suite::opt_out_check`](crate::Suite::opt_out_check)).
    pub opted_out_checks: Vec<OptedOutCheck>,
    /// Whether the module held `Sink` and `Delete` actions to [`Check::Required`]
    /// too ([`Suite::required_on_mutating`](crate::Suite::required_on_mutating)).
    pub required_on_mutating: bool,
    /// The ids and `*`-ended id prefixes the module declared its own
    /// ([`Suite::own`](crate::Suite::own)); empty when the walk is not scoped.
    pub own: Vec<String>,
    /// What that scope left out, filled in by the walk (as [`spaces`](Self::spaces)
    /// carries what SPACE-NAME compared): the description ids no `own` pattern
    /// matched, in walk order. They are bound beside the module and deliberately NOT
    /// walked, so no check read them and no probe fired them. Empty when no `own` is
    /// declared. Pin it exactly in a composing host's test, so a module that gains an
    /// endpoint outside its own scope turns the test red instead of joining this list
    /// silently: [`Suite::inherit`](crate::Suite::inherit) pins it with a reason per
    /// id, or compare it yourself.
    ///
    /// Walk order is CATALOG order (see [`Report::walked`]), not sorted, so a slice
    /// literal matches only when it is written in bind order. Sort a copy first when
    /// it should not depend on that:
    ///
    /// ```
    /// use ikigai_conformance::{Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let space = EndpointSpace::new()
    ///     .bind(Exact::new("urn:example:upper"), builtins::to_upper())
    ///     .bind(Exact::new("urn:example:echo"), builtins::echo())
    ///     .bind(Exact::new("urn:example:reverse"), builtins::reverse_list());
    /// let report = Suite::new()
    ///     .checks(Checks::NAMES)
    ///     .own("echo")
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// assert_eq!(report.declared.inherited, ["to-upper", "reverse-list"]); // bind order
    /// let mut inherited = report.declared.inherited.clone();
    /// inherited.sort();
    /// assert_eq!(inherited, ["reverse-list", "to-upper"]);
    /// ```
    ///
    /// It lives here rather than on [`Report`] because `Report` is the `Err` of
    /// [`check`](crate::check) and travels by value: one more field takes it to
    /// clippy's 128-byte large-error bar, and `declared` is already boxed.
    pub inherited: Vec<String>,
    /// The endpoints the module declared pure (exempt from the golden-thread half
    /// of [`Check::Cacheable`]).
    pub pure: Vec<String>,
    /// The endpoints the module declared cacheable (held to it by
    /// [`Check::Cacheable`]).
    pub cacheable: Vec<String>,
    /// The endpoints the module declared live — uncacheable by decision, held to
    /// `Expiry::Always` by [`Check::Cacheable`].
    pub live: Vec<String>,
    /// The namespaces the module registered as its own for [`Check::Vocabulary`].
    pub namespaces: Vec<String>,
    /// The vocabularies the module ships, held exactly by [`Check::Vocabulary`]
    /// ([`Suite::vocabulary`](crate::Suite::vocabulary)). One that did not parse is a
    /// finding and is not listed here.
    pub vocabularies: Vec<DeclaredVocabulary>,
    /// The fixtures the module supplied, printed one per line (`fixture: file
    /// source path="README.md"`) so a reader can tell which inputs a walk ran over.
    pub fixtures: Vec<Fixture>,
    /// The spaces the module declared to [`Check::SpaceName`], in declaration order,
    /// each with what checking it compared (`space: …`).
    pub spaces: Vec<DeclaredSpace>,
    /// What the walk observed beyond findings: `REQUIRED`'s evidence and the waivers
    /// that suppressed nothing (0.8.1). Filled in by the walk, as
    /// [`inherited`](Self::inherited) is, and here for the same size reason.
    pub evidence: Evidence,
}

impl Report {
    /// `true` when no check found anything. Prefer [`assert_clean`](Self::assert_clean)
    /// in a test: `assert!(report.is_clean())` panics with `assertion failed` and
    /// none of the checklist.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Panic with the whole report when it is not clean — the one line a `#[test]`
    /// wants, and the assertion every adopter was otherwise writing by hand
    /// (`assert!(report.is_clean(), "{report}")`).
    ///
    /// The panic message is the report's [`Display`](fmt::Display): every finding,
    /// the counts, the checks run and skipped, what was declared, what was probed
    /// and what was not. `into_result().unwrap()` prints the same text — `Debug`
    /// delegates to `Display` for exactly that reason.
    ///
    /// ```should_panic
    /// use ikigai_conformance::Suite;
    /// use ikigai_core::builtins;
    /// use ikigai_core::{EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let root = EndpointSpace::new().bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
    /// // The builtins predate the recipe: this panics, printing the checklist.
    /// Suite::new().run_blocking(&Kernel::new(Arc::new(root))).assert_clean();
    /// ```
    #[track_caller]
    pub fn assert_clean(&self) {
        assert!(self.is_clean(), "{self}");
    }

    /// The findings of one check.
    pub fn of(&self, check: Check) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(move |f| f.check == check)
    }

    /// The findings against one endpoint id.
    pub fn against<'a>(&'a self, endpoint: &'a str) -> impl Iterator<Item = &'a Finding> + 'a {
        self.findings.iter().filter(move |f| f.endpoint == endpoint)
    }

    /// `Ok(self)` when clean, `Err(self)` otherwise — the shape [`check`](crate::check)
    /// returns.
    pub fn into_result(self) -> Result<Report, Report> {
        if self.is_clean() {
            Ok(self)
        } else {
            Err(self)
        }
    }

    /// The read actions a pipe cannot reach because their only intake is an input
    /// group with no `pipe` member (`pipeline:` lines): see [`NamedOnly`].
    pub fn named_only(&self) -> &[NamedOnly] {
        &self.declared.evidence.named_only
    }

    /// Every input [`Check::Required`] dropped, and the refusal each drop got, in
    /// walk order: the evidence behind a clean `REQUIRED` (the `required:` lines).
    /// Empty when it dropped nothing; [`required_nothing`](Self::required_nothing)
    /// then says why.
    pub fn required_probes(&self) -> &[RequiredProbe] {
        &self.declared.evidence.required
    }

    /// Why [`Check::Required`] dropped nothing, when it is selected and did not
    /// (`required: nothing dropped — …`): a clean `REQUIRED` that is a statement
    /// about declarations only. `None` when it dropped at least one input, or is
    /// not selected.
    ///
    /// ```
    /// use ikigai_conformance::Suite;
    /// use ikigai_core::builtins;
    /// use ikigai_core::{EndpointSpace, Kernel, UriTemplate};
    /// use std::sync::Arc;
    ///
    /// // `echo` takes its one input as a template binding: nothing by value to drop.
    /// let echo = UriTemplate::parse("urn:example:echo/{message}").unwrap();
    /// let root = EndpointSpace::new().bind(echo, builtins::echo());
    /// let report = Suite::new().run_blocking(&Kernel::new(Arc::new(root)));
    /// assert!(report.required_probes().is_empty());
    /// let why = report.required_nothing().unwrap();
    /// assert!(why.contains("no `Source` or `Exists` action declares a required by-value input"), "{why}");
    /// ```
    pub fn required_nothing(&self) -> Option<&str> {
        self.declared.evidence.required_nothing.as_deref()
    }

    /// The [`Suite::opt_out_check`](crate::Suite::opt_out_check) waivers that
    /// suppressed nothing: the waived check ran with the waiver set aside and found
    /// nothing (`note:` lines). Information, never a finding: a waiver can be green
    /// today on purpose. A waiver the walk could not judge without firing a `Sink` or
    /// a `Delete` it would not otherwise fire is never listed here.
    pub fn idle_waivers(&self) -> &[OptedOutCheck] {
        &self.declared.evidence.idle_waivers
    }

    /// The reasons each inherited id is not the module's own, as
    /// [`Suite::inherit`](crate::Suite::inherit) declared them (`inherited because:`
    /// lines), in declaration order. Empty when nothing was declared with a reason.
    pub fn inherited_because(&self) -> &[Inherited] {
        &self.declared.evidence.inherited_because
    }

    /// Every [`Suite::enforced_under`](crate::Suite::enforced_under) probe that ran and
    /// what it got, in walk order: the evidence behind a clean `ENFORCED` on a module's
    /// own access rule (the `enforced under:` lines). Empty when the suite declared none.
    pub fn enforced_under_probes(&self) -> &[EnforcedUnder] {
        &self.declared.evidence.enforced_under
    }

    /// Every cacheable result [`Check::Recompute`]'s forced-recomputation witness
    /// recomputed and found identical (the `recomputed:` lines), in walk order.
    pub fn recomputed(&self) -> &[Recomputed] {
        &self.declared.evidence.recomputed
    }

    /// What [`Check::Failures`] saw of every action it ran on (the `failures:` lines), in
    /// walk order: see [`FailuresSeen`]. Empty when it did not run.
    pub fn failures_seen(&self) -> &[FailuresSeen] {
        &self.declared.evidence.failures
    }

    /// Every [`Suite::fails_under`](crate::Suite::fails_under) probe that ran and what it
    /// got (the `fails under:` lines), in walk order: see [`FailsUnder`].
    pub fn fails_under_probes(&self) -> &[FailsUnder] {
        &self.declared.evidence.fails_under
    }

    /// Which run produced this report: the [`Suite::label`](crate::Suite::label), or
    /// `None` when the suite named none.
    pub fn label(&self) -> Option<&str> {
        self.declared.evidence.label.as_deref()
    }

    /// Every finding's `(endpoint, check)` pair, as a set: the key a test pins when it
    /// asserts WHICH findings a walk reports rather than how many (ledger #354). Four
    /// composing adopters wrote it by hand, each spelling the check its own way;
    /// `Check` is `Ord`, so the pairs compare and sort as they are.
    ///
    /// ```
    /// use std::collections::BTreeSet;
    /// use ikigai_conformance::{Check, Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let space = EndpointSpace::new().bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
    /// let report = Suite::new()
    ///     .checks(Checks::ARGSPECS | Checks::CACHEABLE)
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// // An untyped input, and a pure function nobody declared pure.
    /// assert_eq!(
    ///     report.keys(),
    ///     BTreeSet::from([("to-upper", Check::ArgSpecs), ("to-upper", Check::Cacheable)])
    /// );
    /// ```
    pub fn keys(&self) -> BTreeSet<(&str, Check)> {
        self.findings
            .iter()
            .map(|f| (f.endpoint.as_str(), f.check))
            .collect()
    }

    /// [`assert_clean`](Self::assert_clean), and the shape of the walk too: panic with
    /// every way the report differs from `expected`, then the whole report, when it is
    /// not clean or any expectation fails (ledger #354).
    ///
    /// A clean report says nothing was FOUND; these say what was walked, so a module
    /// whose endpoint stops being bound, or a host that quietly gained an opt-out,
    /// turns the test red instead of cleaner. Every adopter wrote these by hand.
    ///
    /// ```
    /// use ikigai_conformance::{Expected, Suite};
    /// use ikigai_core::{ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation};
    /// use ikigai_core::{Kernel, ReprType, Representation, Verb};
    /// use std::sync::Arc;
    ///
    /// let upper = FnEndpoint::new("upper", |inv: &Invocation<'_>| {
    ///     Ok(Representation::new(ReprType::new("text/plain"),
    ///         inv.inline_str("in")?.to_uppercase().into_bytes()).cacheable())
    /// })
    /// .with_description(Description::new("upper").verb(Verb::Source)
    ///     .input(ArgSpec::new("in").class("http://www.w3.org/2001/XMLSchema#string"))
    ///     .output("text/plain"));
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:upper"), upper)));
    /// Suite::new().pure("upper").run_blocking(&kernel).assert_clean_with(
    ///     &Expected::new()
    ///         .endpoints(1)
    ///         .actions(1)
    ///         .walked(["upper"])
    ///         .no_opt_outs()
    ///         .nothing_unprobed(),
    /// );
    /// ```
    #[track_caller]
    pub fn assert_clean_with(&self, expected: &Expected) {
        let mut missed: Vec<String> = Vec::new();
        if !self.is_clean() {
            missed.push(format!("{} finding(s)", self.findings.len()));
        }
        if let Some(n) = expected.endpoints {
            if self.endpoints != n {
                missed.push(format!(
                    "endpoints: expected {n}, walked {}",
                    self.endpoints
                ));
            }
        }
        if let Some(n) = expected.actions {
            if self.actions != n {
                missed.push(format!("actions: expected {n}, examined {}", self.actions));
            }
        }
        if let Some(ids) = &expected.walked {
            let want: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            let got: BTreeSet<&str> = self.walked.iter().map(String::as_str).collect();
            if want != got {
                let absent: Vec<&str> = want.difference(&got).copied().collect();
                let extra: Vec<&str> = got.difference(&want).copied().collect();
                missed.push(format!(
                    "walked: expected exactly {}; not walked: {}; walked but not expected: {}",
                    joined(&want.iter().collect::<Vec<_>>()),
                    if absent.is_empty() {
                        "none".to_string()
                    } else {
                        joined(&absent)
                    },
                    if extra.is_empty() {
                        "none".to_string()
                    } else {
                        joined(&extra)
                    },
                ));
            }
        }
        if expected.no_opt_outs {
            let d = &self.declared;
            let n = d.opted_out.len() + d.opted_out_at.len() + d.opted_out_checks.len();
            if n > 0 {
                missed.push(format!(
                    "opt-outs: expected none, the suite declares {n} (see `opted out:`)"
                ));
            }
        }
        if expected.nothing_unprobed && !self.unprobed.is_empty() {
            missed.push(format!(
                "unprobed: expected none, {} (see `unprobed:`)",
                self.unprobed.len()
            ));
        }
        assert!(missed.is_empty(), "{}\n{self}", missed.join("\n"));
    }

    /// An empty report over `checks`, for a probe whose findings are set aside.
    pub(crate) fn scratch(checks: Checks) -> Report {
        Report {
            findings: Vec::new(),
            endpoints: 0,
            actions: 0,
            checks,
            declared: Box::default(),
            unprobed: Box::default(),
            probed: Vec::new(),
            walked: Box::default(),
            collapsed: Box::default(),
        }
    }
}

/// The same text as [`Display`](fmt::Display): a `Report` is the `Err` of
/// [`check`](crate::check), so `unwrap()` — which formats with `Debug` — must print
/// the checklist rather than a struct dump.
impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Which run this is, first: a host that walks one suite over two kernels
        // prints two reports that are otherwise told apart only by reading them.
        if let Some(label) = &self.declared.evidence.label {
            writeln!(f, "run: {}", visible(label))?;
        }
        for finding in &self.findings {
            writeln!(f, "{finding}")?;
        }
        writeln!(
            f,
            "{} finding(s) across {} endpoint(s), {} action(s)",
            self.findings.len(),
            self.endpoints,
            self.actions
        )?;
        // A check has a THIRD state the list could not express: ran, but not
        // everywhere — a per-check waiver takes it off one endpoint and `checked:`
        // said nothing, so a reader over-reads the coverage. Starred, with the
        // count on its own line.
        let declared = &self.declared;
        let mut waived: std::collections::BTreeMap<Check, Vec<&str>> =
            std::collections::BTreeMap::new();
        for out in &declared.opted_out_checks {
            let ids = waived.entry(out.check).or_default();
            if !ids.contains(&out.endpoint.as_str()) {
                ids.push(&out.endpoint);
            }
        }
        let ran: Vec<String> = self
            .checks
            .iter()
            .map(|c| {
                if waived.contains_key(&c) {
                    format!("{}*", c.label())
                } else {
                    c.label().to_string()
                }
            })
            .collect();
        writeln!(f, "checked: {}", ran.join(" "))?;
        for (check, ids) in &waived {
            if !self.checks.contains(*check) {
                continue;
            }
            if *check == Check::SpaceName {
                // Waived per SPACE, by label: the endpoint count says nothing here.
                let spaces = &declared.spaces;
                let reached = ids
                    .iter()
                    .filter(|id| spaces.iter().any(|s| s.label == **id))
                    .count();
                writeln!(
                    f,
                    "* {} ran on {} of {} space(s); waived on the rest (see `opted out:`)",
                    check.label(),
                    spaces.len().saturating_sub(reached),
                    spaces.len()
                )?;
                continue;
            }
            // Only a waiver for an id the walk REACHED took the check off an
            // endpoint; one naming an id nothing binds took it off nothing, and is
            // its own finding. Counting those here would understate the coverage in
            // the one line whose whole job is to state it.
            let reached = ids
                .iter()
                .filter(|id| self.walked.iter().any(|w| w == *id))
                .count();
            writeln!(
                f,
                "* {} ran on {} of {} endpoint(s); waived on the rest (see `opted out:`)",
                check.label(),
                self.endpoints.saturating_sub(reached),
                self.endpoints
            )?;
        }
        let skipped: Vec<&str> = self.checks.skipped().map(Check::label).collect();
        if !skipped.is_empty() {
            writeln!(f, "skipped: {}", skipped.join(" "))?;
        }
        // What was PROBED, not only what was checked: a clean report otherwise says
        // nothing about whether anything was ever reached. Every face, not only the
        // RDF ones — a module with no graph face had no section at all, which is
        // indistinguishable from a walk that probed nothing.
        if self.probed.is_empty() {
            writeln!(
                f,
                "probed: nothing — no action was resolved, so every clean check above is a \
                 statement about declarations only"
            )?;
        } else {
            let endpoints: std::collections::BTreeSet<&str> =
                self.probed.iter().map(|p| p.endpoint.as_str()).collect();
            writeln!(
                f,
                "probed {} face(s) across {} endpoint(s)",
                self.probed.len(),
                endpoints.len()
            )?;
            for p in &self.probed {
                write!(
                    f,
                    "probed: {} {} `{}`: ",
                    visible(&p.endpoint),
                    verb_name(p.verb),
                    p.face
                )?;
                if crate::rdf::is_rdf_face(&p.face) {
                    write!(f, "{} triple(s)", p.triples)?;
                    if p.triples == 0 {
                        write!(f, " — nothing was checked")?;
                    }
                } else {
                    // No check reads these bytes: the line is evidence the action
                    // was reached and served something, nothing more.
                    write!(f, "{} byte(s)", p.bytes)?;
                }
                writeln!(f)?;
            }
        }
        // What REQUIRED dropped, as `probed:` says what was resolved: a clean
        // REQUIRED otherwise read the same over twelve drops and over none.
        if self.checks.contains(Check::Pipeline) {
            for n in &declared.evidence.named_only {
                writeln!(
                    f,
                    "pipeline: {} {} is named-only: {} {} no `pipe` member, so a piped or \
                     positional value has no slot (`InputGroup::pipe` names the member it fills)",
                    visible(&n.endpoint),
                    verb_name(n.verb),
                    n.groups
                        .iter()
                        .map(|g| format!("`{}`", visible(g)))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if n.groups.len() == 1 { "has" } else { "have" }
                )?;
            }
        }
        if self.checks.contains(Check::Required) {
            let evidence = &declared.evidence;
            if !evidence.required.is_empty() {
                let endpoints: std::collections::BTreeSet<&str> = evidence
                    .required
                    .iter()
                    .map(|p| p.endpoint.as_str())
                    .collect();
                writeln!(
                    f,
                    "required: dropped {} input(s) across {} endpoint(s)",
                    evidence.required.len(),
                    endpoints.len()
                )?;
            }
            for p in &evidence.required {
                let Some(group) = &p.group else {
                    write!(
                        f,
                        "required: {} {} without `{}`: ",
                        visible(&p.endpoint),
                        verb_name(p.verb),
                        visible(&p.input)
                    )?;
                    match &p.refusal {
                        Some(kind) => writeln!(f, "refused {kind}")?,
                        None => writeln!(f, "resolved — declared required, actually optional")?,
                    }
                    continue;
                };
                let given = if p.given.is_empty() {
                    "none of its members".to_string()
                } else {
                    p.given
                        .iter()
                        .map(|m| format!("`{}`", visible(m)))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                write!(
                    f,
                    "required: {} {} {} given {given} (the contract {}): ",
                    visible(&p.endpoint),
                    verb_name(p.verb),
                    visible(group),
                    if p.contract_refuses {
                        "refuses it"
                    } else {
                        "accepts it"
                    }
                )?;
                match &p.refusal {
                    Some(kind) => writeln!(f, "refused {kind}")?,
                    None => writeln!(f, "resolved")?,
                }
            }
            if let Some(why) = &evidence.required_nothing {
                writeln!(f, "required: nothing dropped — {why}")?;
            }
        }
        // What the forced recomputation saw, as `required:` says what was dropped.
        if self.checks.contains(Check::Recompute) {
            for r in &declared.evidence.recomputed {
                write!(
                    f,
                    "recomputed: {} {} {} times after a cut of {} thread(s)",
                    visible(&r.endpoint),
                    verb_name(r.verb),
                    r.recomputations,
                    r.cut
                )?;
                if r.declared > 0 {
                    write!(f, ", and of each of {} declared one alone", r.declared)?;
                }
                writeln!(f, ": identical")?;
            }
        }
        // What FAILURES compared, as `required:` says what was dropped.
        if self.checks.contains(Check::Failures) {
            let mut unstated: Vec<String> = Vec::new();
            for seen in &declared.evidence.failures {
                let Some(classes) = &seen.declared else {
                    unstated.push(format!(
                        "{} {}",
                        visible(&seen.endpoint),
                        verb_name(seen.verb)
                    ));
                    continue;
                };
                write!(
                    f,
                    "failures: {} {} declares {}: {} resolution(s), ",
                    visible(&seen.endpoint),
                    verb_name(seen.verb),
                    class_words(classes),
                    seen.resolutions
                )?;
                if seen.failed.is_empty() {
                    writeln!(f, "none failed")?;
                } else {
                    writeln!(f, "failed as {}", class_words(&seen.failed))?;
                }
            }
            if !unstated.is_empty() {
                writeln!(
                    f,
                    "failures: {} action(s) state no failure classes, so nothing was compared: {}",
                    unstated.len(),
                    unstated.join(", ")
                )?;
            }
            for p in &declared.evidence.fails_under {
                write!(
                    f,
                    "fails under: {} expecting {}: ",
                    p.probe,
                    p.expected.word()
                )?;
                match (&p.kind, p.got) {
                    (Some(kind), Some(got)) if got == p.expected => writeln!(f, "failed {kind}")?,
                    (Some(kind), Some(got)) => writeln!(
                        f,
                        "failed {kind}, class {} — not the stated class",
                        got.word()
                    )?,
                    _ => writeln!(f, "resolved — the stated failure did not happen")?,
                }
            }
        }
        // What each `enforced_under` probe got: the module's own rule, reached or not.
        for p in &declared.evidence.enforced_under {
            let holding = p
                .holding
                .iter()
                .map(|s| format!("`{}`", visible(s)))
                .collect::<Vec<_>>();
            write!(
                f,
                "enforced under: {} holding {}: ",
                p.probe,
                if holding.is_empty() {
                    "nothing".to_string()
                } else {
                    holding.join(", ")
                }
            )?;
            match (&p.refusal, &p.refused_by) {
                (None, _) => writeln!(f, "resolved — the module's own rule let it through")?,
                (Some(kind), None) => writeln!(f, "refused {kind} — not a typed `Denied`")?,
                (Some(kind), Some(RefusedBy::Module)) => {
                    writeln!(f, "refused {kind} by the endpoint itself")?
                }
                (Some(kind), Some(RefusedBy::SubRequest { target })) => {
                    writeln!(f, "refused {kind} on its sub-request `{}`", visible(target))?
                }
                (Some(kind), Some(RefusedBy::Floor { lacking })) => writeln!(
                    f,
                    "refused {kind} by the kernel's floor (it lacks `{}`) — the module's own \
                     rule was not reached",
                    visible(lacking)
                )?,
                (Some(kind), Some(RefusedBy::Untraced)) => {
                    writeln!(f, "refused {kind} (no trace says where)")?
                }
            }
        }
        for out in &declared.opted_out {
            match out.verb {
                Some(verb) => writeln!(
                    f,
                    "opted out: {} {}: {}",
                    visible(&out.endpoint),
                    verb_name(verb),
                    out.reason
                )?,
                None => writeln!(f, "opted out: {}: {}", visible(&out.endpoint), out.reason)?,
            }
        }
        for out in &declared.opted_out_at {
            match out.verb {
                Some(verb) => writeln!(
                    f,
                    "opted out at: `{}` {}: {}",
                    out.pattern,
                    verb_name(verb),
                    out.reason
                )?,
                None => writeln!(f, "opted out at: `{}`: {}", out.pattern, out.reason)?,
            }
        }
        // Grouped by (check, reason): one honest waiver of one check across five
        // endpoints printed its reason verbatim five times and buried every other
        // line of a six-endpoint report. One id reads exactly as it did before.
        let mut groups: Vec<((Check, &str), Vec<&str>)> = Vec::new();
        for out in &declared.opted_out_checks {
            let key = (out.check, out.reason.as_str());
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, ids)) => {
                    if !ids.contains(&out.endpoint.as_str()) {
                        ids.push(&out.endpoint);
                    }
                }
                None => groups.push((key, vec![&out.endpoint])),
            }
        }
        for ((check, reason), ids) in groups {
            writeln!(f, "opted out: {} {}: {reason}", joined(&ids), check.label())?;
        }
        // A waiver that suppressed nothing: a note beside the waiver, never a finding.
        for idle in &declared.evidence.idle_waivers {
            writeln!(
                f,
                "note: the waiver of {} for {} suppressed nothing — the check ran with it set \
                 aside and found nothing; drop it if its reason has passed ({})",
                idle.check.label(),
                visible(&idle.endpoint),
                idle.reason
            )?;
        }
        if !declared.own.is_empty() {
            writeln!(
                f,
                "own: {} ({} endpoint(s) walked)",
                joined(&declared.own),
                self.endpoints
            )?;
            if declared.inherited.is_empty() {
                writeln!(f, "inherited: none")?;
            } else {
                writeln!(
                    f,
                    "inherited: {} — bound beside the module, not its own (`Suite::own`), not walked",
                    joined(&declared.inherited)
                )?;
            }
        }
        for inherited in &declared.evidence.inherited_because {
            writeln!(
                f,
                "inherited because: {}: {}",
                visible(&inherited.endpoint),
                inherited.reason
            )?;
        }
        if declared.required_on_mutating {
            writeln!(
                f,
                "declared: REQUIRED fires `Sink` and `Delete` actions too (`Suite::required_on_mutating`)"
            )?;
        }
        if !declared.pure.is_empty() {
            writeln!(f, "declared pure: {}", joined(&declared.pure))?;
        }
        if !declared.cacheable.is_empty() {
            writeln!(f, "declared cacheable: {}", joined(&declared.cacheable))?;
        }
        if !declared.live.is_empty() {
            writeln!(f, "declared live: {}", joined(&declared.live))?;
        }
        if !declared.namespaces.is_empty() {
            writeln!(f, "module namespaces: {}", declared.namespaces.join(" "))?;
        }
        for v in &declared.vocabularies {
            writeln!(
                f,
                "module vocabulary: {} ({} term{})",
                v.namespaces.join(" "),
                v.terms,
                if v.terms == 1 { "" } else { "s" }
            )?;
        }
        for fixture in &declared.fixtures {
            writeln!(f, "fixture: {fixture}")?;
        }
        // What SPACE-NAME looked at. A module that declares no space is held to
        // nothing, and a clean report must not read as one whose name was checked.
        if self.checks.contains(Check::SpaceName) && declared.spaces.is_empty() {
            writeln!(
                f,
                "space: none declared — SPACE-NAME checked no constructor (declare one with \
                 `Suite::self_named_space` or `Suite::host_named_space`)"
            )?;
        }
        for space in &declared.spaces {
            match space.naming {
                SpaceNaming::SelfNamed => {
                    write!(
                        f,
                        "space: {} self-named by `{}`",
                        space.label, space.constructor
                    )?;
                    match space.doors {
                        Some(doors) => {
                            write!(f, ": two calls, {doors} door(s) compared")?;
                            if space.opaque > 0 {
                                write!(
                                    f,
                                    "; {} opaque node(s), whose doors only `entries()` can see",
                                    space.opaque
                                )?;
                            }
                        }
                        None => write!(f, ": not compared")?,
                    }
                }
                SpaceNaming::HostNamed => write!(f, "space: {} host-named", space.label)?,
            }
            writeln!(f)?;
        }
        // A collapse is coverage information, not a finding: it says the walk fired
        // once where the space offered the same request more than once, and names
        // the bindings so a reader can see the duplication is the SPACE's.
        for c in self.collapsed.iter() {
            writeln!(
                f,
                "collapsed: {} {} <{}>: {} binding(s) issue the identical request ({}) — fired once",
                visible(&c.endpoint),
                verb_name(c.verb),
                c.target,
                c.patterns.len(),
                c.patterns
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        for u in self.unprobed.iter() {
            writeln!(
                f,
                "unprobed: {} {} {}: {}",
                visible(&u.endpoint),
                verb_name(u.verb),
                u.check.label(),
                u.reason
            )?;
        }
        Ok(())
    }
}

/// `s` with every control character escaped, so a name cannot break the report line
/// it is printed on. Ordinary Unicode is left as it is, so an ordinary id prints
/// exactly as declared.
pub(crate) fn visible(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Failure classes as their words, comma-separated; an empty declaration is "only the
/// implied classes" (`permanent`, and `denied` beside a `requires`).
pub(crate) fn class_words(classes: &[FailureClass]) -> String {
    if classes.is_empty() {
        return "only the implied classes".to_string();
    }
    classes
        .iter()
        .map(|c| c.word())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ids joined by a space, each [`visible`].
fn joined<S: AsRef<str>>(ids: &[S]) -> String {
    ids.iter()
        .map(|id| visible(id.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn verb_name(verb: Verb) -> &'static str {
    match verb {
        Verb::Source => "source",
        Verb::Sink => "sink",
        Verb::Exists => "exists",
        Verb::Delete => "delete",
        Verb::Meta => "meta",
    }
}

/// What [`Report::assert_clean_with`] holds a clean report to, beyond cleanliness:
/// the shape of the walk. Every expectation is off until set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Expected {
    /// [`Report::endpoints`], exactly.
    pub endpoints: Option<usize>,
    /// [`Report::actions`], exactly.
    pub actions: Option<usize>,
    /// [`Report::walked`], as a set (walk order is catalog order, so it is not compared).
    pub walked: Option<Vec<String>>,
    /// No `opt_out`, `opt_out_at` or `opt_out_check` at all.
    pub no_opt_outs: bool,
    /// No `unprobed:` line: every check observed every action it applies to.
    pub nothing_unprobed: bool,
}

impl Expected {
    /// Nothing expected beyond a clean report.
    pub fn new() -> Self {
        Expected::default()
    }

    /// Exactly `n` endpoints walked ([`Report::endpoints`]).
    pub fn endpoints(mut self, n: usize) -> Self {
        self.endpoints = Some(n);
        self
    }

    /// Exactly `n` actions examined ([`Report::actions`]).
    pub fn actions(mut self, n: usize) -> Self {
        self.actions = Some(n);
        self
    }

    /// Exactly these description ids walked, in any order ([`Report::walked`]).
    pub fn walked<I>(mut self, ids: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.walked = Some(ids.into_iter().map(|id| id.as_ref().to_string()).collect());
        self
    }

    /// No opt-out of any kind (`opt_out`, `opt_out_at`, `opt_out_check`).
    pub fn no_opt_outs(mut self) -> Self {
        self.no_opt_outs = true;
        self
    }

    /// No action left unprobed by any check.
    pub fn nothing_unprobed(mut self) -> Self {
        self.nothing_unprobed = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_line_leads_with_the_endpoint_id() {
        let f = Finding::new(
            "cal",
            Some(Verb::Sink),
            Check::Pipeline,
            "no `content` input",
        );
        assert_eq!(f.to_string(), "cal  PIPELINE  sink: no `content` input");
        let f = Finding::new("cal", None, Check::Names, "not kebab-case");
        assert_eq!(f.to_string(), "cal  NAMES  not kebab-case");
    }

    #[test]
    fn every_id_the_report_prints_is_escaped() {
        // An id is the endpoint's own string; a control character in it must never
        // split a report line (ledger #1392). One id with a newline, at every place
        // the report prints an id, and no line may begin with its second half.
        let id = "two\nlines";
        let report = Report {
            findings: vec![Finding::new(id, None, Check::Names, "not kebab-case")],
            endpoints: 1,
            actions: 1,
            walked: vec![id.to_string()].into_boxed_slice(),
            collapsed: vec![Collapsed {
                endpoint: id.into(),
                verb: Verb::Source,
                target: "urn:example:split".into(),
                patterns: vec!["urn:example:split".into(), "urn:example:split".into()],
            }]
            .into_boxed_slice(),
            checks: Checks::all(),
            declared: Box::new(Declarations {
                opted_out: vec![
                    OptedOut {
                        endpoint: id.into(),
                        verb: Some(Verb::Sink),
                        reason: "a verb".into(),
                    },
                    OptedOut {
                        endpoint: id.into(),
                        verb: None,
                        reason: "the whole endpoint".into(),
                    },
                ],
                opted_out_at: vec![],
                opted_out_checks: vec![OptedOutCheck {
                    endpoint: id.into(),
                    check: Check::Vocabulary,
                    reason: "one check".into(),
                }],
                required_on_mutating: false,
                own: vec![id.into()],
                inherited: vec![id.into()],
                pure: vec![id.into()],
                cacheable: vec![id.into()],
                live: vec![id.into()],
                namespaces: vec![],
                vocabularies: vec![],
                fixtures: vec![Fixture::new(id, Verb::Source)],
                spaces: vec![],
                evidence: Evidence {
                    label: None,
                    enforced_under: vec![],
                    recomputed: vec![],
                    named_only: vec![],
                    check_failures: false,
                    failures: vec![],
                    fails_under: vec![],
                    required: vec![RequiredProbe {
                        endpoint: id.into(),
                        verb: Verb::Source,
                        input: "in".into(),
                        refusal: None,
                        group: None,
                        given: vec![],
                        contract_refuses: true,
                    }],
                    required_nothing: None,
                    idle_waivers: vec![OptedOutCheck {
                        endpoint: id.into(),
                        check: Check::Names,
                        reason: "an idle one".into(),
                    }],
                    inherited_because: vec![Inherited {
                        endpoint: id.into(),
                        reason: "a dependency".into(),
                    }],
                },
            }),
            unprobed: Box::new(vec![Unprobed {
                endpoint: id.into(),
                verb: Verb::Delete,
                check: Check::Outputs,
                reason: "never fired".into(),
            }]),
            probed: vec![Probed {
                endpoint: id.into(),
                verb: Verb::Source,
                face: "text/plain".into(),
                triples: 0,
                bytes: 3,
            }],
        };
        let text = report.to_string();
        assert!(
            !text.contains(id),
            "the raw id never reaches the text:\n{text}"
        );
        for line in text.lines() {
            assert!(!line.starts_with("lines"), "a split line:\n{text}");
        }
        let shown = "two\\u{a}lines";
        for expected in [
            format!("{shown}  NAMES  not kebab-case\n"),
            format!("probed: {shown} source `text/plain`: 3 byte(s)\n"),
            format!("opted out: {shown} sink: a verb\n"),
            format!("opted out: {shown}: the whole endpoint\n"),
            format!("opted out: {shown} VOCABULARY: one check\n"),
            format!("declared pure: {shown}\n"),
            format!("declared cacheable: {shown}\n"),
            format!("declared live: {shown}\n"),
            format!("fixture: {shown} source\n"),
            format!("collapsed: {shown} source <urn:example:split>: 2 binding(s)"),
            format!("unprobed: {shown} delete OUTPUTS: never fired\n"),
            format!("own: {shown} (1 endpoint(s) walked)\n"),
            format!("inherited: {shown} — bound beside the module"),
            format!("inherited because: {shown}: a dependency\n"),
            format!("required: {shown} source without `in`: resolved"),
            format!("note: the waiver of NAMES for {shown} suppressed nothing"),
        ] {
            assert!(text.contains(&expected), "missing {expected:?}:\n{text}");
        }
        // The struct keeps the id as declared: escaping is the printer's job only.
        assert_eq!(report.against(id).count(), 1);
    }

    #[test]
    fn the_report_prints_skipped_checks_and_opt_outs() {
        let report = Report {
            findings: vec![],
            endpoints: 2,
            actions: 3,
            walked: vec!["cms-graph".to_string(), "ik-context".to_string()].into_boxed_slice(),
            collapsed: Box::default(),
            checks: Checks::all() - Checks::RDF,
            declared: Box::new(Declarations {
                opted_out: vec![OptedOut {
                    endpoint: "email-send".into(),
                    verb: Some(Verb::Sink),
                    reason: "sends real mail".into(),
                }],
                opted_out_at: vec![OptedOutAt {
                    pattern: "urn:orgfile:{path}".into(),
                    verb: None,
                    reason: "jailed to the configured org_dir".into(),
                }],
                opted_out_checks: vec![OptedOutCheck {
                    endpoint: "browse-review".into(),
                    check: Check::Vocabulary,
                    reason: "four ik: terms land in the next vocabulary release".into(),
                }],
                required_on_mutating: false,
                own: vec![],
                inherited: vec![],
                pure: vec!["to-upper".into()],
                cacheable: vec!["to-upper".into()],
                live: vec!["clock-now".into()],
                namespaces: vec!["urn:example:ns#".into()],
                vocabularies: vec![DeclaredVocabulary {
                    namespaces: vec!["https://example.org/log#".into()],
                    terms: 12,
                }],
                fixtures: vec![Fixture::new("file", Verb::Source)
                    .binding("path", "README.md")
                    .arg(
                        "content",
                        "a body that is long enough to be cut short in the report",
                    )],
                spaces: vec![
                    DeclaredSpace {
                        label: "urn:iki:space:text".into(),
                        naming: SpaceNaming::SelfNamed,
                        constructor: "ikigai_text::space".into(),
                        doors: Some(8),
                        opaque: 0,
                    },
                    DeclaredSpace {
                        label: "space(root)".into(),
                        naming: SpaceNaming::HostNamed,
                        constructor: String::new(),
                        doors: None,
                        opaque: 0,
                    },
                ],
                evidence: Evidence {
                    label: None,
                    enforced_under: vec![],
                    recomputed: vec![],
                    named_only: vec![],
                    check_failures: false,
                    failures: vec![],
                    fails_under: vec![],
                    required: vec![
                        RequiredProbe {
                            endpoint: "cms-graph".into(),
                            verb: Verb::Source,
                            input: "in".into(),
                            refusal: Some("missing-argument".into()),
                            group: None,
                            given: vec![],
                            contract_refuses: true,
                        },
                        RequiredProbe {
                            endpoint: "to-upper".into(),
                            verb: Verb::Source,
                            input: "in".into(),
                            refusal: None,
                            group: None,
                            given: vec![],
                            contract_refuses: true,
                        },
                    ],
                    inherited_because: vec![],
                    required_nothing: None,
                    idle_waivers: vec![OptedOutCheck {
                        endpoint: "to-upper".into(),
                        check: Check::Required,
                        reason: "sign types its refusals in 0.4".into(),
                    }],
                },
            }),
            unprobed: Box::new(vec![Unprobed {
                endpoint: "notes-delete".into(),
                verb: Verb::Delete,
                check: Check::Outputs,
                reason: "never fired under root".into(),
            }]),
            probed: vec![
                Probed {
                    endpoint: "cms-graph".into(),
                    verb: Verb::Source,
                    face: "text/turtle".into(),
                    triples: 14,
                    bytes: 512,
                },
                Probed {
                    endpoint: "ik-context".into(),
                    verb: Verb::Source,
                    face: "application/ld+json".into(),
                    triples: 0,
                    bytes: 2,
                },
                Probed {
                    endpoint: "to-upper".into(),
                    verb: Verb::Source,
                    face: "text/plain".into(),
                    triples: 0,
                    bytes: 3,
                },
            ],
        };
        // The Err of `check` travels by value: keep it under clippy's large-error bar.
        assert!(std::mem::size_of::<Report>() <= 128);
        let text = report.to_string();
        assert!(text.contains("0 finding(s) across 2 endpoint(s), 3 action(s)"));
        assert!(text.contains("skipped: SKOLEM-RDF VOCABULARY"));
        assert!(text.contains("opted out: email-send sink: sends real mail"));
        assert!(
            text.contains(
                "opted out: browse-review VOCABULARY: four ik: terms land in the next \
                 vocabulary release"
            ),
            "{text}"
        );
        assert!(text.contains("declared pure: to-upper"));
        assert!(text.contains("declared live: clock-now"), "{text}");
        assert!(
            text.contains("probed 3 face(s) across 3 endpoint(s)"),
            "{text}"
        );
        assert!(
            text.contains("probed: to-upper source `text/plain`: 3 byte(s)\n"),
            "a face no check parses is evidence in BYTES:\n{text}"
        );
        assert!(
            text.contains("probed: cms-graph source `text/turtle`: 14 triple(s)\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was \
                 checked"
            ),
            "a vacuous pass is visible:\n{text}"
        );
        assert!(
            text.contains("fixture: file source path=\"README.md\" content=\"a body that is long"),
            "{text}"
        );
        assert!(
            text.contains("chars)"),
            "a long value is cut, and says so:\n{text}"
        );
        assert!(
            text.contains("unprobed: notes-delete delete OUTPUTS: never fired under root"),
            "{text}"
        );
        assert!(
            text.contains(
                "space: urn:iki:space:text self-named by `ikigai_text::space`: two calls, 8 \
                 door(s) compared\n"
            ),
            "{text}"
        );
        assert!(text.contains("space: space(root) host-named\n"), "{text}");
        assert!(
            text.contains("required: dropped 2 input(s) across 2 endpoint(s)\n"),
            "{text}"
        );
        assert!(
            text.contains("required: cms-graph source without `in`: refused missing-argument\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "required: to-upper source without `in`: resolved — declared required, \
                 actually optional\n"
            ),
            "{text}"
        );
        assert!(!text.contains("required: nothing dropped"), "{text}");
        assert!(
            text.contains(
                "note: the waiver of REQUIRED for to-upper suppressed nothing — the check ran \
                 with it set aside and found nothing; drop it if its reason has passed (sign \
                 types its refusals in 0.4)\n"
            ),
            "{text}"
        );
        assert_eq!(report.required_probes().len(), 2);
        assert_eq!(report.idle_waivers().len(), 1);
        assert_eq!(report.required_nothing(), None);
        assert!(!text.contains("space: none declared"), "{text}");
        assert!(report.is_clean());
        assert!(report.into_result().is_ok());
    }
}

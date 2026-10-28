//! The walk: every endpoint the kernel lists, every action each declares, every
//! selected check — findings accumulated, never short-circuited.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use ikigai_core::{
    space_iri, ActionSpec, ArgRef, ArgSpec, Arguments, Capability, Description, Error, Expiry,
    FailureClass, GroupKind, InputGroup, InputSource, Invocation, Iri, IssueContext, Kernel,
    OutputFrom, ProposedArgs, Representation, Request, Resolution, Scope, SealOwner, Space,
    SpaceEntry, SpaceKind, Thread, Topology, TraceEvent, Tracer, UriTemplate, Verb, DENIED_NOTE,
    FAILED_NOTE, SPACE_PREFIX,
};

use crate::checks::{Check, Checks};
use crate::rdf;
use crate::report::{
    Collapsed, Declarations, DeclaredSpace, DeclaredVocabulary, EnforcedUnder, Evidence,
    FailsUnder, FailuresSeen, Finding, Inherited, NamedOnly, OptedOut, OptedOutAt, OptedOutCheck,
    Probed, Recomputed, RefusedBy, Report, RequiredProbe, SpaceNaming, Unprobed,
};

/// Whether an entry belongs to core, according to the kernel under test. Host
/// and level seals protect module endpoints too; those must still be walked.
fn core_owned(pattern: &str, seals: &[(String, SealOwner)]) -> bool {
    seals
        .iter()
        .any(|(prefix, owner)| *owner == SealOwner::Core && pattern.starts_with(prefix))
}

/// The XSD namespace, for deriving a plausible scalar from a declared `class`.
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The universal conneg selector: an argument every endpoint may be sent and none
/// declares, so a fixture naming it is never an undeclared input.
const CONNEG: &str = "as";

/// The value substituted for an untyped input or template variable.
const PLACEHOLDER: &str = "x";

/// The IRI substituted for an entity-valued (`rdfs:Class`) or `xsd:anyURI` input.
const PLACEHOLDER_IRI: &str = "urn:example:conformance";

/// The longest fixture value a report line prints before cutting it and saying so.
const SHOWN: usize = 48;

/// How many leading bytes a report line shows of a fixture value that is not UTF-8.
const SHOWN_BYTES: usize = 16;

/// How many times [`Check::Recompute`] cuts a cached answer's threads and recomputes
/// it, holding every computation to the first one's bytes (ledger #1573).
///
/// One recomputation caught a `HashMap`-order defect only by luck: a new map gets a
/// fresh `RandomState`, so two computations over a two-entry map agree on the order
/// about half the time (0.508 measured over four million pairs), and ikigai-jsonld's
/// two-node fixture was caught in 5 runs of 12. A miss now needs every recomputation
/// to agree with the first: for that worst case, the smallest map whose order can
/// differ, 24 agree about 6 times in 10 million (111 in 200 million, measured; more
/// than 0.508^24, because the order is not a fair coin: a two-entry map iterates in
/// insertion order 56% of the time, and 0.563^25 is the same 6 in 10 million). A cut and a resolution re-randomize the order already:
/// each computation builds its own map, so no fresh kernel is needed (and a suite is
/// handed its kernel, it cannot build one). A clean answer costs all 24; a defect
/// stops at the first that differs.
const RECOMPUTATIONS: usize = 24;

/// The by-value arguments of one request, by name. Bytes, because that is what the
/// wire carries (`ArgRef::Inline`): a gzip stream or a PNG is a fixture value too
/// ([`Fixture::arg_bytes`]).
type Args = BTreeMap<String, Vec<u8>>;

/// The endpoint field of a finding about the suite itself rather than an endpoint —
/// a registered namespace has no id. Matches `(kernel)`, used for a root space that
/// cannot be enumerated.
const SUITE: &str = Finding::SUITE_WIDE;

/// Module-supplied inputs for one action, for when the ArgSpecs cannot say what a
/// valid call looks like (a `path` that must exist, a JSON-LD document that must
/// parse). Named arguments override the derived minimal ones; bindings fill the
/// entry's template variables.
///
/// ```
/// use ikigai_conformance::Fixture;
/// use ikigai_core::Verb;
///
/// let f = Fixture::new("jsonld-expand", Verb::Source)
///     .arg("content", r#"{"@id": "urn:x", "http://purl.org/dc/terms/title": "t"}"#);
/// let g = Fixture::new("file", Verb::Source).binding("path", "README.md");
/// assert_eq!(f.id(), "jsonld-expand");
/// assert_eq!(g.id(), "file");
/// ```
///
/// A fixture can also name the CALLER ([`capability`](Fixture::capability)), for an
/// endpoint that answers about whoever asks and refuses root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    id: String,
    verb: Verb,
    args: Args,
    bindings: BTreeMap<String, String>,
    capability: Option<Capability>,
}

impl Fixture {
    /// A fixture for the action `(id, verb)`, where `id` is the endpoint's
    /// [`Description::id`].
    pub fn new(id: impl Into<String>, verb: Verb) -> Self {
        Fixture {
            id: id.into(),
            verb,
            args: BTreeMap::new(),
            bindings: BTreeMap::new(),
            capability: None,
        }
    }

    /// Supply (or override) one by-value argument.
    ///
    /// `name` must be an input the action declares by value, or `as` (the conneg
    /// selector, which no endpoint declares). Any other name is still sent, and
    /// [`Check::Declarations`](crate::Check::Declarations) reports it: either the
    /// endpoint reads an input its contract does not declare, or it ignored the
    /// value and the walk tested something other than what the fixture says. A
    /// template variable is a [`binding`](Fixture::binding), never an argument.
    pub fn arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.args.insert(name.into(), value.into().into_bytes());
        self
    }

    /// Supply (or override) one by-value argument as BYTES, for an endpoint whose
    /// input is not text: a decompressor's gzip stream, an image a sniffer reads.
    /// Everything [`arg`](Fixture::arg) says holds; the value reaches the endpoint
    /// byte for byte (`ArgRef::Inline`), exactly as `arg`'s UTF-8 does.
    ///
    /// Before 0.8.1 a fixture value had to be a `String`, so an endpoint that reads
    /// binary input could not be given one it accepts: every invoking check reported
    /// its refusal of the suite's text sample, and the module opted it out of all of
    /// them (`REQUIRED` included) and wrote them by hand (ledger #1493).
    ///
    /// The report prints a value that is not UTF-8 as its first 16 bytes in hex and
    /// its length; one that is UTF-8 prints exactly as `arg`'s would.
    ///
    /// ```
    /// use ikigai_conformance::Fixture;
    /// use ikigai_core::Verb;
    ///
    /// // The 20-byte gzip of an empty input.
    /// let empty_gzip: &[u8] = &[
    ///     0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x03, 0x00, 0x00, 0x00,
    ///     0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    /// ];
    /// let f = Fixture::new("gunzip", Verb::Source).arg_bytes("content", empty_gzip);
    /// assert_eq!(f.to_string(), "gunzip source content=0x1f8b0800000000000003030000000000… (20 bytes)");
    /// // UTF-8 bytes read exactly as `arg` would print them.
    /// let g = Fixture::new("upper", Verb::Source).arg_bytes("in", b"abc".to_vec());
    /// assert_eq!(g, Fixture::new("upper", Verb::Source).arg("in", "abc"));
    /// ```
    pub fn arg_bytes(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
        self.args.insert(name.into(), value.into());
        self
    }

    /// Supply the value of one template variable of the entry this endpoint is
    /// bound at (`urn:file:{path}` → `binding("path", …)`).
    ///
    /// Bindings are looked up per ENTRY, not per action: every verb of the entry
    /// resolves the same IRI, and the verb a binding-only fixture was built with
    /// is ignored (the first fixture for the id that binds the variable wins).
    /// Arguments ARE per `(id, verb)`. So a Sink that writes a scratch file while
    /// Source reads a seeded one cannot be expressed as two bindings; give every
    /// verb's fixture the same value.
    ///
    /// A binding naming a variable no pattern of that id has — a typo, or a
    /// variable that was renamed — is reported by
    /// [`Check::Declarations`](crate::Check::Declarations): it used to be dropped
    /// silently, and the IRI was formed from the ArgSpec's derived value instead.
    pub fn binding(mut self, var: impl Into<String>, value: impl Into<String>) -> Self {
        self.bindings.insert(var.into(), value.into());
        self
    }

    /// Probe this action AS a caller holding `capability`, in place of root.
    ///
    /// For an endpoint about its caller: one that answers for the principal a door
    /// minted (`urn:layers:me`) and refuses a request naming none, root included,
    /// since root holds every principal and names none. Under root such an endpoint
    /// refuses every probe, so `OUTPUTS`, `SKOLEM-RDF`, `VOCABULARY` and `CACHEABLE`
    /// could only report the refusal or be waived, and a [`Suite::live`] held it to
    /// nothing. Named here, the walk resolves the action as that caller and every one
    /// of those checks judges what it served.
    ///
    /// Mint it the way the door does: the scopes the caller is granted plus its
    /// principal (`Capability::scoped([...]).with_principal(iri)` on core 0.1.93+, or
    /// the `urn:cap:principal:<iri>` scope spelled out). Root stays the default: a
    /// fixture without a capability, and every action no fixture names, probes under
    /// root exactly as before.
    ///
    /// Per `(id, verb)`, like [`arg`](Fixture::arg). It is the caller of every probe
    /// that resolves the action to judge an answer: the minimal resolution (and the
    /// cache probe's second one, which the kernel keys on this same capability), each
    /// extra RDF face, and the pipeline probe's firing of a mutating action that
    /// declares `content`. It is NOT the caller of `ENFORCED` and `AUTHORITY`, which
    /// ask what the action does under a capability holding no grants and so carry no
    /// principal either: an endpoint that refuses an anonymous caller declares that
    /// gate (`urn:cap:principal:*` is the presence test) or `ENFORCED` reports the
    /// undeclared refusal, as it always has. A capability no probe resolved under (a
    /// mutating action without `content`, which is fired only under no grants) is
    /// reported by [`Check::Declarations`](crate::Check::Declarations).
    ///
    /// ```
    /// use ikigai_conformance::Fixture;
    /// use ikigai_core::{Capability, Verb};
    ///
    /// // A participant holding the read scope, named as a door would name it.
    /// let caller = Capability::scoped([
    ///     "urn:cap:collab:read:demo",
    ///     "urn:cap:principal:urn:example:person:alice",
    /// ]);
    /// let me = Fixture::new("me", Verb::Source).capability(caller.clone());
    /// assert_eq!(me.caller(), Some(&caller));
    /// assert_eq!(Fixture::new("me", Verb::Source).caller(), None); // root
    /// ```
    pub fn capability(mut self, capability: Capability) -> Self {
        self.capability = Some(capability);
        self
    }

    /// The capability this fixture probes its action under, or `None` for root (the
    /// default). See [`capability`](Fixture::capability).
    pub fn caller(&self) -> Option<&Capability> {
        self.capability.as_ref()
    }

    /// The endpoint id this fixture is for.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The verb this fixture is for.
    pub fn verb(&self) -> Verb {
        self.verb
    }
}

/// The report line for a fixture: `<id> <verb> <var>=<value>… <name>=<value>…`,
/// bindings first, values in their escaped string form so a line stays a line, cut
/// after 48 characters with the true length stated. A named caller comes last, as
/// `as-caller=` and its principal (or its scope count when it names none), so the
/// record says the action was probed as someone other than root.
///
/// ```
/// use ikigai_conformance::Fixture;
/// use ikigai_core::{Capability, Verb};
///
/// let f = Fixture::new("file", Verb::Sink).binding("path", "scratch.txt").arg("content", "a\nb");
/// assert_eq!(f.to_string(), r#"file sink path="scratch.txt" content="a\nb""#);
/// let long = Fixture::new("doc", Verb::Source).arg("content", "x".repeat(200));
/// assert!(long.to_string().ends_with("… (202 chars)"), "{long}");
/// let me = Fixture::new("me", Verb::Source).capability(Capability::scoped([
///     "urn:cap:collab:read:demo",
///     "urn:cap:principal:urn:example:person:alice",
/// ]));
/// assert_eq!(me.to_string(), "me source as-caller=urn:example:person:alice (2 scopes)");
/// let anonymous = Fixture::new("me", Verb::Source).capability(Capability::scoped(["urn:cap:a"]));
/// assert_eq!(anonymous.to_string(), "me source as-caller=no principal (1 scope)");
/// ```
impl fmt::Display for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            crate::report::visible(&self.id),
            crate::report::verb_name(self.verb)
        )?;
        for (var, value) in &self.bindings {
            write!(f, " {var}={}", shown(value))?;
        }
        for (name, value) in &self.args {
            write!(f, " {name}={}", shown_bytes(value))?;
        }
        if let Some(cap) = &self.capability {
            match cap.scopes() {
                None => write!(f, " as-caller=root")?,
                Some(scopes) => {
                    let who = principal_named(scopes).unwrap_or("no principal");
                    let n = scopes.len();
                    write!(
                        f,
                        " as-caller={} ({n} scope{})",
                        shown_bare(who),
                        if n == 1 { "" } else { "s" }
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// The principal a scope set names: its one `urn:cap:principal:<iri>` scope that is
/// not a wildcard, as `Capability::principal` reads it on core 0.1.93+ (several name
/// none). Spelled out here so the floor stays the API this crate otherwise uses.
fn principal_named(scopes: &BTreeSet<String>) -> Option<&str> {
    let mut named = scopes
        .iter()
        .filter_map(|s| s.strip_prefix("urn:cap:principal:"))
        .filter(|p| !p.contains('*'));
    let one = named.next()?;
    named.next().is_none().then_some(one)
}

/// An IRI as one printable token, unquoted: escaped, and cut like [`shown`].
fn shown_bare(value: &str) -> String {
    let escaped = value.escape_debug().to_string();
    let len = escaped.chars().count();
    if len <= SHOWN {
        return escaped;
    }
    let cut: String = escaped.chars().take(SHOWN).collect();
    format!("{cut}… ({len} chars)")
}

/// A by-value fixture argument as one printable token: as [`shown`] when it is
/// UTF-8, else its leading bytes in hex with its length.
fn shown_bytes(value: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(value) {
        return shown(text);
    }
    let hex: String = value
        .iter()
        .take(SHOWN_BYTES)
        .map(|b| format!("{b:02x}"))
        .collect();
    let more = if value.len() > SHOWN_BYTES { "…" } else { "" };
    format!("0x{hex}{more} ({} bytes)", value.len())
}

/// A fixture value as one printable token: escaped, and cut when long.
fn shown(value: &str) -> String {
    let escaped = format!("{value:?}");
    let len = escaped.chars().count();
    if len <= SHOWN {
        return escaped;
    }
    let cut: String = escaped.chars().take(SHOWN).collect();
    format!("{cut}… ({len} chars)")
}

#[derive(Clone, Debug)]
struct OptOut {
    id: String,
    verb: Option<Verb>,
    reason: String,
}

#[derive(Clone, Debug)]
struct OptOutAt {
    pattern: String,
    verb: Option<Verb>,
    reason: String,
}

/// The identity of a FIRING: the request the walk would issue for one action of one
/// bound entry — the verb, the target IRI, and the arguments derived from the
/// ArgSpecs and the fixtures.
///
/// ★ **This is the key the invoking checks memoize on, and `Description::id` is not.**
/// An id is a name for a KIND of endpoint, not for a thing that can be fired: two
/// entries sharing one description can be two instances over different state
/// (`urn:file:{path}` jailed to a scratch root, `urn:orgfile:{path}` jailed to
/// whatever the config names — with no config at all, the process's working
/// directory). Memoizing on the id would collapse those to whichever the walk
/// reached first: either the dangerous entry goes unprobed and the report says green
/// about something it never looked at, or it is fired and the safe entry's coverage
/// is lost. A request is the opposite: two entries agreeing on it agree on every byte
/// the walk would send, so firing the second can differ from firing the first only in
/// the state the first one left — which is exactly the defect.
///
/// What it does NOT cover: two DIFFERENT requests that reach one state. An alias
/// spelled as a second binding (`urn:iki:ledger:append` beside
/// `urn:iki:ledger:{name}:append`) is two IRIs, and to the kernel that is two
/// resources — two cache entries, two golden threads — so the walk probes both, and
/// is right to. A module that means them to be ONE resource says so where the kernel
/// can see it: one `Grammar` matching both spellings, or core's `Alias`. A module
/// that means them to be two and wants only one probed says so with
/// [`Suite::opt_out_at`].
///
/// `Verb` is not `Ord`, so the verb is keyed by its printed name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Firing {
    target: String,
    verb: &'static str,
    args: Args,
}

/// One firing the walk considered, and every binding that would have issued it.
#[derive(Debug)]
struct Fired {
    endpoint: String,
    verb: Verb,
    target: String,
    patterns: Vec<String>,
}

/// What each [`Suite::opt_out_check`] waiver would have let through, counted as the
/// walk goes: findings a waived check produced and the walk set aside, and the
/// waivers no set-aside run could judge.
#[derive(Debug, Default)]
struct Tally {
    /// Findings set aside, per waived `(id, check)`.
    suppressed: BTreeMap<(String, Check), usize>,
    /// The invoking `(id, check)` pairs at least one set-aside run judged.
    judged: BTreeSet<(String, Check)>,
    /// The pairs some set-aside run could not judge: it needed a `Sink` or `Delete`
    /// firing, or could not observe the action.
    unjudged: BTreeSet<(String, Check)>,
}

impl Tally {
    /// Record what one waived run saw: `Some(n)` findings, or `None` when it could
    /// not tell.
    fn record(&mut self, id: &str, check: Check, seen: Option<usize>) {
        let key = (id.to_string(), check);
        match seen {
            Some(n) => {
                *self.suppressed.entry(key.clone()).or_default() += n;
                self.judged.insert(key);
            }
            None => {
                self.unjudged.insert(key);
            }
        }
    }

    /// Whether the waiver of `check` for `id` was judged and suppressed nothing. An
    /// invoking check needs a set-aside run to have judged it; a description-only
    /// one ran wherever the walk reached the id.
    fn idle(&self, id: &str, check: Check) -> bool {
        let key = (id.to_string(), check);
        if self.unjudged.contains(&key) || self.suppressed.get(&key).is_some_and(|n| *n > 0) {
            return false;
        }
        !check.invokes() || self.judged.contains(&key)
    }
}

/// What the walk actually covered, recorded as it goes — the evidence
/// [`Check::Declarations`] reads to say which declarations were never consulted.
///
/// A declaration is a promise about a check that will honor it. When the target of
/// that promise is never reached — a `live` on a Sink, a fixture whose id is a typo,
/// a waiver for a check that was not selected — the declaration is inert, and the
/// report prints it as if it had run. This is the ledger that makes the difference
/// visible.
#[derive(Debug, Default)]
struct Coverage {
    /// Description ids the walk reached.
    walked: BTreeSet<String>,
    /// Every `(id, verb)` action a walked description declared, in walk order
    /// (`Verb` is not `Ord`, so these are vectors kept unique on insert).
    actions: Vec<(String, Verb)>,
    /// The actions the walk skipped because of [`Suite::opt_out`].
    opted_out: Vec<(String, Verb)>,
    /// The actions for which the invoking checks actually ran, at some binding.
    /// Distinct from the complement of `opted_out`: [`Suite::opt_out_at`] excludes
    /// one BINDING, and the same action may still be invoked at another.
    invoked: Vec<(String, Verb)>,
    /// Ids declaring at least one RDF face, the only thing the graph checks probe.
    rdf_faces: BTreeSet<String>,
    /// The template variables of every pattern an id is bound at.
    vars: BTreeMap<String, BTreeSet<String>>,
    /// Every pattern the walk reached, in walk order — what an `opt_out_at` names,
    /// and the list a finding about one that named nothing has to print.
    patterns: Vec<String>,
    /// The `opt_out_at` patterns that excluded at least one action of one entry.
    patterns_excluded: BTreeSet<String>,
    /// The ids at least one binding of which an `opt_out_at` excluded.
    ids_excluded_at: BTreeSet<String>,
    /// Ids for which `CACHEABLE` ran at all — past its non-cacheable-verb guard,
    /// which is where `live` and `cacheable` are read.
    cacheable_ran: BTreeSet<String>,
    /// Ids whose result came back cacheable, so the golden-thread rule — the one
    /// `pure` exempts — actually applied.
    pure_consulted: BTreeSet<String>,
    /// Ids whose minimal resolution failed under CACHEABLE: nothing can be said
    /// about `pure` for them, and a report already carries the real finding.
    cacheable_unresolved: BTreeSet<String>,
    /// Registered namespaces that accounted for at least one term in a probed face.
    namespaces_used: BTreeSet<String>,
    /// The [`Suite::vocabulary`] graphs (by index) under whose namespace at least one
    /// term in a probed face lay, defined or not.
    vocabularies_used: BTreeSet<usize>,
    /// Whether any RDF face was probed at all (an unused namespace means something
    /// different when nothing was parsed).
    faces_probed: bool,
    /// The actions some probe resolved as their caller (root or a fixture's
    /// capability): a fixture capability for an action not in here reached nothing.
    caller_used: Vec<(String, Verb)>,
    /// The input names each walked action declares, by-value and binding apart,
    /// keyed by `(id, verb name)`: what a fixture's argument names are read against.
    inputs: BTreeMap<(String, &'static str), DeclaredInputs>,
    /// Ids with at least one action REQUIRED would examine: a required by-value
    /// input without a default, on a verb it probes.
    required_eligible: BTreeSet<String>,
    /// Whether any walked mutating action declares a required by-value input, the
    /// only thing [`Suite::required_on_mutating`] reaches.
    required_mutating_candidates: bool,
    /// Ids [`Suite::own`] scoped out: bound, and deliberately not walked.
    inherited: BTreeSet<String>,
    /// The [`Suite::enforced_under`] probes (by index) that ran at some binding.
    under_ran: BTreeSet<usize>,
    /// The probes the kernel's floor refused before the endpoint ran, with the scope
    /// the caller lacked: they did not reach the module's own rule.
    under_floor: BTreeMap<usize, String>,
    /// The probes whose bindings formed no IRI at some binding, with why.
    under_untargeted: BTreeMap<usize, String>,
    /// The [`Suite::fails_under`] probes (by index) that ran at some binding.
    fails_ran: BTreeSet<usize>,
    /// The `fails_under` probes whose bindings formed no IRI at some binding, with why.
    fails_untargeted: BTreeMap<usize, String>,
}

/// The input names one action declares: by value (what a fixture argument may
/// name) and by binding (a template variable, which an argument cannot fill).
#[derive(Debug, Default)]
struct DeclaredInputs {
    by_value: BTreeSet<String>,
    binding: BTreeSet<String>,
    /// The action's argument set is OPEN (core 0.1.105, ledger #607): any other name is
    /// an argument too, so a fixture may send one.
    open: bool,
}

impl Coverage {
    /// Record an action of `id`, once.
    fn record_action(&mut self, id: &str, verb: Verb, opted_out: bool) {
        let list = if opted_out {
            &mut self.opted_out
        } else {
            &mut self.actions
        };
        if !list.iter().any(|(i, v)| i == id && *v == verb) {
            list.push((id.to_string(), verb));
        }
    }

    /// Record that the invoking checks ran for an action, once.
    fn record_invoked(&mut self, id: &str, verb: Verb) {
        if !self.invoked.iter().any(|(i, v)| i == id && *v == verb) {
            self.invoked.push((id.to_string(), verb));
        }
    }

    /// Record that a probe resolved an action as its caller, once.
    fn record_caller_used(&mut self, id: &str, verb: Verb) {
        if !self.caller_used.iter().any(|(i, v)| i == id && *v == verb) {
            self.caller_used.push((id.to_string(), verb));
        }
    }

    /// Whether the invoking checks ran for any action of `id`.
    fn any_invoked(&self, id: &str) -> bool {
        self.invoked.iter().any(|(i, _)| i == id)
    }

    /// Whether `(id, verb)` is an action the walk saw declared.
    fn declares(&self, id: &str, verb: Verb) -> bool {
        self.actions.iter().any(|(i, v)| i == id && *v == verb)
    }

    /// Whether `(id, verb)` was excluded by [`Suite::opt_out`].
    fn is_opted_out(&self, id: &str, verb: Option<Verb>) -> bool {
        self.opted_out
            .iter()
            .any(|(i, v)| i == id && verb.is_none_or(|w| w == *v))
    }

    /// The verbs a walked description declared, in verb order.
    fn verbs_of(&self, id: &str) -> Vec<Verb> {
        self.actions
            .iter()
            .filter(|(i, _)| i == id)
            .map(|(_, v)| *v)
            .collect()
    }

    /// Record the input names one action declares.
    fn record_inputs(&mut self, id: &str, spec: &ActionSpec) {
        let declared = self
            .inputs
            .entry((id.to_string(), crate::report::verb_name(spec.verb)))
            .or_default();
        for input in &spec.inputs {
            let names = match input.source {
                InputSource::Binding => &mut declared.binding,
                _ => &mut declared.by_value,
            };
            names.insert(input.name.clone());
        }
        declared.open |= matches!(spec.arguments, Arguments::Open(_));
    }

    /// Whether every action of `id` was excluded by [`Suite::opt_out`].
    fn wholly_opted_out(&self, id: &str) -> bool {
        let verbs = self.verbs_of(id);
        !verbs.is_empty() && verbs.iter().all(|v| self.is_opted_out(id, Some(*v)))
    }
}

/// A configuration-free constructor, called twice by [`Check::SpaceName`].
type MakeSpace = Arc<dyn Fn() -> Arc<dyn Space> + Send + Sync>;

/// One space a module declared to [`Check::SpaceName`].
#[derive(Clone)]
struct SpaceDecl {
    /// What findings and waivers name it by (the IRI, or the host-named label).
    label: String,
    /// The constructor's Rust path, for a self-named space; empty otherwise.
    constructor: String,
    kind: SpaceDeclKind,
}

#[derive(Clone)]
enum SpaceDeclKind {
    /// `space_iri(module)` is claimed by every call of `make`.
    SelfNamed {
        module: String,
        iri: Iri,
        make: MakeSpace,
    },
    /// The space the module built for the host to name: it claims nothing.
    HostNamed { space: Arc<dyn Space> },
}

impl SpaceDecl {
    fn naming(&self) -> SpaceNaming {
        match self.kind {
            SpaceDeclKind::SelfNamed { .. } => SpaceNaming::SelfNamed,
            SpaceDeclKind::HostNamed { .. } => SpaceNaming::HostNamed,
        }
    }
}

/// By hand: a constructor and a space are not `Debug`, and the label says which
/// declaration this is.
impl fmt::Debug for SpaceDecl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpaceDecl")
            .field("label", &self.label)
            .field("constructor", &self.constructor)
            .field("naming", &self.naming())
            .finish()
    }
}

#[derive(Clone)]
struct BodySpace(Arc<dyn Space>);

impl fmt::Debug for BodySpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BodySpace")
    }
}

#[derive(Clone, Debug)]
struct OptOutCheck {
    id: String,
    check: Check,
    reason: String,
}

/// A configured run: which checks, which fixtures, which actions are opted out of
/// being invoked and why, which namespaces are the module's own, and which
/// endpoints are pure.
///
/// [`check`](crate::check) is `Suite::new().run_blocking(kernel).into_result()`;
/// build a `Suite` when a module needs more than the defaults.
///
/// ```no_run
/// use ikigai_conformance::{Checks, Fixture, Suite};
/// use ikigai_core::{Kernel, Verb};
///
/// fn my_kernel() -> Kernel { unimplemented!() }
///
/// let report = Suite::new()
///     .checks(Checks::all() - Checks::NAMES)
///     .fixture(Fixture::new("jsonld-expand", Verb::Source).arg("content", "{}"))
///     .opt_out("email-send", Some(Verb::Sink), "sends real mail")
///     .namespace("https://example.org/ns#")
///     .pure("to-upper")
///     .run_blocking(&my_kernel());
/// report.assert_clean();
/// ```
#[derive(Clone, Debug)]
pub struct Suite {
    /// What [`Suite::checks`] selected.
    checks: Checks,
    check_failures: bool,
    body_space: Option<BodySpace>,
    body_input_store: Option<Arc<ikigai_core::MemoryContentStore>>,
    fixtures: Vec<Fixture>,
    enforced_under: Vec<Fixture>,
    fails_under: Vec<(Fixture, FailureClass)>,
    opt_outs: Vec<OptOut>,
    opt_outs_at: Vec<OptOutAt>,
    opt_out_checks: Vec<OptOutCheck>,
    namespaces: Vec<String>,
    /// What [`Suite::vocabulary`] was given, read once: a graph that does not parse
    /// is kept as its reason, and reported rather than dropped.
    vocabularies: Vec<Result<rdf::Vocabulary, String>>,
    pure: Vec<String>,
    cacheable: Vec<String>,
    live: Vec<String>,
    spaces: Vec<SpaceDecl>,
    kernel_ops: bool,
    required_on_mutating: bool,
    own: Vec<String>,
    inherit: Vec<Inherited>,
    label: Option<String>,
}

impl Default for Suite {
    fn default() -> Self {
        Suite::new()
    }
}

impl Suite {
    /// Every check, no fixtures, no opt-outs, kernel operations skipped.
    /// [`Check::Failures`] runs by default since 0.11.0; [`Check::Bodies`] since
    /// 0.12.0 (supply [`body_space`](Self::body_space) for its direct endpoint calls).
    /// [`Check::BodyInputs`] runs by default since 0.12.0.
    pub fn new() -> Self {
        Suite {
            checks: Checks::all(),
            check_failures: false,
            body_space: None,
            body_input_store: None,
            fixtures: Vec::new(),
            enforced_under: Vec::new(),
            fails_under: Vec::new(),
            opt_outs: Vec::new(),
            opt_outs_at: Vec::new(),
            opt_out_checks: Vec::new(),
            namespaces: Vec::new(),
            vocabularies: Vec::new(),
            pure: Vec::new(),
            cacheable: Vec::new(),
            live: Vec::new(),
            spaces: Vec::new(),
            kernel_ops: false,
            required_on_mutating: false,
            own: Vec::new(),
            inherit: Vec::new(),
            label: None,
        }
    }

    /// Supply the module space used to construct the kernel for [`Check::Bodies`].
    ///
    /// Pass the same shared space, e.g. `.body_space(space.clone())` alongside
    /// `Kernel::new(space)`. Core 0.2.0 keeps the kernel's root private; neither
    /// `Kernel::issue` nor `Kernel::issue_body` can compare the endpoint methods,
    /// because both dispatch `invoke_body`. Without this handle BODIES is reported
    /// as unprobed, never as a successful comparison. Sub-requests use the supplied
    /// kernel, including its other configuration.
    pub fn body_space(mut self, space: impl Space + 'static) -> Self {
        self.body_space = Some(BodySpace(Arc::new(space)));
        self
    }

    /// Supply the same store installed with `Kernel::with_content_store` for
    /// [`Check::BodyInputs`]. The suite holds each sample in it while issuing a
    /// content-id request through the supplied kernel. Without this handle the
    /// stored-content comparison is explicitly unprobed; the reference and attached
    /// comparisons still run. A different store will cause the content probe to fail.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use ikigai_core::{EndpointSpace, Kernel, MemoryContentStore};
    /// use ikigai_conformance::Suite;
    /// let store = Arc::new(MemoryContentStore::new(1024 * 1024));
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new()))
    ///     .with_content_store(store.clone());
    /// let report = Suite::new().body_input_store(store).run_blocking(&kernel);
    /// report.assert_clean();
    /// ```
    pub fn body_input_store(mut self, store: Arc<ikigai_core::MemoryContentStore>) -> Self {
        self.body_input_store = Some(store);
        self
    }

    /// Select exactly the checks to run; the rest are reported as skipped.
    pub fn checks(mut self, checks: Checks) -> Self {
        self.checks = checks;
        self
    }

    /// Record an explicit request for [`Check::Failures`], kept for compatibility.
    ///
    /// The check runs by default since 0.11.0: it holds every action that states its
    /// failure classes to them and runs the [`fails_under`](Self::fails_under) probes.
    /// Subtract [`Checks::FAILURES`] to turn it off. This method records the request
    /// without changing the selected checks. FAILURES fires nothing a walk does not
    /// already fire, but one minimal resolution of a read no other selected check resolved.
    ///
    /// A declaration that reached nothing (FAILURES not selected) is reported by
    /// [`Check::Declarations`].
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks, Suite};
    /// use ikigai_core::{
    ///     ArgSpec, Description, EndpointSpace, Error, Exact, FailureClass, FnEndpoint,
    ///     Invocation, Kernel, Verb,
    /// };
    /// use std::sync::Arc;
    ///
    /// // It says it can be rate-limited, and fails as an opaque `Endpoint` error instead.
    /// let fetch = FnEndpoint::new("fetch", |_: &Invocation<'_>| {
    ///     Err(Error::Endpoint("upstream answered 500".into()))
    /// })
    /// .with_description(
    ///     Description::new("fetch")
    ///         .verb(Verb::Source)
    ///         .input(ArgSpec::new("url").class("http://www.w3.org/2001/XMLSchema#anyURI"))
    ///         .output("text/plain")
    ///         .fails(FailureClass::RetryAfter),
    /// );
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:fetch"), fetch)));
    ///
    /// // Subtracting FAILURES prints it as skipped and judges nothing.
    /// let off = Suite::new().checks(Checks::all() - Checks::FAILURES).run_blocking(&kernel);
    /// assert_eq!(off.of(Check::Failures).count(), 0, "{off}");
    /// assert!(off.to_string().lines().any(|l| l.starts_with("skipped:") && l.ends_with(" FAILURES")), "{off}");
    ///
    /// let on = Suite::new().run_blocking(&kernel);
    /// let finding = on.of(Check::Failures).next().expect("an undeclared class");
    /// assert!(finding.detail.contains("class `unknown`"), "{on}");
    /// ```
    pub fn check_failures(mut self) -> Self {
        self.check_failures = true;
        self
    }

    /// Hold one action to a stated failure class under an input the module supplies
    /// (ledger #1546): the probe must FAIL, with an error whose class
    /// ([`Error::class`](ikigai_core::Error::class)) is `class`. For a class a walk cannot
    /// provoke with minimal inputs: `over-budget` for an input past the endpoint's bound,
    /// `retry-after` from a fake origin answering 429, `refused` from one answering 401.
    ///
    /// The probe is a [`Fixture`], laid over the walk's minimal inputs for the action (a
    /// [`Suite::fixture`]'s included) the way an [`enforced_under`](Self::enforced_under)
    /// probe is; its bindings form the target for this probe alone, and its caller
    /// ([`Fixture::capability`]) is root, or the action's fixture's caller, when it names
    /// none. A probe that RESOLVES, or fails with another class, is a
    /// [`Check::Failures`] finding. Its failure is also an observation like any other, so
    /// an action that states its classes must admit the one the probe provokes.
    ///
    /// It runs where FAILURES runs (selected, not waived for the id) and fires the
    /// action once per probe: a `Sink` or a `Delete` named here is fired. A probe that
    /// never ran is reported by [`Check::Declarations`].
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks, Fixture, Suite};
    /// use ikigai_core::{
    ///     ArgSpec, Description, EndpointSpace, Error, Exact, FailureClass, FnEndpoint,
    ///     Invocation, Kernel, ReprType, Representation, Verb,
    /// };
    /// use std::sync::Arc;
    ///
    /// // A bound that refuses, but as `InvalidArgument`: a caller holding a larger
    /// // bound cannot tell it from a malformed request.
    /// let unzip = FnEndpoint::new("unzip", |inv: &Invocation<'_>| {
    ///     let text = inv.inline_str("content")?;
    ///     if text.len() > 4 {
    ///         return Err(Error::InvalidArgument { name: "content".into(), detail: "too large".into() });
    ///     }
    ///     Ok(Representation::new(ReprType::new("text/plain"), text.as_bytes().to_vec()))
    /// })
    /// .with_description(
    ///     Description::new("unzip")
    ///         .verb(Verb::Source)
    ///         .input(ArgSpec::new("content").class("http://www.w3.org/2001/XMLSchema#string"))
    ///         .output("text/plain")
    ///         .fails(FailureClass::OverBudget),
    /// );
    /// let kernel = Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:unzip"), unzip)));
    /// let report = Suite::new()
    ///     .checks(Checks::FAILURES)
    ///     .fails_under(
    ///         Fixture::new("unzip", Verb::Source).arg("content", "a bomb"),
    ///         FailureClass::OverBudget,
    ///     )
    ///     .run_blocking(&kernel);
    /// let finding = report.of(Check::Failures).next().expect("the wrong class");
    /// assert!(finding.detail.contains("got `invalid-argument` (class `permanent`)"), "{report}");
    /// ```
    pub fn fails_under(mut self, probe: Fixture, class: FailureClass) -> Self {
        self.fails_under.push((probe, class));
        self
    }

    /// Supply inputs for one action.
    pub fn fixture(mut self, fixture: Fixture) -> Self {
        self.fixtures.push(fixture);
        self
    }

    /// Hold one action's OWN access rule to a typed `Denied`, under a caller that
    /// passes the kernel's floor but should not pass the module (ledger #331).
    ///
    /// [`Check::Enforced`] probes every action under a capability holding no grants,
    /// so for an action declaring a parameterized scope (`urn:cap:net:*`,
    /// `urn:cap:secret:read:*`) the kernel refuses first, before the endpoint runs,
    /// and the `Denied` the check sees is the kernel's. The module's own rule (per
    /// host, per name, per prefix: the real gate) is never reached. This probe
    /// reaches it: the caller holds SOME grant under the declared prefix, but not the
    /// one for this target, so the floor lets it through and only the module can
    /// refuse. The module must refuse with a typed `Denied`, on its own path or on a
    /// sub-request it makes; it is a finding when it resolves (the rule let the caller
    /// through) and when it refuses some other way (an `Error::Endpoint`, a refusal
    /// wrapped into prose: the type was lost on the way out).
    ///
    /// The probe is a [`Fixture`], so it says everything a fixture can: the caller
    /// ([`Fixture::capability`], required: a probe without one would run as root,
    /// which holds every grant, so it does not run and
    /// [`Check::Declarations`] says so), and the inputs that reach the gate, which
    /// need not be the walk's. A gate in a SUB-RESOLUTION is reachable only by its own
    /// input shape (an `http(s)://` stylesheet, a program that issues a request, a
    /// remote context), so the probe's arguments are laid over the walk's minimal ones
    /// for this action (a [`Suite::fixture`]'s included) and its bindings form the
    /// target for this probe alone. It does not replace the action's fixture, and an
    /// action can have several probes: one per capability shape worth pinning.
    ///
    /// Opt-in: nothing is probed for an action no `enforced_under` names. It runs
    /// where `ENFORCED` runs (selected, not waived for the id) and fires the action
    /// once per probe, under the probe's caller: a `Sink` or a `Delete` named here is
    /// fired, and fired successfully only if the rule is broken. The trace says where
    /// the refusal came from ([`RefusedBy`]), printed as an `enforced under:` line; a
    /// refusal by the kernel's floor means the probe's caller lacks a declared scope,
    /// so the module's rule was not reached, and [`Check::Declarations`] reports the
    /// probe.
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks, Fixture, Suite};
    /// use ikigai_core::{
    ///     ArgSpec, Capability, Description, EndpointSpace, Error, Exact, FnEndpoint, Invocation,
    ///     Kernel, ReprType, Representation, Verb,
    /// };
    /// use std::sync::Arc;
    ///
    /// // A per-host rule that LOSES the type: it refuses with `Error::Endpoint`.
    /// let fetch = FnEndpoint::new("fetch", |inv: &Invocation<'_>| {
    ///     let host = inv.inline_str("host")?;
    ///     if !inv.capability.allows(&format!("urn:cap:net:{host}")) {
    ///         return Err(Error::Endpoint(format!("could not fetch from {host}")));
    ///     }
    ///     Ok(Representation::new(ReprType::new("text/plain"), b"ok".to_vec()))
    /// })
    /// .with_description(
    ///     Description::new("fetch")
    ///         .verb(Verb::Source)
    ///         .input(ArgSpec::new("host").class("http://www.w3.org/2001/XMLSchema#string"))
    ///         .output("text/plain")
    ///         .requires("urn:cap:net:*"),
    /// );
    /// let space = EndpointSpace::new().bind(Exact::new("urn:example:fetch"), fetch);
    /// let report = Suite::new()
    ///     .checks(Checks::ENFORCED)
    ///     .enforced_under(
    ///         Fixture::new("fetch", Verb::Source)
    ///             .arg("host", "127.0.0.1")
    ///             .capability(Capability::scoped(["urn:cap:net:example.com"])),
    ///     )
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// // The no-grants probe alone is clean: the kernel's floor refused it, typed.
    /// let finding = report.of(Check::Enforced).next().expect("the module's rule is probed");
    /// assert!(finding.detail.contains("got `endpoint`"), "{report}");
    /// ```
    pub fn enforced_under(mut self, probe: Fixture) -> Self {
        self.enforced_under.push(probe);
        self
    }

    /// Exclude one action (`verb`), or every action of an endpoint (`None`), from
    /// the checks that invoke it — for an action with real side effects. The reason
    /// is printed in the report. The description-only checks still run.
    ///
    /// Scoped by `Description::id`, which is a name for a KIND of endpoint: an id
    /// bound at more than one pattern is excluded at every one of them. When only one
    /// of those bindings is the hazardous one — one instance jailed to a scratch root
    /// and another to the working directory — use [`opt_out_at`](Self::opt_out_at),
    /// which names the binding.
    pub fn opt_out(
        mut self,
        id: impl Into<String>,
        verb: Option<Verb>,
        reason: impl Into<String>,
    ) -> Self {
        self.opt_outs.push(OptOut {
            id: id.into(),
            verb,
            reason: reason.into(),
        });
        self
    }

    /// Exclude one BINDING — one `pattern` exactly as the space reports it — from
    /// the checks that invoke it, for one `verb` or for every verb (`None`).
    ///
    /// The scope [`opt_out`](Self::opt_out) cannot express, and the reason it cannot
    /// is the same reason `Description::id` is the wrong key to memoize a firing on:
    /// **an id names a kind of endpoint, not a place one can be reached.** `ikigai-cli`
    /// binds `ikigai_fs::FileEndpoint` twice — `urn:file:{path}` jailed to a scratch
    /// root and safe to fire, `urn:orgfile:{path}` jailed to whatever `calendar.json`
    /// names, which with no config at all is the EMPTY path, i.e. the process's own
    /// working directory. Both describe as `file`, so `opt_out("file", …)` excludes
    /// the safe one too and the walk loses the coverage it was right to have.
    ///
    /// ```no_run
    /// # use ikigai_conformance::Suite;
    /// # use ikigai_core::Kernel;
    /// # fn my_kernel() -> Kernel { unimplemented!() }
    /// Suite::new()
    ///     .opt_out_at(
    ///         "urn:orgfile:{path}",
    ///         None,
    ///         "jailed to the configured org_dir, which is the CWD when there is no config",
    ///     )
    ///     .run_blocking(&my_kernel())
    ///     .assert_clean();
    /// ```
    ///
    /// The pattern is matched as an exact string against
    /// [`SpaceEntry::pattern`](ikigai_core::SpaceEntry) — the template, not an
    /// expanded IRI (`urn:file:{path}`, not `urn:file:x`). One that excluded nothing
    /// is reported by [`Check::Declarations`](crate::Check::Declarations), with the
    /// patterns the walk did reach.
    ///
    /// ⚠ One ordering wrinkle, from the interaction with firing identity: when two
    /// bindings would issue the IDENTICAL request and only one of them is excluded
    /// here, the first in walk order decides, because the collapse is recorded before
    /// the second binding's opt-out is consulted. Exclude both bindings, or exclude
    /// by id.
    pub fn opt_out_at(
        mut self,
        pattern: impl Into<String>,
        verb: Option<Verb>,
        reason: impl Into<String>,
    ) -> Self {
        self.opt_outs_at.push(OptOutAt {
            pattern: pattern.into(),
            verb,
            reason: reason.into(),
        });
        self
    }

    /// Exclude ONE check for one endpoint, with a reason — for a rule that is
    /// legitimately red on this endpoint while every other check still runs on it.
    ///
    /// The lever [`opt_out`](Self::opt_out) is not: that one drops every *invoking*
    /// check at once, so a module silencing one red rule loses `ENFORCED`,
    /// `CACHEABLE` and `SKOLEM-RDF` on the same endpoint. ikigai-browse paid exactly
    /// that: one endpoint's `text/turtle` face used four `ik:` terms the published
    /// vocabulary did not define — a fix another repo owns — and silencing
    /// `VOCABULARY` cost it every invoking check on its most security-relevant
    /// endpoint (capability-gated, model-calling, store-writing) for a release cycle.
    ///
    /// Applies to the description-only checks too, which nothing else can silence
    /// per id: `NAMES` on an endpoint whose id is a full IRI by contract, `ARGSPECS`
    /// on an input whose class is a lie the module documents.
    ///
    /// ```no_run
    /// # use ikigai_conformance::{Check, Suite};
    /// # use ikigai_core::Kernel;
    /// # fn my_kernel() -> Kernel { unimplemented!() }
    /// Suite::new()
    ///     .opt_out_check(
    ///         "browse-review",
    ///         Check::Vocabulary,
    ///         "ik:quote/ik:note land in ikigai-vocab 0.1.70; pinned by hand until then",
    ///     )
    ///     .run_blocking(&my_kernel())
    ///     .assert_clean();
    /// ```
    ///
    /// Prefer pinning the exception EXACTLY where you can — `rdf::parse` +
    /// [`rdf::terms`](crate::rdf::terms) + [`rdf::is_defined`](crate::rdf::is_defined)
    /// reproduce `VOCABULARY` in a hand test, so the waiver can assert the undefined
    /// set is *exactly* the known list and go red in both directions.
    ///
    /// **A waiver that suppressed nothing is a `note:`** (0.8.1, ledger #1493). The
    /// waived check still runs, with its findings set aside rather than reported, so
    /// the report can say when it would have found nothing: `note: the waiver of
    /// REQUIRED for sign suppressed nothing — …`, with the reason, and
    /// [`Report::idle_waivers`](crate::Report::idle_waivers) as data. A note, never a
    /// finding: a waiver can be green today on purpose (an upstream fix that comes
    /// and goes). What the set-aside run costs is what the check costs unwaived, on a
    /// READ: a description-only check reads the description, and an invoking one fires
    /// the `Source` or `Exists` it would have fired, after the walk proper is done with
    /// the action. It never fires a `Sink` or `Delete` the walk would not otherwise
    /// fire: a waiver it could judge only that way is left unjudged and gets no note,
    /// and so is a `SPACE-NAME` waiver. A waiver that could not have applied at all is
    /// still a [`Check::Declarations`](crate::Check::Declarations) finding, as before.
    pub fn opt_out_check(
        mut self,
        id: impl Into<String>,
        check: Check,
        reason: impl Into<String>,
    ) -> Self {
        self.opt_out_checks.push(OptOutCheck {
            id: id.into(),
            check,
            reason: reason.into(),
        });
        self
    }

    /// Register a namespace prefix as the module's own, so terms under it are not
    /// reported by [`Check::Vocabulary`] — any term, defined or not, now or in any
    /// later release.
    ///
    /// **What counts as the module's own is a claim nothing here can check.** The
    /// suite does not dereference the prefix or read a definition: "the module's own"
    /// means the module DOCUMENTS the namespace as its own — a graph it serves, a
    /// published vocabulary, or prose in its README (ikigai-sign's `sig:` is the prose
    /// case). A namespace it merely speaks belongs to someone else, and an undefined
    /// one is what the check exists to catch.
    ///
    /// **A module that ships the graph defining its terms wants
    /// [`vocabulary`](Self::vocabulary) instead**, which holds every term under the
    /// graph's namespace to the graph's subjects rather than waiving them.
    ///
    /// A registration that accounted for no term in any probed face is reported by
    /// [`Check::Declarations`]: it waives every term under that prefix forever —
    /// including the next one somebody invents — so one that waives nothing today
    /// is scope with no owner. The finding's endpoint is `(suite)`, a namespace
    /// having no id.
    pub fn namespace(mut self, prefix: impl Into<String>) -> Self {
        self.namespaces.push(prefix.into());
        self
    }

    /// Hold the module's own terms to the vocabulary it SHIPS: every term a probed
    /// face uses under the namespace of `turtle` must be one of that graph's subjects
    /// (ledger #344). Where [`namespace`](Self::namespace) waives a prefix, this
    /// checks it exactly — the rule [`Check::Vocabulary`] already applies to `ik:`
    /// against `ikigai-vocab`, pointed at the module's own file, so a typo or a term
    /// added to a face and not to the graph is a finding.
    ///
    /// The namespace is read off the graph's subjects ([`rdf::Vocabulary`] says how);
    /// the report prints it with the term count (`module vocabulary: … (12 terms)`). A
    /// term under it is judged by the graph alone, even when a registered
    /// [`namespace`](Self::namespace) also covers it — which leaves that registration
    /// accounting for nothing, and [`Check::Declarations`] says so.
    ///
    /// A graph that does not parse, or defines no term of its own, is a
    /// [`Check::Vocabulary`] finding at `(suite)`: the module ships it, so it is the
    /// module's defect. One under whose namespace no probed face used a term is a
    /// [`Check::Declarations`] finding, as an unused `namespace` is.
    ///
    /// ```
    /// use ikigai_conformance::{Check, Suite};
    /// use ikigai_core::{Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel};
    /// use ikigai_core::{ReprType, Representation, Verb};
    /// use std::sync::Arc;
    ///
    /// // What `ikigai_log::VOCABULARY_TTL` is to ikigai-log: the module's own graph.
    /// const VOCABULARY_TTL: &str = "@prefix log: <https://example.org/log#> .\n\
    ///     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    ///     log:Entry a rdfs:Class .\n\
    ///     log:level rdfs:domain log:Entry .";
    ///
    /// let entries = FnEndpoint::new("entries", |_inv: &Invocation<'_>| {
    ///     Ok(Representation::new(
    ///         ReprType::new("text/turtle"),
    ///         b"@prefix log: <https://example.org/log#> .\n\
    ///           <urn:log:1> a log:Entry ; log:levle \"warn\" ." as &[u8],
    ///     ))
    /// })
    /// .with_description(Description::new("entries").verb(Verb::Source).output("text/turtle"));
    /// let kernel = Kernel::new(Arc::new(
    ///     EndpointSpace::new().bind(Exact::new("urn:log:entries"), entries),
    /// ));
    ///
    /// let report = Suite::new().vocabulary(VOCABULARY_TTL).run_blocking(&kernel);
    /// let vocab: Vec<_> = report.of(Check::Vocabulary).collect();
    /// assert_eq!(vocab.len(), 1, "{report}");
    /// assert!(vocab[0].detail.contains("`https://example.org/log#levle`"));
    ///
    /// // `namespace` would have waived the typo with the rest of the prefix.
    /// let waived = Suite::new().namespace("https://example.org/log#").run_blocking(&kernel);
    /// assert_eq!(waived.of(Check::Vocabulary).count(), 0);
    /// ```
    pub fn vocabulary(mut self, turtle: impl AsRef<str>) -> Self {
        self.vocabularies
            .push(rdf::Vocabulary::parse(turtle.as_ref()));
        self
    }

    /// Declare an endpoint a pure function of its inputs, so a cacheable result
    /// with no golden thread but its own name is correct rather than a
    /// representation that caches forever with nothing to cut it. (The kernel hangs
    /// every cacheable read on its own name, so "no thread" is never observable.)
    ///
    /// Only an `Expiry::Never` result needs it: an `Expiry::At` deadline already
    /// bounds the answer, so a clock reading cacheable to the minute is not asked to
    /// be pure. A `pure` on such an endpoint is still counted as consulted, since
    /// the same endpoint may answer `Never` under another kernel (a pinned clock).
    ///
    /// **The claim is checked, not taken on trust** (since 0.8.3, ledger #1508): a
    /// cacheable result of an endpoint declared pure that carries a thread which is
    /// neither its own name nor the name or a thread of anything it read is a
    /// [`Check::Cacheable`] finding, `declared pure (`Suite::pure`) but carries a
    /// foreign thread …`. Such a thread names state the endpoint hangs from, so it is
    /// not a pure function of its inputs — and before this the declaration passed
    /// clean, because the foreign thread is exactly what silences the rule `pure`
    /// exempts from. A thread it cannot attribute is never reported (see the README).
    ///
    /// ```
    /// use std::sync::Arc;
    /// use ikigai_conformance::{Check, Suite};
    /// use ikigai_core::{
    ///     Description, EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, ReprType,
    ///     Representation, Verb,
    /// };
    ///
    /// // Declared pure, but its answer hangs from a store it never reads by name.
    /// let lookup = FnEndpoint::new("lookup", |_inv: &Invocation<'_>| {
    ///     Ok(Representation::new(ReprType::new("text/plain"), "row 7")
    ///         .cacheable()
    ///         .depends_on("urn:example:db"))
    /// })
    /// .with_description(Description::new("lookup").verb(Verb::Source).output("text/plain"));
    /// let kernel = Kernel::new(Arc::new(
    ///     EndpointSpace::new().bind(Exact::new("urn:example:lookup"), lookup),
    /// ));
    /// let report = Suite::new().pure("lookup").run_blocking(&kernel);
    /// let finding = report.of(Check::Cacheable).next().unwrap();
    /// assert!(finding
    ///     .detail
    ///     .starts_with("declared pure (`Suite::pure`) but carries a foreign thread `urn:example:db`:"));
    /// ```
    pub fn pure(mut self, id: impl Into<String>) -> Self {
        self.pure.push(id.into());
        self
    }

    /// Declare that an endpoint marks its results cacheable, and hold it to that:
    /// a result the kernel hands back uncacheable is then a finding.
    ///
    /// Needed because the kernel returns the **effective** expiry — the least
    /// cacheable of the result's own and every dependency's — so an endpoint that
    /// says `.cacheable()` over a volatile sub-resolution comes back looking as if
    /// it never said so, and the probe cannot tell the two apart. This is the
    /// declaration that makes the ~2000× incident (a cached read silently turning
    /// into a recomputation) a red test.
    pub fn cacheable(mut self, id: impl Into<String>) -> Self {
        self.cacheable.push(id.into());
        self
    }

    /// Declare that an endpoint's results are **live by decision** — `Expiry::Always`,
    /// never cached — and hold it to that: a result the kernel hands back cacheable
    /// is then a finding.
    ///
    /// The polarity twin of [`cacheable`](Self::cacheable), and the half
    /// [`Check::Cacheable`] is otherwise blind to. When the first resolution is
    /// `Expiry::Always` the probe reports only if the id was declared cacheable and
    /// is silent otherwise — so "declared cacheable, is not" is caught, and
    /// **"nobody declared anything, and it silently BECAME cacheable" is caught by
    /// nothing.** That is the direction the field guide's cacheability-propagation
    /// rule says nothing else can see: a resource that must be read fresh (a secret,
    /// a live platform read, a non-deterministic generation, process-global host
    /// state no `depends_on` could name) starts being served from the cache until
    /// something cuts a thread — and no type changes, no test fails, nothing says so.
    ///
    /// It is also the spelling for "uncacheable on purpose" in the printed record: a
    /// clean report over a module with no cacheable result is otherwise
    /// indistinguishable from one where caching was never considered.
    ///
    /// `live` and [`cacheable`](Self::cacheable) contradict each other; declaring
    /// both for one id is itself a finding.
    ///
    /// It applies only where `CACHEABLE` runs, which is only on a cacheable verb: a
    /// `live` on a `Sink` or `Delete` holds the endpoint to nothing, and
    /// [`Check::Declarations`] reports it rather than letting the report print
    /// `declared live:` for a check that returned early.
    pub fn live(mut self, id: impl Into<String>) -> Self {
        self.live.push(id.into());
        self
    }

    /// Declare `constructor` the module's **configuration-free** space, which names
    /// itself [`space_iri`]`(module)`, and hold it to that ([`Check::SpaceName`]).
    ///
    /// `module` is the crate name without `ikigai-` (`"text"` claims
    /// `urn:iki:space:text`), or a module and a part for a space with doors of its
    /// own (`"sexpr:arrangement"`). The check calls `constructor` twice and asserts:
    ///
    /// - `id()` is `space_iri(module)`, which lies under [`SPACE_PREFIX`];
    /// - both calls claim the same name and hold the same doors: equal `topology()`
    ///   (the whole tree: every door's pattern, match kind and endpoint name, every
    ///   confined corridor, every enclosed space) and equal `entries()`;
    /// - the topology's root node carries the name, which is where
    ///   `urn:kernel:topology` and the space diagram read it;
    /// - any other self-named declaration under the same name holds the same doors.
    ///
    /// A name is a claim: same name, same doors. The cache partitions on it, and a
    /// corridor built from it shares cache entries with every other instance, so a
    /// constructor that reads configuration, the environment or files while building,
    /// or allocates fresh state per call, is not self-named: declare it with
    /// [`host_named_space`](Self::host_named_space).
    ///
    /// ```
    /// use ikigai_conformance::Suite;
    /// use ikigai_core::{builtins, space_iri, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// fn space() -> EndpointSpace {
    ///     EndpointSpace::new()
    ///         .bind(Exact::new("urn:example:echo"), builtins::echo())
    ///         .named(space_iri("example"))
    /// }
    ///
    /// let report = Suite::new()
    ///     .checks(ikigai_conformance::Checks::SPACE_NAME)
    ///     .self_named_space("example", space)
    ///     .run_blocking(&Kernel::new(Arc::new(space())));
    /// report.assert_clean();
    /// assert!(report.to_string().contains("space: urn:iki:space:example self-named"));
    /// ```
    ///
    /// # Panics
    ///
    /// When `module` is not a [`space_iri`] segment list (`"ikigai-text"` still
    /// carries the crate prefix): the literal is wrong where it is written.
    pub fn self_named_space<F, S>(mut self, module: &str, constructor: F) -> Self
    where
        F: Fn() -> S + Send + Sync + 'static,
        S: Space + 'static,
    {
        let iri = space_iri(module);
        self.spaces.push(SpaceDecl {
            label: iri.as_str().to_string(),
            constructor: std::any::type_name::<F>().to_string(),
            kind: SpaceDeclKind::SelfNamed {
                module: module.to_string(),
                iri,
                make: Arc::new(move || Arc::new(constructor()) as Arc<dyn Space>),
            },
        });
        self
    }

    /// Declare `space` built by an **instance-built or parameterized** constructor
    /// (`space(root)`, `space_with_budget(..)`, a config), which the HOST names, and
    /// hold it to claiming nothing itself ([`Check::SpaceName`]): its `id()` is
    /// `None` and its topology root is anonymous.
    ///
    /// Only the host knows which instance it passed in, so a module that names such
    /// a space claims one name for doors that depend on what it was handed. `label`
    /// is what findings and waivers name it by; the constructor's call is a good one
    /// (`"space(root)"`). The space is passed by value because one is enough: `None`
    /// is `None` on every call. Pass an `Arc` clone of the space the test's kernel
    /// is built from, if you like (`Arc<S>` is a `Space`).
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// fn space(prefix: &str) -> EndpointSpace {
    ///     EndpointSpace::new().bind(Exact::new(format!("{prefix}echo")), builtins::echo())
    /// }
    ///
    /// let built = Arc::new(space("urn:example:"));
    /// let report = Suite::new()
    ///     .checks(Checks::SPACE_NAME)
    ///     .host_named_space("space(prefix)", built.clone())
    ///     .run_blocking(&Kernel::new(built));
    /// report.assert_clean();
    /// assert_eq!(report.of(Check::SpaceName).count(), 0);
    /// ```
    pub fn host_named_space(
        mut self,
        label: impl Into<String>,
        space: impl Space + 'static,
    ) -> Self {
        self.spaces.push(SpaceDecl {
            label: label.into(),
            constructor: String::new(),
            kind: SpaceDeclKind::HostNamed {
                space: Arc::new(space),
            },
        });
        self
    }

    /// Walk operations under every prefix the kernel seals for core too. They are core's
    /// endpoints, not the module's — off by default so a module's report is about
    /// the module.
    ///
    /// Off, a MOUNTED peer's operations are skipped too, whatever an alias renamed
    /// them to (ledger #335): a mounted binding whose description id is one this
    /// kernel's own operations use (`kernel-catalog`), whose pattern is one of theirs
    /// re-prefixed (`urn:edge:kernel:catalog`), or whose id is `kernel-…` under a
    /// `:kernel:` segment. On, those are walked with the rest.
    pub fn include_kernel_ops(mut self) -> Self {
        self.kernel_ops = true;
        self
    }

    /// Scope the walk to the module's OWN endpoints: a description id, or an id
    /// prefix ending in `*` (`"ledger-*"`). Call it once per id or prefix. Once any is
    /// declared, an endpoint whose id matches none is INHERITED: bound beside the
    /// module (a dependency the host composes, a fixture endpoint, a mounted peer),
    /// listed on the report's `inherited:` line and in
    /// [`Declarations::inherited`](crate::Declarations::inherited),
    /// and NOT walked: no check reads it and no probe fires it.
    ///
    /// Three composing hosts wrote the same 40-60 line partition by hand to get a
    /// report about the module rather than about everything the host composes
    /// (ledger #328), and the hand partition bought one thing a prefix alone would
    /// lose: it went red when a module quietly gained an endpoint, instead of letting
    /// the endpoint join the inherited pile. Keep that by pinning the inherited list
    /// exactly, the way [`Report::walked`](crate::Report::walked) is pinned:
    ///
    /// ```
    /// use ikigai_conformance::{Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// // The module's endpoint (`echo`) beside a dependency the host composes.
    /// let space = EndpointSpace::new()
    ///     .bind(Exact::new("urn:example:echo"), builtins::echo())
    ///     .bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
    /// let report = Suite::new()
    ///     .checks(Checks::NAMES)
    ///     .own("echo")
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// report.assert_clean(); // `to-upper` is not this module's, so it is not walked
    /// assert_eq!(&*report.walked, ["echo"]);
    /// assert_eq!(report.declared.inherited, ["to-upper"]);
    /// ```
    ///
    /// Not walking is also the safer default for a composing host: an inherited
    /// endpoint that spawns a process or calls a model is not fired at all.
    ///
    /// A pattern that matched no endpoint is a [`Check::Declarations`] finding that
    /// no check selection and no waiver removes: a scope that matches nothing turns
    /// the suite into a no-op that passes.
    ///
    /// Migrating a host from opt-outs to `own` means DELETING the opt-outs it
    /// replaces: an opt-out left on an inherited id excluded nothing, which is a
    /// [`Check::Declarations`] finding. Its reason moves to [`inherit`](Self::inherit).
    pub fn own(mut self, id: impl Into<String>) -> Self {
        self.own.push(id.into());
        self
    }

    /// [`own`](Self::own) for each of `ids`, in order: for a host with many exact
    /// ids, instead of a fold. An empty list declares nothing, and the walk stays
    /// unscoped.
    ///
    /// ```
    /// use ikigai_conformance::{Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// const OURS: [&str; 2] = ["echo", "reverse-list"];
    /// let space = EndpointSpace::new()
    ///     .bind(Exact::new("urn:example:echo"), builtins::echo())
    ///     .bind(Exact::new("urn:example:toUpper"), builtins::to_upper())
    ///     .bind(Exact::new("urn:example:reverse"), builtins::reverse_list());
    /// let report = Suite::new()
    ///     .checks(Checks::NAMES)
    ///     .own_all(OURS)
    ///     .run_blocking(&Kernel::new(Arc::new(space)));
    /// assert_eq!(report.declared.own, OURS);
    /// assert_eq!(&*report.walked, ["echo", "reverse-list"]);
    /// assert_eq!(report.declared.inherited, ["to-upper"]);
    /// ```
    pub fn own_all<I>(mut self, ids: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.own
            .extend(ids.into_iter().map(|id| id.as_ref().to_string()));
        self
    }

    /// Declare one id INHERITED, with the reason it is not the module's own: the
    /// record an opt-out's reason carried before [`own`](Self::own) replaced it. The
    /// reason is printed beside the report (`inherited because: <id>: <reason>`) and
    /// listed by [`Report::inherited_because`](crate::Report::inherited_because).
    ///
    /// Once any is declared, the inherited list is PINNED: it must be exactly the
    /// declared ids, compared as a set (catalog order does not matter). An inherited
    /// endpoint with no `inherit` is a [`Check::Declarations`] finding, so a module
    /// that gains an endpoint outside its scope turns the suite red instead of
    /// joining the pile; so is an `inherit` for an id that was not inherited (an
    /// `own` pattern matches it, no `own` is declared at all, or nothing binds it).
    /// Neither is removed by a check selection or a waiver: the pin is the assertion
    /// a host would otherwise write by hand.
    ///
    /// ```
    /// use ikigai_conformance::{Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let host = |grown: bool| {
    ///     let mut space = EndpointSpace::new()
    ///         .bind(Exact::new("urn:example:echo"), builtins::echo())
    ///         .bind(Exact::new("urn:example:toUpper"), builtins::to_upper());
    ///     if grown {
    ///         space = space.bind(Exact::new("urn:example:reverse"), builtins::reverse_list());
    ///     }
    ///     Kernel::new(Arc::new(space))
    /// };
    /// let suite = Suite::new()
    ///     .checks(Checks::NAMES)
    ///     .own("echo")
    ///     .inherit("to-upper", "ikigai-core's builtin; its own tests cover it");
    ///
    /// let report = suite.run_blocking(&host(false));
    /// report.assert_clean();
    /// assert!(report
    ///     .to_string()
    ///     .contains("inherited because: to-upper: ikigai-core's builtin; its own tests cover it"));
    ///
    /// // A third endpoint joins the pile: red, not silent.
    /// let report = suite.run_blocking(&host(true));
    /// let finding = report.against("reverse-list").next().unwrap();
    /// assert!(finding.detail.contains("no `Suite::inherit` gives the reason"), "{report}");
    /// ```
    pub fn inherit(mut self, id: impl Into<String>, reason: impl Into<String>) -> Self {
        self.inherit.push(Inherited {
            endpoint: id.into(),
            reason: reason.into(),
        });
        self
    }

    /// Hold the inherited list to the [`inherit`](Self::inherit) declarations,
    /// exactly, when there are any. Like an `own` pattern that matched nothing, these
    /// findings are not subject to selection or waiver: the pin is an assertion the
    /// host asked for.
    fn inherit_pin(&self, report: &mut Report) {
        if self.inherit.is_empty() {
            return;
        }
        let mut findings = Vec::new();
        for declared in &self.inherit {
            let id = declared.endpoint.as_str();
            if report.declared.inherited.iter().any(|i| i == id) {
                continue;
            }
            let why = if self.own.is_empty() {
                "no `Suite::own` is declared, so nothing is inherited: every endpoint the \
                 kernel binds is walked as the module's own"
                    .to_string()
            } else if let Some(pattern) = self.owner(id) {
                format!(
                    "`Suite::own(\"{}\")` matches it, so it is the module's own and is walked",
                    crate::report::visible(pattern)
                )
            } else {
                "the kernel binds no endpoint with that id (the id is the `Description::id`, \
                 not the bound IRI)"
                    .to_string()
            };
            findings.push(Finding::new(
                id,
                None,
                Check::Declarations,
                format!(
                    "`Suite::inherit` gives a reason ({}) for an id that is not inherited: {why}. \
                     This finding cannot be waived: a reason is only true of the id it names",
                    declared.reason
                ),
            ));
        }
        for id in &report.declared.inherited {
            if self.inherit.iter().any(|d| &d.endpoint == id) {
                continue;
            }
            findings.push(Finding::new(
                id,
                None,
                Check::Declarations,
                format!(
                    "inherited, and no `Suite::inherit` gives the reason: once one is declared \
                     the inherited list is pinned exactly, so an endpoint that joins it is a \
                     finding rather than a silent addition. Declare \
                     `Suite::inherit(\"{}\", \"<why it is not the module's own>\")`, or own it \
                     with `Suite::own`. This finding cannot be waived",
                    crate::report::visible(id)
                ),
            ));
        }
        report.findings.extend(findings);
    }

    /// The `own` pattern matching `id`, or `None`. Every id is owned when nothing
    /// is declared.
    fn owner(&self, id: &str) -> Option<&str> {
        self.own
            .iter()
            .find(|p| match p.strip_suffix('*') {
                Some(prefix) => id.starts_with(prefix),
                None => p.as_str() == id,
            })
            .map(String::as_str)
    }

    /// Name this RUN, for a host that walks one suite over two or more kernels
    /// (in-process and mounted, inline and served shapes): the report's first line is
    /// `run: <label>`, and [`Report::label`](crate::Report::label) returns it (ledger
    /// #354). Six adopters printed a header line of their own per walk, because the
    /// declarations and fixtures a report echoes are the same in both and the two
    /// reports could be told apart only by reading them.
    ///
    /// ```
    /// use ikigai_conformance::{Checks, Suite};
    /// use ikigai_core::{builtins, EndpointSpace, Exact, Kernel};
    /// use std::sync::Arc;
    ///
    /// let space = || EndpointSpace::new().bind(Exact::new("urn:example:echo"), builtins::echo());
    /// let suite = Suite::new().checks(Checks::NAMES);
    /// let local = suite.clone().label("in-process").run_blocking(&Kernel::new(Arc::new(space())));
    /// assert_eq!(local.label(), Some("in-process"));
    /// assert!(local.to_string().starts_with("run: in-process\n"), "{local}");
    /// ```
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Hold `Sink` and `Delete` actions to [`Check::Required`] too.
    ///
    /// Off by default because every required input dropped is one more FIRING of a
    /// mutating action, and a drop the endpoint wrongly accepts has mutated: the
    /// defect the check exists to find is, on these verbs, a write. Turn it on over a
    /// kernel built as a test fixture, after the walk's other firings are accounted
    /// for. A `Sink` whose baseline call the pipeline probe already fired (it declares
    /// `content`) is measured against that firing; one that does not is fired once
    /// more, with its minimal inputs, as the baseline.
    ///
    /// A declaration that reached nothing (REQUIRED unselected, or no mutating action
    /// declares a required by-value input) is reported by
    /// [`Check::Declarations`].
    pub fn required_on_mutating(mut self) -> Self {
        self.required_on_mutating = true;
        self
    }

    /// Run on the futures executor — the one-line form for a `#[test]`. Prefer
    /// [`run`](Self::run) inside an async host.
    pub fn run_blocking(&self, kernel: &Kernel) -> Report {
        futures::executor::block_on(self.run(kernel))
    }

    /// Walk every endpoint and run every selected check, returning all findings.
    pub async fn run(&self, kernel: &Kernel) -> Report {
        let mut report = Report {
            findings: Vec::new(),
            endpoints: 0,
            actions: 0,
            checks: self.checks,
            declared: Box::new(Declarations {
                opted_out: self
                    .opt_outs
                    .iter()
                    .map(|o| OptedOut {
                        endpoint: o.id.clone(),
                        verb: o.verb,
                        reason: o.reason.clone(),
                    })
                    .collect(),
                opted_out_at: self
                    .opt_outs_at
                    .iter()
                    .map(|o| OptedOutAt {
                        pattern: o.pattern.clone(),
                        verb: o.verb,
                        reason: o.reason.clone(),
                    })
                    .collect(),
                opted_out_checks: self
                    .opt_out_checks
                    .iter()
                    .map(|o| OptedOutCheck {
                        endpoint: o.id.clone(),
                        check: o.check,
                        reason: o.reason.clone(),
                    })
                    .collect(),
                required_on_mutating: self.required_on_mutating,
                own: self.own.clone(),
                inherited: Vec::new(),
                pure: self.pure.clone(),
                cacheable: self.cacheable.clone(),
                live: self.live.clone(),
                namespaces: self.namespaces.clone(),
                vocabularies: self
                    .vocabularies
                    .iter()
                    .flatten()
                    .map(|v| DeclaredVocabulary {
                        namespaces: v.namespaces().iter().cloned().collect(),
                        terms: v.terms().len(),
                    })
                    .collect(),
                fixtures: self.fixtures.clone(),
                spaces: self
                    .spaces
                    .iter()
                    .map(|d| DeclaredSpace {
                        label: d.label.clone(),
                        naming: d.naming(),
                        constructor: d.constructor.clone(),
                        doors: None,
                        opaque: 0,
                    })
                    .collect(),
                evidence: Evidence {
                    inherited_because: self.inherit.clone(),
                    label: self.label.clone(),
                    check_failures: self.check_failures,
                    ..Evidence::default()
                },
            }),
            unprobed: Box::default(),
            probed: Vec::new(),
            walked: Box::default(),
            collapsed: Box::default(),
        };

        let Some(entries) = kernel.entries() else {
            report.findings.push(Finding::new(
                "(kernel)",
                None,
                Check::ArgSpecs,
                "the root space is not enumerable (`Kernel::entries()` is `None`): \
                 nothing can be walked — bind the module in an `EndpointSpace`",
            ));
            // The spaces are the module's constructors, not the kernel: checkable
            // whatever the kernel lists.
            self.space_checks(&mut report);
            if self.checks.contains(Check::Required) {
                report.declared.evidence.required_nothing =
                    Some("the root space is not enumerable, so nothing was walked".to_string());
            }
            return report;
        };

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut walk_order: Vec<String> = Vec::new();
        let mut coverage = Coverage::default();
        // Every firing the walk has already considered, keyed by the request it
        // issues, with the index of its record so a repeat can name both bindings.
        let mut firings: BTreeMap<Firing, usize> = BTreeMap::new();
        let mut fired: Vec<Fired> = Vec::new();
        // What every waiver would have suppressed, for the `note:` lines.
        let mut tally = Tally::default();
        // What `Suite::own` scoped out, in walk order, and which patterns matched.
        let mut inherited: Vec<String> = Vec::new();
        let mut own_matched: BTreeSet<&str> = BTreeSet::new();
        // Every id the kernel listed, owned or not: what an empty match prints.
        let mut listed: Vec<String> = Vec::new();
        // The kernel's own operations by what they are called HERE, so a mount that
        // renamed a peer's can still be told apart (ledger #335).
        let seals = kernel.sealed();
        let ops = KernelOps::of(kernel, &entries, &seals);
        // Mounted bindings that describe with no action: a peer that did not answer
        // with a contract, reported once per mount after the walk (ledger #334).
        let mut contractless: Vec<Contractless> = Vec::new();
        for entry in entries {
            if core_owned(&entry.pattern, &seals) && !self.kernel_ops {
                continue;
            }
            let description = kernel.describe_pattern(&entry.pattern);
            if !self.kernel_ops && ops.renamed(&entry, description.as_ref()) {
                continue;
            }
            let id = description
                .as_ref()
                .map_or(entry.endpoint.as_str(), |d| d.id.as_str());
            if !listed.iter().any(|l| l == id) {
                listed.push(id.to_string());
            }
            if !self.own.is_empty() {
                match self.owner(id) {
                    Some(pattern) => {
                        own_matched.insert(pattern);
                    }
                    None => {
                        if !inherited.iter().any(|i| i == id) {
                            inherited.push(id.to_string());
                        }
                        coverage.inherited.insert(id.to_string());
                        continue;
                    }
                }
            }
            let Some(description) = description else {
                if self.checks.contains(Check::ArgSpecs) {
                    let from = report.findings.len();
                    report.findings.push(Finding::new(
                        &entry.endpoint,
                        None,
                        Check::ArgSpecs,
                        format!(
                            "bound at `{}` but describes nothing: a Meta resolution of the \
                             pattern reaches no description",
                            entry.pattern
                        ),
                    ));
                    self.set_aside_waived(&mut report, from, &mut tally);
                }
                continue;
            };
            if let Some(origin) = &entry.origin {
                if description.action_specs().is_empty() {
                    contractless.push(Contractless {
                        origin: origin.clone(),
                        pattern: entry.pattern.clone(),
                        id: description.id.clone(),
                        summary: description.summary.clone(),
                    });
                    continue;
                }
            }
            let first_time = seen.insert(description.id.clone());
            if first_time {
                report.endpoints += 1;
                walk_order.push(description.id.clone());
                let from = report.findings.len();
                self.static_checks(&description, &mut report);
                self.set_aside_waived(&mut report, from, &mut tally);
            }
            // What the walk covered, recorded from the DESCRIPTION rather than from
            // what was reached: a declaration is inert against the shape the module
            // declared, so a template that fails to expand must not also make every
            // fixture for that id read as inert.
            coverage.walked.insert(description.id.clone());
            if !coverage.patterns.contains(&entry.pattern) {
                coverage.patterns.push(entry.pattern.clone());
            }
            if let Some(vars) = template_vars(&entry.pattern) {
                coverage
                    .vars
                    .entry(description.id.clone())
                    .or_default()
                    .extend(vars);
            }
            for spec in description.action_specs() {
                coverage.record_action(&description.id, spec.verb, false);
                coverage.record_inputs(&description.id, &spec);
                if required_probes_something(&spec) {
                    if spec.verb.is_mutating() {
                        coverage.required_mutating_candidates = true;
                    }
                    if !spec.verb.is_mutating() || self.required_on_mutating {
                        coverage.required_eligible.insert(description.id.clone());
                    }
                }
                if spec.outputs.iter().any(|o| rdf::is_rdf_face(o)) {
                    coverage.rdf_faces.insert(description.id.clone());
                }
            }
            // Per entry, not per id: two patterns binding one endpoint are two
            // places it can be reached, each with its own template variables.
            let from = report.findings.len();
            self.template_checks(&entry, &description, &mut report);
            self.set_aside_waived(&mut report, from, &mut tally);

            let target = match self.target_for(&entry, &description) {
                Ok(iri) => iri,
                Err(detail) => {
                    if self.checks.contains(Check::ArgSpecs) {
                        let from = report.findings.len();
                        report.findings.push(Finding::new(
                            &description.id,
                            None,
                            Check::ArgSpecs,
                            detail,
                        ));
                        self.set_aside_waived(&mut report, from, &mut tally);
                    }
                    continue;
                }
            };
            // Whether a write can land on this target through the description's own
            // verbs: the kernel's auto-cut fires on the thread named for it, which is
            // the thread it hangs every cacheable read of the same name from.
            let written_here = description
                .action_specs()
                .iter()
                .any(|s| s.verb.is_mutating());
            for spec in description.action_specs() {
                // The identity of what would be FIRED, computed before anything is:
                // a second binding issuing the identical request adds nothing but a
                // second mutation, landing on the state the first one left. The
                // template checks above stay per entry — two patterns really are two
                // places, each with its own variables — and only the invoking checks
                // are memoized here.
                let key = Firing {
                    target: target.as_str().to_string(),
                    verb: crate::report::verb_name(spec.verb),
                    args: self.args_for(&description.id, &spec),
                };
                // Read BEFORE the collapse, so a waiver naming a binding the walk
                // reached is never reported inert merely because another binding got
                // there first. NOT recorded as an opted-out ACTION: this lever
                // excludes one binding, and the same action may still be invoked at
                // another — `Coverage::invoked` is what says whether anything ran.
                let excluded_at = self.opted_out_at(&entry.pattern, spec.verb);
                if let Some(out) = excluded_at {
                    coverage.patterns_excluded.insert(out.pattern.clone());
                    coverage.ids_excluded_at.insert(description.id.clone());
                }
                if let Some(index) = firings.get(&key) {
                    fired[*index].patterns.push(entry.pattern.clone());
                    continue;
                }
                firings.insert(key, fired.len());
                fired.push(Fired {
                    endpoint: description.id.clone(),
                    verb: spec.verb,
                    target: target.as_str().to_string(),
                    patterns: vec![entry.pattern.clone()],
                });
                report.actions += 1;
                if self.opted_out(&description.id, spec.verb) {
                    coverage.record_action(&description.id, spec.verb, true);
                    continue;
                }
                if excluded_at.is_some() {
                    continue;
                }
                // A required input that accepts no value (`"one_of": []`, ledger #41): no
                // call conforms, the kernel's manifold does not offer the action, and
                // there is nothing to invoke. Said once, under the first selected check
                // that would have resolved it.
                if let Some(input) = uncallable_by(&spec) {
                    if let Some(check) = Check::ALL
                        .into_iter()
                        .find(|c| Checks::INVOKING.contains(*c) && self.runs(&description.id, *c))
                    {
                        report.unprobed.push(Unprobed {
                            endpoint: description.id.clone(),
                            verb: spec.verb,
                            check,
                            reason: format!(
                                "uncallable, so no invoking check ran: the required input `{}` \
                                 declares an empty enumeration (`\"one_of\": []`) and accepts no \
                                 value, so no call conforms and `urn:kernel:actions` does not \
                                 offer the action",
                                input.name
                            ),
                        });
                    }
                    continue;
                }
                coverage.record_invoked(&description.id, spec.verb);
                let mut under: Vec<UnderProbe<'_>> = Vec::new();
                for (index, probe) in self.enforced_under.iter().enumerate() {
                    if probe.id != description.id || probe.verb != spec.verb {
                        continue;
                    }
                    if probe.capability.is_none() {
                        continue;
                    }
                    match self.target_with(&entry, &description, Some(&probe.bindings)) {
                        Ok(target) => under.push(UnderProbe {
                            index,
                            probe,
                            target,
                        }),
                        Err(why) => {
                            coverage.under_untargeted.entry(index).or_insert(why);
                        }
                    }
                }
                let mut fails: Vec<FailsProbe<'_>> = Vec::new();
                for (index, (probe, class)) in self.fails_under.iter().enumerate() {
                    if probe.id != description.id || probe.verb != spec.verb {
                        continue;
                    }
                    match self.target_with(&entry, &description, Some(&probe.bindings)) {
                        Ok(target) => fails.push(FailsProbe {
                            index,
                            probe,
                            class: *class,
                            target,
                        }),
                        Err(why) => {
                            coverage.fails_untargeted.entry(index).or_insert(why);
                        }
                    }
                }
                let mut action = Action {
                    suite: self,
                    kernel,
                    id: &description.id,
                    target: &target,
                    spec: &spec,
                    written_here,
                    minimal: None,
                    fired: None,
                    no_grants: None,
                    repeated: None,
                    under,
                    under_ran: Vec::new(),
                    under_floor: Vec::new(),
                    failure_reported: false,
                    caller: self
                        .fixture_for(&description.id, spec.verb)
                        .and_then(Fixture::caller)
                        .cloned()
                        .unwrap_or_else(Capability::root),
                    caller_used: false,
                    shadow: None,
                    shadow_blocked: false,
                    shadow_unresolved: false,
                    fails,
                    fails_ran: Vec::new(),
                    observed: Vec::new(),
                };
                action.enforced(&mut report).await;
                action.authority(&mut report).await;
                action.rdf_faces(&mut report, &mut coverage).await;
                action.cacheable(&mut report, &mut coverage).await;
                action.pipeline(&mut report).await;
                // After PIPELINE, so a mutating action's baseline is the one firing
                // the pipeline probe already made.
                action.required(&mut report).await;
                // Last of the answer-judging checks: it reads what the checks above resolved and fires
                // nothing new for a mutating verb, so the walk's footprint stays
                // "Source once or twice, Sink once, Delete never".
                action.outputs(&mut report).await;
                // After CACHEABLE, because it cuts and CACHEABLE reads the cache as its
                // own two resolutions left it; and after every check that reads the
                // minimal resolution, so a failed one is still reported as needed by
                // the check that always needed it.
                action.recompute(&mut report).await;
                action.bodies(&mut report).await;
                action.body_inputs(&mut report).await;
                // After every check that resolves the action, so it judges every failure
                // they observed; it fires only its own `fails_under` probes, and one
                // minimal resolution of a read nothing above resolved.
                action.failures(&mut report).await;
                // Positive evidence for a face the RDF checks do not cover: what
                // this action actually served, whatever its media type. Fires
                // nothing of its own — it reads what the checks above resolved.
                action.record_probe(&mut report);
                if action.caller_used {
                    coverage.record_caller_used(&description.id, spec.verb);
                }
                coverage.under_ran.extend(action.under_ran.iter().copied());
                coverage.fails_ran.extend(action.fails_ran.iter().copied());
                for (index, lacking) in std::mem::take(&mut action.under_floor) {
                    coverage.under_floor.entry(index).or_insert(lacking);
                }
                // Last of all, once the walk proper is done with the action: every
                // check WAIVED for it runs again with its findings set aside, so the
                // report can say which waiver suppressed nothing (ledger #1493).
                for check in self.waived_invoking(&description.id) {
                    let seen = action.set_aside(check).await;
                    tally.record(&description.id, check, seen);
                }
            }
        }
        report.walked = walk_order.into_boxed_slice();
        report.declared.inherited = inherited;
        contractless_findings(&contractless, &mut report);
        // A scope that matches nothing makes the suite a no-op that passes, so this
        // finding is not subject to selection or waiver (ledger #328).
        for pattern in &self.own {
            if own_matched.contains(pattern.as_str()) {
                continue;
            }
            report.findings.push(Finding::new(
                SUITE,
                None,
                Check::Declarations,
                format!(
                    "`Suite::own(\"{pattern}\")` matched no endpoint the kernel binds, so it \
                     scoped the walk to nothing (an id is the `Description::id`, and a prefix \
                     ends in `*`). The kernel binds: {}. This finding cannot be waived: a \
                     scope that matches nothing turns the suite into a no-op that passes",
                    if listed.is_empty() {
                        "nothing".to_string()
                    } else {
                        listed
                            .iter()
                            .map(|l| format!("`{}`", crate::report::visible(l)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ),
            ));
        }
        self.inherit_pin(&mut report);
        report.collapsed = fired
            .into_iter()
            .filter(|f| f.patterns.len() > 1)
            .map(|f| Collapsed {
                endpoint: f.endpoint,
                verb: f.verb,
                target: f.target,
                patterns: f.patterns,
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        self.space_checks(&mut report);
        let from = report.findings.len();
        self.declaration_checks(&coverage, &mut report);
        self.set_aside_waived(&mut report, from, &mut tally);
        report.declared.evidence.required_nothing = self.required_nothing(&coverage, &report);
        report.declared.evidence.idle_waivers = self.idle_waivers(&coverage, &tally);
        report
    }

    // ----- SPACE-NAME ---------------------------------------------------------

    /// Every declared space against the kind the module said it is. Builds spaces
    /// and reads them; resolves nothing.
    fn space_checks(&self, report: &mut Report) {
        // (label, constructor, rootless topology) of every self-named space checked,
        // for the cross-declaration rule: one name, one set of doors.
        let mut claimed: Vec<(String, String, Topology)> = Vec::new();
        for (index, decl) in self.spaces.iter().enumerate() {
            if !self.runs(&decl.label, Check::SpaceName) {
                continue;
            }
            let mut findings = Vec::new();
            match &decl.kind {
                SpaceDeclKind::SelfNamed { module, iri, make } => {
                    let (first, second) = (make(), make());
                    let ctor = &decl.constructor;
                    let id = first.id();
                    match &id {
                        None => findings.push(format!(
                            "`{ctor}` claims no name, but it is declared self-named: a \
                             configuration-free space names itself — end the constructor with \
                             `.named(ikigai_core::space_iri(\"{module}\"))`, which is `{iri}`",
                            iri = iri.as_str()
                        )),
                        Some(id) if !id.as_str().starts_with(SPACE_PREFIX) => {
                            findings.push(format!(
                                "`{ctor}` claims `{id}`, outside `{SPACE_PREFIX}`: a module's \
                                 configuration-free space is named \
                                 `ikigai_core::space_iri(\"{module}\")`, which is `{iri}`",
                                id = id.as_str(),
                                iri = iri.as_str()
                            ))
                        }
                        Some(id) if id != iri => findings.push(format!(
                            "`{ctor}` claims `{id}`, but it is declared as `{iri}`: declare the \
                             name it claims, or name it `ikigai_core::space_iri(\"{module}\")`. A \
                             part of a module's space with doors of its own is \
                             `space_iri(\"<module>:<part>\")`",
                            id = id.as_str(),
                            iri = iri.as_str()
                        )),
                        Some(_) => {}
                    }
                    let again = second.id();
                    if again != id {
                        findings.push(format!(
                            "two calls of `{ctor}` claim different names ({} then {}): a name is \
                             a claim — same name, same doors — and one that changes per call \
                             claims nothing a cache can share",
                            show_id(id.as_ref()),
                            show_id(again.as_ref())
                        ));
                    }
                    let top = first.topology();
                    if top.id != id {
                        findings.push(format!(
                            "the topology's root node names {} while `id()` claims {}: \
                             `urn:kernel:topology` and the space diagram read the node, so a \
                             space that overrides `id()` names its node too \
                             (`Topology::with_id(self.id())`)",
                            show_id(top.id.as_ref()),
                            show_id(id.as_ref())
                        ));
                    }
                    let (a, b) = (top.clone().with_id(None), second.topology().with_id(None));
                    let difference = if a != b {
                        Some(first_difference(&lines_of(&a), &lines_of(&b)))
                    } else if first.entries() != second.entries() {
                        Some(first_difference(
                            &entry_lines(first.entries()),
                            &entry_lines(second.entries()),
                        ))
                    } else {
                        None
                    };
                    if let Some(difference) = difference {
                        findings.push(format!(
                            "two calls of `{ctor}` hold different doors ({difference}): a name \
                             is a claim — same name, same doors — and every call answers to \
                             `{iri}`. A constructor whose doors vary per call (it reads \
                             configuration, the environment or files while building) is \
                             host-named: declare it with `Suite::host_named_space` and drop the \
                             name",
                            iri = iri.as_str()
                        ));
                    }
                    let evidence = &mut report.declared.spaces[index];
                    evidence.doors = Some(count_doors(&a));
                    evidence.opaque = count_opaque(&a);
                    claimed.push((decl.label.clone(), ctor.clone(), a));
                }
                SpaceDeclKind::HostNamed { space } => {
                    let top = space.topology();
                    if let Some(id) = space.id() {
                        findings.push(format!(
                            "claims `{}`, but it is declared host-named: its doors depend on what \
                             it was handed, so only the host knows which instance it is. Drop \
                             `.named(..)` and let the host name it — a name is a claim: same \
                             name, same doors",
                            id.as_str()
                        ));
                    } else if let Some(id) = top.id {
                        findings.push(format!(
                            "`id()` claims nothing, but the topology's root node names `{}`: a \
                             host-named space is anonymous in both places, or the diagram shows \
                             a name the cache never partitions on",
                            id.as_str()
                        ));
                    }
                }
            }
            for detail in findings {
                report
                    .findings
                    .push(Finding::new(&decl.label, None, Check::SpaceName, detail));
            }
        }
        // One name, one set of doors, across declarations too: each passes alone
        // when a module declares a PART of its space under the whole's name.
        for (i, (label, ctor, top)) in claimed.iter().enumerate() {
            let clash = claimed[..i]
                .iter()
                .find(|(other, _, other_top)| other == label && other_top != top);
            if let Some((_, first_ctor, first_top)) = clash {
                report.findings.push(Finding::new(
                    label,
                    None,
                    Check::SpaceName,
                    format!(
                        "`{first_ctor}` and `{ctor}` are both declared as `{label}` but hold \
                         different doors ({}): a name is a claim — same name, same doors. A part \
                         of a module's space with doors of its own is \
                         `space_iri(\"<module>:<part>\")`",
                        first_difference(&lines_of(first_top), &lines_of(top))
                    ),
                ));
            }
        }
    }

    // ----- DECLARATIONS -------------------------------------------------------

    /// Every declaration the module made, against what the walk covered: one
    /// finding per declaration the walk never consulted.
    ///
    /// The rule is structural, never "this waiver silenced nothing today": a
    /// standing waiver that happens to be green is doing its job, while a waiver
    /// for an id nothing binds could not have done anything at all.
    fn declaration_checks(&self, coverage: &Coverage, report: &mut Report) {
        for id in &self.live {
            if coverage.cacheable_ran.contains(id) {
                continue;
            }
            let why = self.why_no_cacheable(id, coverage);
            self.inert(
                report,
                id,
                format!(
                    "declared live (`Suite::live`) but nothing held it to it: {why}. The report \
                 prints `declared live: {id}`, which reads as a check that ran"
                ),
            );
        }
        for id in &self.cacheable {
            if coverage.cacheable_ran.contains(id) {
                continue;
            }
            let why = self.why_no_cacheable(id, coverage);
            self.inert(
                report,
                id,
                format!(
                "declared cacheable (`Suite::cacheable`) but nothing held it to it: {why}. The \
                 report prints `declared cacheable: {id}`, which reads as a check that ran"
            ),
            );
        }
        for id in &self.pure {
            if coverage.pure_consulted.contains(id) || coverage.cacheable_unresolved.contains(id) {
                continue;
            }
            let why = if coverage.cacheable_ran.contains(id) {
                "no action of it came back cacheable, so the golden-thread rule it exempts \
                 never applied"
                    .to_string()
            } else {
                self.why_no_cacheable(id, coverage)
            };
            self.inert(
                report,
                id,
                format!(
                "declared pure (`Suite::pure`) but nothing consulted it: {why}. `pure` exempts \
                 a CACHEABLE result from the no-thread-but-its-own-name finding and does nothing else"
            ),
            );
        }
        if self.required_on_mutating {
            let why = if !self.checks.contains(Check::Required) {
                Some("REQUIRED is not selected (`Suite::checks`)")
            } else if !coverage.required_mutating_candidates {
                Some("no walked `Sink` or `Delete` declares a required by-value input without a default")
            } else {
                None
            };
            if let Some(why) = why {
                self.inert(
                    report,
                    SUITE,
                    format!(
                        "`Suite::required_on_mutating` reached nothing: {why}. The report prints \
                         that mutating actions were held to REQUIRED, which reads as a check that ran"
                    ),
                );
            }
        }
        if self.check_failures && !self.checks.contains(Check::Failures) {
            self.inert(
                report,
                SUITE,
                "`Suite::check_failures` reached nothing: FAILURES is not selected \
                 (`Suite::checks`). The report would print FAILURES as checked if it were, so \
                 select it (`Checks::FAILURES`) or drop the declaration"
                    .to_string(),
            );
        }
        self.namespace_declarations(coverage, report);
        self.vocabulary_declarations(coverage, report);
        self.space_declarations(report);
        self.fixture_declarations(coverage, report);
        self.enforced_under_declarations(coverage, report);
        self.fails_under_declarations(coverage, report);
        self.opt_out_declarations(coverage, report);
    }

    /// A [`fails_under`](Suite::fails_under) probe that never ran held its action to
    /// nothing, and the class it names reads as one the module proved.
    fn fails_under_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for (index, (probe, class)) in self.fails_under.iter().enumerate() {
            if coverage.fails_ran.contains(&index) {
                continue;
            }
            let id = &probe.id;
            let why = if let Some(why) = coverage.fails_untargeted.get(&index) {
                format!("its bindings formed no IRI: {why}")
            } else if let Some(why) = self.unreachable_check(id, Check::Failures, coverage, true) {
                why
            } else if !coverage.declares(id, probe.verb) {
                format!(
                    "`{id}` declares no {} action ({})",
                    crate::report::verb_name(probe.verb),
                    verb_list(&coverage.verbs_of(id))
                )
            } else if coverage.is_opted_out(id, Some(probe.verb)) {
                "the action is excluded by `Suite::opt_out`".to_string()
            } else {
                "no binding of the action was invoked".to_string()
            };
            self.inert(
                report,
                id,
                format!(
                    "`Suite::fails_under` probe `{probe}` expecting `{}` never ran: {why}",
                    class.word()
                ),
            );
        }
    }

    /// An [`enforced_under`](Suite::enforced_under) probe that never ran, or ran and
    /// never reached the module's rule, tested nothing it says it did.
    fn enforced_under_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for (index, probe) in self.enforced_under.iter().enumerate() {
            let id = &probe.id;
            let shown = Fixture {
                capability: None,
                ..probe.clone()
            };
            if probe.capability.is_none() {
                self.inert(
                    report,
                    id,
                    format!(
                        "`Suite::enforced_under` probe `{shown}` names no caller \
                         (`Fixture::capability`): it would run as root, which holds every \
                         grant and can be refused by nothing, so it did not run. Give it a \
                         caller holding some grant under the declared prefix, but not the one \
                         for this target"
                    ),
                );
                continue;
            }
            if let Some(lacking) = coverage.under_floor.get(&index) {
                self.inert(
                    report,
                    id,
                    format!(
                        "`Suite::enforced_under` probe `{shown}` was refused by the kernel's \
                         floor before the endpoint ran (its caller lacks `{}`), so the \
                         module's own rule was never reached and the probe tested only what \
                         ENFORCED's no-grants probe already does. Its caller must satisfy \
                         every scope the action declares (some grant under a `…:*` prefix) \
                         and still not hold the one for this target",
                        crate::report::visible(lacking)
                    ),
                );
                continue;
            }
            if coverage.under_ran.contains(&index) {
                continue;
            }
            let why = if let Some(why) = coverage.under_untargeted.get(&index) {
                format!("its bindings formed no IRI: {why}")
            } else if let Some(why) = self.unreachable_check(id, Check::Enforced, coverage, true) {
                why
            } else if !coverage.declares(id, probe.verb) {
                format!(
                    "`{id}` declares no {} action ({})",
                    crate::report::verb_name(probe.verb),
                    verb_list(&coverage.verbs_of(id))
                )
            } else if coverage.is_opted_out(id, Some(probe.verb)) {
                "the action is excluded by `Suite::opt_out`".to_string()
            } else {
                "no binding of the action was invoked".to_string()
            };
            self.inert(
                report,
                id,
                format!("`Suite::enforced_under` probe `{shown}` never ran: {why}"),
            );
        }
    }

    /// Why `CACHEABLE` never ran for `id` — in the order a reader would ask.
    fn why_no_cacheable(&self, id: &str, coverage: &Coverage) -> String {
        if let Some(reason) = self.unreachable_check(id, Check::Cacheable, coverage, true) {
            return reason;
        }
        "CACHEABLE ran for no action of it".to_string()
    }

    /// Why `check` could not run for `id`, structurally — `None` when it could.
    /// `waivers` says whether a per-check waiver counts as a reason (it does for
    /// every declaration except a waiver judging itself).
    fn unreachable_check(
        &self,
        id: &str,
        check: Check,
        coverage: &Coverage,
        waivers: bool,
    ) -> Option<String> {
        if check == Check::SpaceName {
            // Waived per declared SPACE, by label, never per endpoint.
            if !self.checks.contains(check) {
                return Some(format!(
                    "{check} is not selected (`Suite::checks`), so it is already skipped everywhere"
                ));
            }
            if !self.spaces.iter().any(|s| s.label == id) {
                return Some(
                    "no space is declared under that label (a self-named space's label is its \
                     IRI, `urn:iki:space:<module>`; a host-named one's is the label given to \
                     `Suite::host_named_space`)"
                        .to_string(),
                );
            }
            return None;
        }
        if !coverage.walked.contains(id) {
            return Some(unwalked(id, coverage));
        }
        if !self.checks.contains(check) {
            return Some(format!(
                "{check} is not selected (`Suite::checks`), so it is already skipped everywhere"
            ));
        }
        if waivers
            && self
                .opt_out_checks
                .iter()
                .any(|o| o.id == id && o.check == check)
        {
            return Some(format!(
                "{check} is waived for it by a `Suite::opt_out_check`"
            ));
        }
        if check.invokes() && coverage.wholly_opted_out(id) {
            return Some(
                "every action of it is opted out (`Suite::opt_out`), and this check invokes"
                    .to_string(),
            );
        }
        if check.invokes() && coverage.ids_excluded_at.contains(id) && !coverage.any_invoked(id) {
            return Some(
                "every binding of it is excluded (`Suite::opt_out_at`), and this check invokes"
                    .to_string(),
            );
        }
        match check {
            Check::Cacheable | Check::Recompute
                if !coverage.verbs_of(id).iter().any(|v| v.is_cacheable()) =>
            {
                Some(format!(
                    "it declares no cacheable verb ({}), and {check} returns early on a \
                     mutating one",
                    verb_list(&coverage.verbs_of(id))
                ))
            }
            Check::Bodies if !coverage.verbs_of(id).contains(&Verb::Source) => Some(format!(
                "it declares no `Source` ({}), and BODIES probes only `Source`",
                verb_list(&coverage.verbs_of(id))
            )),
            Check::Authority if !coverage.verbs_of(id).iter().any(|v| v.is_mutating()) => {
                Some(format!(
                    "it declares no mutating verb ({}), and AUTHORITY looks only at what a \
                     `Sink` or a `Delete` did under no grants",
                    verb_list(&coverage.verbs_of(id))
                ))
            }
            Check::Required if !coverage.required_eligible.contains(id) => Some(format!(
                "no action of it REQUIRED probes declares a required by-value input without a \
                 default ({}; `Sink` and `Delete` only under `Suite::required_on_mutating`)",
                verb_list(&coverage.verbs_of(id))
            )),
            Check::SkolemRdf | Check::Vocabulary | Check::Datatypes
                if !coverage.rdf_faces.contains(id) =>
            {
                Some(
                    "it declares no RDF face, and the graph checks probe declared RDF outputs only"
                        .to_string(),
                )
            }
            _ => None,
        }
    }

    /// A space declared while SPACE-NAME is not selected was checked by nothing,
    /// and its `space:` line reads as a check that ran.
    fn space_declarations(&self, report: &mut Report) {
        if self.checks.contains(Check::SpaceName) {
            return;
        }
        for decl in &self.spaces {
            let how = match decl.naming() {
                SpaceNaming::SelfNamed => "self-named (`Suite::self_named_space`)",
                SpaceNaming::HostNamed => "host-named (`Suite::host_named_space`)",
            };
            self.inert(
                report,
                &decl.label,
                format!(
                    "declared {how} but SPACE-NAME is not selected (`Suite::checks`), so \
                     nothing checked it. The report prints `space: {}`, which reads as a check \
                     that ran",
                    decl.label
                ),
            );
        }
    }

    fn namespace_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for ns in &self.namespaces {
            if coverage.namespaces_used.contains(ns) {
                continue;
            }
            let why = if !self.checks.contains(Check::Vocabulary) {
                "VOCABULARY is not selected (`Suite::checks`)".to_string()
            } else if !coverage.faces_probed {
                "no RDF face was probed at all".to_string()
            } else {
                "no term in any probed face lies under it, or the terms under it are defined \
                 without it"
                    .to_string()
            };
            self.inert(
                report,
                SUITE,
                format!(
                "registered namespace `{ns}` (`Suite::namespace`) accounted for no term: {why}. \
                 A registration waives every term under it forever, so an unused one is scope \
                 nobody needs — drop it, or narrow it to the prefix the module really serves"
            ),
            );
        }
    }

    fn vocabulary_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for (i, vocabulary) in self.vocabularies.iter().enumerate() {
            let vocabulary = match vocabulary {
                Ok(v) => v,
                Err(e) => {
                    // The module ships this graph: one that cannot be read is its
                    // defect, not an idle declaration, and it checked nothing.
                    let detail = format!(
                        "the vocabulary passed to `Suite::vocabulary` {e}, so no face was held \
                         to it"
                    );
                    if self.checks.contains(Check::Vocabulary) {
                        report
                            .findings
                            .push(Finding::new(SUITE, None, Check::Vocabulary, detail));
                    } else {
                        self.inert(report, SUITE, detail);
                    }
                    continue;
                }
            };
            if coverage.vocabularies_used.contains(&i) {
                continue;
            }
            let why = if !self.checks.contains(Check::Vocabulary) {
                "VOCABULARY is not selected (`Suite::checks`)"
            } else if !coverage.faces_probed {
                "no RDF face was probed at all"
            } else {
                "no term in any probed face lies under its namespace"
            };
            let shown: Vec<&str> = vocabulary.namespaces().iter().map(String::as_str).collect();
            self.inert(
                report,
                SUITE,
                format!(
                    "the vocabulary for `{}` (`Suite::vocabulary`) held no term to its \
                     definition: {why}. Drop it, or probe the face that speaks it",
                    shown.join("` `")
                ),
            );
        }
    }

    fn fixture_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for fixture in &self.fixtures {
            let id = &fixture.id;
            if !coverage.walked.contains(id) {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` was never used: {}",
                        unwalked(id, coverage)
                    ),
                );
                continue;
            }
            for var in fixture.bindings.keys() {
                let known = coverage.vars.get(id).is_some_and(|v| v.contains(var));
                if !known {
                    self.inert(
                        report,
                        id,
                        format!(
                        "fixture `{fixture}` binds `{var}`, which is not a template variable of \
                         any pattern `{id}` is bound at ({}): the binding was ignored and the \
                         IRI was formed without it",
                        if coverage.vars.get(id).is_none_or(BTreeSet::is_empty) {
                            "it is bound at an exact IRI".to_string()
                        } else {
                            coverage.vars[id]
                                .iter()
                                .map(|v| format!("`{v}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    ),
                    );
                }
            }
            // What the fixture supplies PER ACTION: arguments, a caller, or both.
            let (what, they) = match (fixture.args.is_empty(), fixture.capability.is_none()) {
                // A binding-only fixture is per ENTRY: its verb is ignored by
                // design, so a verb that names no action says nothing about it.
                (true, true) => continue,
                (false, true) => ("arguments", "they were"),
                (true, false) => ("a caller", "it was"),
                (false, false) => ("arguments and a caller", "they were"),
            };
            let verb = fixture.verb;
            if coverage.declares(id, verb) && !coverage.is_opted_out(id, Some(verb)) {
                self.fixture_argument_names(fixture, coverage, report);
            }
            if !coverage.declares(id, verb) {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` supplies {what} for a {} action `{id}` does not \
                     declare ({}): {they} never used",
                        crate::report::verb_name(verb),
                        verb_list(&coverage.verbs_of(id))
                    ),
                );
            } else if coverage.is_opted_out(id, Some(verb)) {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` supplies {what} for an action excluded by \
                     `Suite::opt_out`: {they} never used"
                    ),
                );
            } else if fixture.capability.is_some()
                && !coverage
                    .caller_used
                    .iter()
                    .any(|(i, v)| i == id && *v == verb)
            {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` names a caller no probe resolved as: {}",
                        if verb.is_mutating() {
                            "a mutating action is fired as a caller only by the pipeline \
                             probe, on an action declaring `content` (ENFORCED and AUTHORITY \
                             fire it under no grants, never as the fixture's caller), or every \
                             check that would fire it is waived or unselected"
                        } else {
                            "every check that resolves it as a caller (OUTPUTS, SKOLEM-RDF, \
                             VOCABULARY, CACHEABLE) is waived or unselected, or no binding of \
                             it was invoked"
                        }
                    ),
                );
            }
        }
    }

    /// Every argument a fixture sends must name an input its action declares by
    /// value (ledger #340). The walk puts every one on the wire, so an undeclared
    /// name is one of two bad things and the suite cannot tell which: the endpoint
    /// reads an input its contract does not declare (the lie this suite exists to
    /// catch), or it ignored the value and the walk tested something other than
    /// what the fixture says. `as`, the universal conneg selector, is the one
    /// undeclared name any endpoint may be sent.
    fn fixture_argument_names(&self, fixture: &Fixture, coverage: &Coverage, report: &mut Report) {
        let id = &fixture.id;
        let verb = crate::report::verb_name(fixture.verb);
        let empty = DeclaredInputs::default();
        let declared = coverage.inputs.get(&(id.clone(), verb)).unwrap_or(&empty);
        for name in fixture.args.keys() {
            if name == CONNEG || declared.by_value.contains(name) {
                continue;
            }
            // An open argument set admits any other name, as core's checker does; a
            // binding sent as an argument is still a mistake there.
            if declared.open && !declared.binding.contains(name) {
                continue;
            }
            let detail = if declared.binding.contains(name) {
                format!(
                    "fixture `{fixture}` sends `{name}` as an argument, but `{name}` is a binding \
                     input (a template variable): an argument never reaches the IRI, which was \
                     formed without it. Bind it with `Fixture::binding` (`.binding(\"{name}\", …)`)"
                )
            } else {
                format!(
                    "fixture `{fixture}` sends `{name}`, which no input of its {verb} action \
                     declares ({}): the walk sent it anyway, so either the endpoint reads an input \
                     its contract does not declare, or it ignored the value and the walk tested \
                     something other than what the fixture says. `{CONNEG}`, the conneg selector, \
                     is the one undeclared name a fixture may send",
                    if declared.by_value.is_empty() {
                        "it declares no by-value input".to_string()
                    } else {
                        format!(
                            "it declares {}",
                            declared
                                .by_value
                                .iter()
                                .map(|n| format!("`{n}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                )
            };
            self.inert(report, id, detail);
        }
    }

    fn opt_out_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for out in &self.opt_outs {
            let id = &out.id;
            if coverage.is_opted_out(id, out.verb) {
                continue;
            }
            let why = if !coverage.walked.contains(id) {
                unwalked(id, coverage)
            } else {
                match out.verb {
                    Some(verb) => format!(
                        "`{id}` declares no {} action ({})",
                        crate::report::verb_name(verb),
                        verb_list(&coverage.verbs_of(id))
                    ),
                    None => format!("`{id}` declares no action at all"),
                }
            };
            self.inert(
                report,
                id,
                format!(
                    "opted out of the invoking checks ({}) but excluded nothing: {why}",
                    out.reason
                ),
            );
        }
        for out in &self.opt_outs_at {
            if coverage.patterns_excluded.contains(&out.pattern) {
                continue;
            }
            // A pattern is matched verbatim against what the space reports, so the
            // two ways to miss are a typo and an expanded IRI in place of the
            // template. Print what the walk did reach: that is the difference.
            let why = if coverage.patterns.contains(&out.pattern) {
                match out.verb {
                    Some(verb) => format!(
                        "the binding was reached, but nothing bound there declares a {} action",
                        crate::report::verb_name(verb)
                    ),
                    None => "the binding was reached, but nothing bound there declares an action"
                        .to_string(),
                }
            } else {
                format!(
                    "the walk reached no binding with that pattern (it is matched verbatim \
                     against the space's own pattern — the template, not an expanded IRI). \
                     The walk reached: {}",
                    if coverage.patterns.is_empty() {
                        "no pattern at all".to_string()
                    } else {
                        coverage
                            .patterns
                            .iter()
                            .map(|p| format!("`{p}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )
            };
            self.inert(
                report,
                SUITE,
                format!(
                    "opted out of the invoking checks at `{}` ({}) but excluded nothing: {why}",
                    out.pattern, out.reason
                ),
            );
        }
        for out in &self.opt_out_checks {
            // A waiver is judged against the check it waives, with ITSELF discounted
            // — otherwise every waiver would report as its own reason to be inert.
            let Some(why) = self.unreachable_check(&out.id, out.check, coverage, false) else {
                continue;
            };
            self.inert(
                report,
                &out.id,
                format!(
                    "waived {} ({}) but that check could not have run for it anyway: {why}",
                    out.check, out.reason
                ),
            );
        }
    }

    /// Record one inert-declaration finding, unless DECLARATIONS is itself
    /// unselected or waived for this id.
    /// A waiver of DECLARATIONS for `id` is honored by the caller, which sets the
    /// finding aside ([`Suite::set_aside_waived`]) and so can say when it suppressed
    /// nothing.
    fn inert(&self, report: &mut Report, id: &str, detail: String) {
        if self.checks.contains(Check::Declarations) {
            report
                .findings
                .push(Finding::new(id, None, Check::Declarations, detail));
        }
    }

    // ----- description-only checks -------------------------------------------

    /// Every selected description-only check, WAIVED ones included: the caller sets
    /// a waived check's findings aside ([`Suite::set_aside_waived`]), which is how a
    /// waiver that suppressed nothing can be told from one doing its job. Reading a
    /// description fires nothing, so running a waived one costs nothing.
    fn static_checks(&self, description: &Description, report: &mut Report) {
        if self.checks.contains(Check::ArgSpecs) {
            argspecs(description, report);
        }
        if self.checks.contains(Check::RequiresVerb) {
            requires_verb(description, report);
        }
        if self.checks.contains(Check::Names) {
            names(description, report);
        }
        if self.checks.contains(Check::Validate) {
            validate(description, report);
        }
        if self.checks.contains(Check::Pipeline) {
            pipeline_static(description, report);
        }
    }

    /// Every template variable of the entry's pattern must be a declared
    /// `Binding`-source input of every action — otherwise the manifold cannot form
    /// the IRI from the contract and drops the action.
    /// Runs when ARGSPECS is selected, waived or not, like [`static_checks`](Self::static_checks).
    fn template_checks(&self, entry: &SpaceEntry, description: &Description, report: &mut Report) {
        if !self.checks.contains(Check::ArgSpecs) {
            return;
        }
        let Some(vars) = template_vars(&entry.pattern) else {
            return;
        };
        for spec in description.action_specs() {
            for var in &vars {
                let declared = spec
                    .inputs
                    .iter()
                    .any(|i| i.name == *var && i.source == InputSource::Binding);
                if !declared {
                    report.findings.push(Finding::new(
                        &description.id,
                        Some(spec.verb),
                        Check::ArgSpecs,
                        format!(
                            "template variable `{var}` of `{}` is not a declared binding input \
                             (`ArgSpec::new(\"{var}\").binding()`): the manifold cannot form the \
                             IRI from the contract, so the action is undrivable",
                            entry.pattern
                        ),
                    ));
                }
            }
        }
    }

    // ----- inputs -------------------------------------------------------------

    fn opted_out(&self, id: &str, verb: Verb) -> bool {
        self.opt_outs
            .iter()
            .any(|o| o.id == id && o.verb.is_none_or(|v| v == verb))
    }

    /// The `opt_out_at` excluding this action at this BINDING, if any — matched on
    /// the pattern exactly as the space reports it.
    fn opted_out_at(&self, pattern: &str, verb: Verb) -> Option<&OptOutAt> {
        self.opt_outs_at
            .iter()
            .find(|o| o.pattern == pattern && o.verb.is_none_or(|v| v == verb))
    }

    /// Whether `check` runs for `id`: selected suite-wide ([`Suite::checks`]) and not
    /// excluded for this endpoint ([`Suite::opt_out_check`]). Every check asks this
    /// rather than `checks.contains`, so a per-id waiver silences one rule and one
    /// endpoint — never a second check by accident.
    fn runs(&self, id: &str, check: Check) -> bool {
        self.checks.contains(check) && !self.waived(id, check)
    }

    /// Whether a [`Suite::opt_out_check`] waives `check` for `id`.
    fn waived(&self, id: &str, check: Check) -> bool {
        self.opt_out_checks
            .iter()
            .any(|o| o.id == id && o.check == check)
    }

    /// The selected INVOKING checks waived for `id`, each once: what an action of it
    /// is re-run for with the findings set aside ([`Action::set_aside`]).
    fn waived_invoking(&self, id: &str) -> Vec<Check> {
        let mut checks: Vec<Check> = Vec::new();
        for o in &self.opt_out_checks {
            if o.id == id
                && o.check.invokes()
                && self.checks.contains(o.check)
                && !checks.contains(&o.check)
            {
                checks.push(o.check);
            }
        }
        checks
    }

    /// Take the findings appended since `from` that a waiver covers back out of the
    /// report, counting each against its waiver. The description-only checks and
    /// DECLARATIONS run waived or not and are honored here, so a waiver that took
    /// nothing out is visible ([`Evidence::idle_waivers`]). The findings that stay
    /// keep their order, so the report reads exactly as it did when those checks
    /// were skipped outright.
    fn set_aside_waived(&self, report: &mut Report, from: usize, tally: &mut Tally) {
        if self.opt_out_checks.is_empty() || from >= report.findings.len() {
            return;
        }
        for finding in report.findings.split_off(from) {
            if self.waived(&finding.endpoint, finding.check) {
                tally.record(&finding.endpoint, finding.check, Some(1));
            } else {
                report.findings.push(finding);
            }
        }
    }

    /// Why REQUIRED dropped nothing, when it is selected and did not.
    fn required_nothing(&self, coverage: &Coverage, report: &Report) -> Option<String> {
        if !self.checks.contains(Check::Required) || !report.declared.evidence.required.is_empty() {
            return None;
        }
        let why = if report.endpoints == 0 {
            "the walk reached no endpoint"
        } else if !coverage.required_eligible.is_empty() {
            "every action that declares a required by-value input without a default was \
             opted out, waived, or did not resolve (see the `opted out:` and `unprobed:` lines)"
        } else if self.required_on_mutating {
            "no walked action declares a required by-value input without a default"
        } else if coverage.required_mutating_candidates {
            "no `Source` or `Exists` action declares a required by-value input without a \
             default; a `Sink` or `Delete` does, and REQUIRED fires those only under \
             `Suite::required_on_mutating`"
        } else {
            "no `Source` or `Exists` action declares a required by-value input without a \
             default, and `Sink` and `Delete` are not walked (`Suite::required_on_mutating` \
             is off)"
        };
        Some(why.to_string())
    }

    /// The waivers that suppressed nothing: judged (every description it covers read,
    /// every action it covers re-run without a mutating firing it was refused), and
    /// nothing found. A waiver DECLARATIONS already reports as structurally inert is
    /// not repeated here, and a SPACE-NAME waiver is never judged: re-running it would
    /// fill in the `space:` line the waiver keeps empty.
    fn idle_waivers(&self, coverage: &Coverage, tally: &Tally) -> Vec<OptedOutCheck> {
        self.opt_out_checks
            .iter()
            .filter(|o| o.check != Check::SpaceName)
            .filter(|o| {
                self.unreachable_check(&o.id, o.check, coverage, false)
                    .is_none()
            })
            .filter(|o| tally.idle(&o.id, o.check))
            .map(|o| OptedOutCheck {
                endpoint: o.id.clone(),
                check: o.check,
                reason: o.reason.clone(),
            })
            .collect()
    }

    fn fixture_for(&self, id: &str, verb: Verb) -> Option<&Fixture> {
        self.fixtures.iter().find(|f| f.id == id && f.verb == verb)
    }

    /// The concrete IRI to resolve for an entry: the pattern itself when it is an
    /// IRI, else the template expanded with fixture bindings or values derived
    /// from the declared binding inputs.
    fn target_for(&self, entry: &SpaceEntry, description: &Description) -> Result<Iri, String> {
        self.target_with(entry, description, None)
    }

    /// [`target_for`](Self::target_for), with `first` (an `enforced_under` probe's
    /// bindings) read ahead of every fixture's.
    fn target_with(
        &self,
        entry: &SpaceEntry,
        description: &Description,
        first: Option<&BTreeMap<String, String>>,
    ) -> Result<Iri, String> {
        if let Ok(iri) = Iri::parse(&entry.pattern) {
            return Ok(iri);
        }
        let template = UriTemplate::parse(&entry.pattern).map_err(|e| {
            format!(
                "bound at `{}`, which is neither an IRI nor a URI template: {e}",
                entry.pattern
            )
        })?;
        let mut bindings = ikigai_core::Bindings::new();
        for var in template.variables() {
            let from_fixture = first.and_then(|b| b.get(var).cloned()).or_else(|| {
                self.fixtures
                    .iter()
                    .filter(|f| f.id == description.id)
                    .find_map(|f| f.bindings.get(var).cloned())
            });
            let value = from_fixture.unwrap_or_else(|| {
                binding_input(description, var)
                    .map(sample_value)
                    .unwrap_or_else(|| PLACEHOLDER.to_string())
            });
            bindings.insert(var, value);
        }
        let expanded = template
            .expand(&bindings)
            .ok_or_else(|| format!("template `{}` could not be expanded", entry.pattern))?;
        Iri::parse(&expanded).map_err(|e| {
            format!(
                "template `{}` expands to `{expanded}`, which is not an IRI ({e}); supply a \
                 Fixture binding",
                entry.pattern
            )
        })
    }

    /// The minimal by-value arguments for an action: one derived value per required
    /// `Argument`-source input, overlaid with the fixture's arguments, then completed
    /// with what the contract's relations ask (a member of each exactly-one or
    /// at-least-one group, every input a given one `needs`). An envelope's fields arrive
    /// inside its carrier, so only the carrier is derived unless the fixture proposes
    /// fields.
    fn args_for(&self, id: &str, spec: &ActionSpec) -> Args {
        let fixture = self.fixture_for(id, spec.verb);
        let fields_given = match &spec.arguments {
            Arguments::Envelope(carrier) => fixture.is_some_and(|f| {
                f.args
                    .keys()
                    .any(|k| k != carrier && spec.inputs.iter().any(|i| i.name == *k))
            }),
            _ => false,
        };
        let mut args: Args = minimal_inputs(spec, fields_given)
            .map(|i| (i.name.clone(), sample_value(i).into_bytes()))
            .collect();
        if let Some(fixture) = fixture {
            for (k, v) in &fixture.args {
                args.insert(k.clone(), v.clone());
            }
        }
        complete_relations(spec, &mut args);
        args
    }
}

/// The `Binding`-source input named `var` in the flat inputs or any explicit action.
fn binding_input<'a>(description: &'a Description, var: &str) -> Option<&'a ArgSpec> {
    description
        .inputs
        .iter()
        .chain(description.actions.iter().flat_map(|a| a.inputs.iter()))
        .find(|i| i.name == var && i.source == InputSource::Binding)
}

/// Why the walk never reached `id`: scoped out by [`Suite::own`], or bound nowhere.
fn unwalked(id: &str, coverage: &Coverage) -> String {
    if coverage.inherited.contains(id) {
        "it is inherited (no `Suite::own` matches it), and an inherited endpoint is not walked: \
         delete the declaration (an opt-out's reason belongs in `Suite::inherit`)"
            .to_string()
    } else {
        "the walk reached no endpoint with that id (the id is the `Description::id`, not the \
         bound IRI)"
            .to_string()
    }
}

/// The kernel's own operations, as this kernel lists them: what lets the walk tell a
/// peer's operations from a module's when a mount has RENAMED them (ledger #335).
///
/// An alias mount re-prefixes a peer's whole catalog (`urn:kernel:catalog` arrives as
/// `urn:edge:kernel:catalog`), so a prefix test on `urn:kernel:` does not see them, and
/// a module walked through one had core's findings reported as its own. A mounted
/// binding (its entry carries an `origin`) is one of core's operations when its
/// description id is one this kernel's own operations use (`kernel-catalog`), or when
/// its pattern is one of theirs re-prefixed (`…:kernel:catalog` for
/// `urn:kernel:catalog`), or, for an operation a newer peer has and this kernel does
/// not, when its id is `kernel-…` and its pattern holds a `:kernel:` segment.
struct KernelOps {
    /// The description ids of this kernel's core-sealed operations.
    ids: BTreeSet<String>,
    /// Their patterns without the leading `urn:` (`kernel:catalog`), the part an
    /// alias keeps.
    names: BTreeSet<String>,
}

impl KernelOps {
    fn of(kernel: &Kernel, entries: &[SpaceEntry], seals: &[(String, SealOwner)]) -> Self {
        let mut ops = KernelOps {
            ids: BTreeSet::new(),
            names: BTreeSet::new(),
        };
        for entry in entries
            .iter()
            .filter(|e| e.origin.is_none() && core_owned(&e.pattern, seals))
        {
            if let Some(name) = entry.pattern.strip_prefix("urn:") {
                ops.names.insert(name.to_string());
            }
            if let Some(description) = kernel.describe_pattern(&entry.pattern) {
                ops.ids.insert(description.id);
            }
        }
        ops
    }

    /// Whether a MOUNTED binding is one of core's operations under another name.
    fn renamed(&self, entry: &SpaceEntry, description: Option<&Description>) -> bool {
        if entry.origin.is_none() {
            return false;
        }
        let id = description.map(|d| d.id.as_str());
        if id.is_some_and(|id| self.ids.contains(id)) {
            return true;
        }
        let re_prefixed = self.names.iter().any(|name| {
            entry
                .pattern
                .strip_suffix(name.as_str())
                .is_some_and(|prefix| prefix.ends_with(':'))
        });
        re_prefixed
            || (id.is_some_and(|id| id.starts_with("kernel-"))
                && entry.pattern.contains(":kernel:"))
    }
}

/// A mounted binding that describes with no action (ledger #334).
struct Contractless {
    origin: String,
    pattern: String,
    id: String,
    summary: String,
}

/// One finding per mount (origin and id) whose bindings described with no action.
///
/// A peer that cannot answer `Meta` (built with `Kernel::new`, no Meta renderer) gives
/// every endpoint it serves the same anonymous, action-less placeholder description
/// (`remote`, from `ikigai-resolve`), and the walk used to count them as ONE endpoint
/// with one "declares no action" line: a whole federated kernel read as small rather
/// than broken. These bindings are not walked (there is no action to walk), and the
/// finding is not subject to selection or waiver, like an `own` pattern that matched
/// nothing: a mount whose contract the walk could not read turns the walk of that mount
/// into a no-op that passes.
fn contractless_findings(contractless: &[Contractless], report: &mut Report) {
    let mut groups: Vec<(&str, &str, Vec<&Contractless>)> = Vec::new();
    for c in contractless {
        match groups
            .iter_mut()
            .find(|(origin, id, _)| *origin == c.origin && *id == c.id)
        {
            Some((_, _, members)) => members.push(c),
            None => groups.push((&c.origin, &c.id, vec![c])),
        }
    }
    for (origin, id, members) in groups {
        let patterns = members
            .iter()
            .take(SHOWN_PATTERNS)
            .map(|c| format!("`{}`", c.pattern))
            .collect::<Vec<_>>()
            .join(", ");
        let more = members.len().saturating_sub(SHOWN_PATTERNS);
        let said = members
            .iter()
            .map(|c| c.summary.trim())
            .find(|s| !s.is_empty())
            .map(|s| {
                let stop = if s.ends_with(['.', '!', '?']) {
                    ""
                } else {
                    "."
                };
                format!(" The mount says: {s}{stop}")
            })
            .unwrap_or_default();
        report.findings.push(Finding::new(
            id,
            None,
            Check::ArgSpecs,
            format!(
                "{} binding(s) mounted from `{origin}` describe as `{id}` with no action, so \
                 none of them was walked: {patterns}{}. A peer that cannot answer `Meta` gives \
                 every endpoint it serves this one anonymous, action-less placeholder, and the \
                 usual cause is a peer kernel built with `Kernel::new`, which has NO Meta \
                 renderer: build it with `Kernel::with_meta_renderer` and \
                 `ikigai_vocab::TurtleRenderer`, which serves the JSON face a mount reads. A peer endpoint that really \
                 declares no action is an ARGSPECS finding on the peer's own walk.{said} This \
                 finding cannot be unselected or waived: a mount the walk cannot read is a \
                 walk of nothing that passes",
                members.len(),
                if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                }
            ),
        ));
    }
}

/// How many patterns a mount finding lists before it counts the rest.
const SHOWN_PATTERNS: usize = 5;

/// The inputs [`Check::Required`] drops one at a time: declared required, by value,
/// with no default. A default beside `required` is already an ARGSPECS finding, and a
/// call missing it succeeds on the default by design, so it is not reported twice.
fn droppable(spec: &ActionSpec) -> Vec<&ArgSpec> {
    spec.inputs
        .iter()
        .filter(|i| i.required && i.source == InputSource::Argument && i.default.is_none())
        .collect()
}

/// One call [`Check::Required`] makes: the baseline with an input dropped, or with an
/// input group's members rearranged.
struct RequiredCall<'a> {
    args: Args,
    shape: CallShape,
    /// The group the call is about, for a group call.
    group: Option<&'a InputGroup>,
    /// The group members the call gives (empty: none of them).
    given: Vec<String>,
}

enum CallShape {
    /// The baseline without this required input.
    Dropped(String),
    /// The baseline with the group's members replaced by `given`.
    Group,
}

impl RequiredCall<'_> {
    /// The [`RequiredProbe::input`] field: the dropped input, or the group's node
    /// segment.
    fn input(&self) -> String {
        match (&self.shape, self.group) {
            (CallShape::Dropped(name), _) => name.clone(),
            (CallShape::Group, Some(group)) => group.node_segment(),
            (CallShape::Group, None) => String::new(),
        }
    }

    /// `with none of them` / `with only `in`` / `with both `content`, `in``.
    fn described(&self) -> String {
        let names = |n: &[String]| {
            n.iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match (&self.shape, self.given.as_slice()) {
            (CallShape::Dropped(name), _) => format!("without `{name}`"),
            (CallShape::Group, []) => "with none of them".to_string(),
            (CallShape::Group, [one]) => format!("with only `{one}`"),
            (CallShape::Group, many) => format!("with both {}", names(many)),
        }
    }

    /// `exactly one of `content`, `in``.
    fn rule(&self) -> String {
        match self.group {
            Some(group) => format!(
                "{} {}",
                match group.kind {
                    GroupKind::ExactlyOne => "exactly one of",
                    GroupKind::AtLeastOne => "at least one of",
                    _ => "at most one of",
                },
                group
                    .members
                    .iter()
                    .map(|m| format!("`{m}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => String::new(),
        }
    }
}

/// Whether [`Check::Required`] has anything to probe on this action: a required input
/// it can drop, or an input group whose rule it can break (core 0.1.105, ledger #15).
fn required_probes_something(spec: &ActionSpec) -> bool {
    !droppable(spec).is_empty() || !spec.groups.is_empty()
}

/// Whether `name` is GIVEN in `args`, as core's checker counts a group member: present
/// with a value that is not blank.
fn given(args: &Args, name: &str) -> bool {
    args.get(name)
        .is_some_and(|v| !String::from_utf8_lossy(v).trim().is_empty())
}

/// The inputs an action's minimal call sends of its own accord: the required by-value
/// ones, except that an [`Envelope`](Arguments::Envelope)'s fields arrive inside its
/// carrier, so the call sends the carrier unless `fields_given` (a fixture proposes
/// fields, and then the carrier is not sent).
fn minimal_inputs(spec: &ActionSpec, fields_given: bool) -> impl Iterator<Item = &ArgSpec> {
    let carrier = match &spec.arguments {
        Arguments::Envelope(carrier) => Some(carrier.as_str()),
        _ => None,
    };
    spec.inputs.iter().filter(move |i| {
        i.required
            && i.source == InputSource::Argument
            && match carrier {
                Some(carrier) if fields_given => i.name != carrier,
                Some(carrier) => i.name == carrier,
                None => true,
            }
    })
}

/// Fill in what the contract's RELATIONS ask of a call (core 0.1.105): a member of each
/// exactly-one and at-least-one group none of whose members is given (the group's `pipe`
/// member, else its first), and every input a given input, or a given value's
/// [`ValueNote`](ikigai_core::ValueNote), `needs`. Each with the input's sample value.
/// What is already given is never changed, so a fixture's choice of member stands.
fn complete_relations(spec: &ActionSpec, args: &mut Args) {
    let input = |name: &str| spec.inputs.iter().find(|i| i.name == name);
    // Each pass adds at least one input or stops, so the inputs bound the passes.
    for _ in 0..=spec.inputs.len() {
        let mut add: Vec<&ArgSpec> = Vec::new();
        for group in &spec.groups {
            if !matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne) {
                continue;
            }
            if group.members.iter().any(|m| given(args, m)) {
                continue;
            }
            let pick = group
                .pipe
                .as_deref()
                .or(group.members.first().map(String::as_str));
            add.extend(pick.and_then(input));
        }
        for arg in &spec.inputs {
            if !given(args, &arg.name) {
                continue;
            }
            let mut needed: Vec<&str> = arg.needs.iter().map(String::as_str).collect();
            let value = String::from_utf8_lossy(&args[&arg.name]).into_owned();
            let items = match arg.list {
                Some(separator) => separator.split(&value),
                None => vec![value.as_str()],
            };
            for item in items {
                if let Some(note) = arg.values.get(item) {
                    needed.extend(note.needs.iter().map(String::as_str));
                }
            }
            add.extend(
                needed
                    .into_iter()
                    .filter(|n| !given(args, n))
                    .filter_map(input),
            );
        }
        if add.is_empty() {
            return;
        }
        for arg in add {
            args.entry(arg.name.clone())
                .or_insert_with(|| sample_value(arg).into_bytes());
        }
    }
}

/// `args` as core's checker reads a proposed call.
fn proposed(args: &Args) -> ProposedArgs {
    ProposedArgs::from_pairs(
        args.iter()
            .map(|(k, v)| (k.clone(), String::from_utf8_lossy(v).into_owned())),
    )
}

/// The required input of `spec` that declares an EMPTY enumeration (`"one_of": []`,
/// ledger #41): it accepts no value, so no call conforms and the kernel's
/// `urn:kernel:actions` does not offer the action.
fn uncallable_by(spec: &ActionSpec) -> Option<&ArgSpec> {
    spec.inputs.iter().find(|i| {
        i.required && i.source == InputSource::Argument && i.is_enumerated() && i.one_of.is_empty()
    })
}

/// The variables of a template pattern, or `None` for an exact IRI / a malformed
/// pattern.
fn template_vars(pattern: &str) -> Option<Vec<String>> {
    if Iri::parse(pattern).is_ok() {
        return None;
    }
    let template = UriTemplate::parse(pattern).ok()?;
    let vars: Vec<String> = template.variables().map(str::to_string).collect();
    if vars.is_empty() {
        None
    } else {
        Some(vars)
    }
}

/// The smallest plausible value the ArgSpec admits: its default, else its first
/// declared example (core 0.1.105: `Description::validate` holds an example to the
/// contract, so it is a value the author vouches for), else the first `one_of`, else a
/// value shaped by its `class`, else a placeholder.
fn sample_value(spec: &ArgSpec) -> String {
    if let Some(default) = &spec.default {
        return default.clone();
    }
    if let Some(example) = spec.examples.first() {
        return example.clone();
    }
    if let Some(first) = spec.one_of.first() {
        return first.clone();
    }
    let Some(class) = spec.class.as_deref() else {
        return PLACEHOLDER.to_string();
    };
    let Some(xsd) = class.strip_prefix(XSD) else {
        // An entity class (schema:Person, ik:Endpoint): the value is a reference.
        return PLACEHOLDER_IRI.to_string();
    };
    match xsd {
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "unsignedInt" | "unsignedLong" | "unsignedShort" => "1".into(),
        "decimal" | "double" | "float" => "1.0".into(),
        "boolean" => "true".into(),
        "dateTime" | "dateTimeStamp" => "2026-01-01T00:00:00Z".into(),
        "date" => "2026-01-01".into(),
        "time" => "00:00:00".into(),
        "duration" => "PT1S".into(),
        "anyURI" => PLACEHOLDER_IRI.into(),
        "language" => "en".into(),
        _ => PLACEHOLDER.into(),
    }
}

/// What a set-aside run gets in place of a firing it may not make
/// ([`Action::may_fire`]). Never reaches the report: the run's findings are set aside,
/// and a refused firing leaves the waiver unjudged.
fn set_aside_refusal() -> Error {
    Error::Endpoint(
        "not fired: a set-aside run of a waived check fires no `Sink` or `Delete`".into(),
    )
}

fn build_request(verb: Verb, target: &Iri, args: &Args) -> Request {
    let mut request = Request::new(verb, target.clone());
    for (name, value) in args {
        request = request.with_arg(name, ArgRef::Inline(value.clone()));
    }
    request
}

/// `it declares source, exists` / `it declares nothing` — the verbs an id was
/// walked with, for a finding that has to say why a declaration missed.
fn verb_list(verbs: &[Verb]) -> String {
    if verbs.is_empty() {
        return "it declares no action".to_string();
    }
    format!(
        "it declares {}",
        verbs
            .iter()
            .map(|v| crate::report::verb_name(*v))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn requires_list(spec: &ActionSpec) -> String {
    spec.requires
        .iter()
        .map(|r| format!("`{r}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ----- ARGSPECS ---------------------------------------------------------------

fn argspecs(description: &Description, report: &mut Report) {
    let specs = description.action_specs();
    if specs.is_empty() {
        report.findings.push(Finding::new(
            &description.id,
            None,
            Check::ArgSpecs,
            "declares no action: no verb other than Meta, so nothing is selectable and no \
             `requires` can be enforced (`.verb(Verb::Source)` or `.action(…)`)",
        ));
    }
    for spec in specs {
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for input in &spec.inputs {
            if !names.insert(&input.name) {
                report.findings.push(Finding::new(
                    &description.id,
                    Some(spec.verb),
                    Check::ArgSpecs,
                    format!("input `{}` is declared twice", input.name),
                ));
            }
            match input.class.as_deref() {
                None => report.findings.push(Finding::new(
                    &description.id,
                    Some(spec.verb),
                    Check::ArgSpecs,
                    format!(
                        "input `{}` has no class: declare an rdfs:Class IRI for an entity or an \
                         XSD datatype IRI for a scalar (`.class(\"http://www.w3.org/2001/\
                         XMLSchema#string\")`)",
                        input.name
                    ),
                )),
                Some(class) if !class.contains(':') => report.findings.push(Finding::new(
                    &description.id,
                    Some(spec.verb),
                    Check::ArgSpecs,
                    format!(
                        "class `{class}` of input `{}` is not an IRI (no scheme)",
                        input.name
                    ),
                )),
                Some(_) => {}
            }
            if let Some(default) = &input.default {
                if !input.one_of.is_empty() && !input.one_of.contains(default) {
                    report.findings.push(Finding::new(
                        &description.id,
                        Some(spec.verb),
                        Check::ArgSpecs,
                        format!(
                            "input `{}` defaults to `{default}`, which is not one of its \
                             declared values ({})",
                            input.name,
                            input.one_of.join(", ")
                        ),
                    ));
                }
                if input.required {
                    report.findings.push(Finding::new(
                        &description.id,
                        Some(spec.verb),
                        Check::ArgSpecs,
                        format!(
                            "input `{}` has a default but is marked required: a default implies \
                             optional",
                            input.name
                        ),
                    ));
                }
            }
        }
    }
}

// ----- REQUIRES-VERB ----------------------------------------------------------

fn requires_verb(description: &Description, report: &mut Report) {
    let has_verb = description.verbs.iter().any(|v| *v != Verb::Meta);
    if !description.requires.is_empty() && !has_verb {
        report.findings.push(Finding::new(
            &description.id,
            None,
            Check::RequiresVerb,
            format!(
                "declares requires {} but no verb: since ikigai-core 0.1.85 the kernel enforces \
                 it on every verb, but the catalog cannot say which actions it gates \
                 (add `.verb(…)`)",
                description
                    .requires
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
}

// ----- NAMES ------------------------------------------------------------------

fn is_kebab(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && !id.contains("--")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn names(description: &Description, report: &mut Report) {
    if !is_kebab(&description.id) {
        report.findings.push(Finding::new(
            &description.id,
            None,
            Check::Names,
            format!(
                "id `{}` is not a kebab-case noun (`tag-suggest`, `kernel-catalog`): \
                 the convention ikigai-core's crate docs state, and the MCP projection \
                 derives an agent's tool name from it",
                description.id
            ),
        ));
    }
}

// ----- VALIDATE ---------------------------------------------------------------

/// Core's own refusal, reported. Matched on the refusal and never on its variant:
/// core 0.1.99 refused an `IRIREF` character as `Error::Endpoint` and a code point no
/// IRI carries as `Error::InvalidArgument`, and core 0.1.100 makes them one variant
/// (ledger #1361), so a match on either would go quiet across a release.
///
/// Since core 0.1.100 the emitters ENCODE a name and degrade an authored IRI that is
/// not one to a literal, so a refused value no longer breaks a document: the finding
/// says what it is, the strict check on an authoring mistake, not the thing that keeps
/// the Turtle parsing (ledger #1385).
///
/// Since core 0.1.105 `validate` also refuses a contract whose RELATIONS it cannot
/// honor (ledger #1547): an input group, a `needs`, a value note, an envelope, an
/// example, an `output_from`, or a DEFAULT (on a required input, or one the contract's
/// own checker refuses). Since core 0.1.106 an optional binding may carry a default, which
/// the kernel fills when the matched name omits its segment; a REQUIRED binding's default
/// is still refused (ledger #1547). Those are reported with what they cost
/// instead: the kernel injects a declared default, and the pre-flight trusts the rest.
fn validate(description: &Description, report: &mut Report) {
    let Err(refusal) = description.validate() else {
        return;
    };
    let field = match &refusal {
        Error::InvalidArgument { name, .. } => name.as_str(),
        _ => "",
    };
    let cost = match field {
        "default" => {
            "Since core 0.1.105 the kernel INJECTS a declared default (ledger #19), so a \
             default the contract cannot honor is one every caller who omits the input is \
             handed: drop it, or make it a value the contract accepts. Since core 0.1.106 an \
             OPTIONAL binding may carry one (`.binding().optional()`; the kernel fills it when \
             the matched name leaves its segment out, ledger #1547), but a required binding is \
             part of every name the endpoint answers and is never omitted: drop \
             `.default_value(…)` from it, or make the binding optional and bind the shorter \
             name too"
        }
        "group" | "needs" | "values" | "arguments" | "examples" | "output_from" => {
            "The contract states a relation it cannot honor, and core's one checker \
             (`ActionSpec::check`, behind `urn:kernel:validate` and every MCP tool schema) \
             trusts it as written. `bind` does not validate, so this is where the refusal \
             is seen"
        }
        _ => {
            "Core's emitters encode a name and degrade an authored IRI that is not one to a \
             literal, so the endpoint's Meta Turtle and contract IRIs still parse, but not \
             as written: the value is an authoring mistake, and `bind` does not validate, so \
             this is where the refusal is seen"
        }
    };
    report.findings.push(Finding::new(
        &description.id,
        None,
        Check::Validate,
        format!(
            "`Description::validate` refuses it: {}. {cost}",
            crate::report::visible(&refusal.to_string())
        ),
    ));
}

// ----- PIPELINE (static half) -------------------------------------------------

fn pipeline_static(description: &Description, report: &mut Report) {
    for spec in description.action_specs() {
        if !spec.verb.is_mutating() {
            // A read whose only intake is a pipe-less group: the engine's "required set"
            // is the required inputs plus each group's `pipe` (core's
            // `ActionSpec::pipe_slots`), and here both are empty.
            let required = spec
                .inputs
                .iter()
                .any(|i| i.required && i.source == InputSource::Argument);
            let asking: Vec<&InputGroup> = spec
                .groups
                .iter()
                .filter(|g| matches!(g.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne))
                .collect();
            if !required && !asking.is_empty() && spec.pipe_slots(&[]).is_empty() {
                report.declared.evidence.named_only.push(NamedOnly {
                    endpoint: description.id.clone(),
                    verb: spec.verb,
                    groups: asking.iter().map(|g| g.node_segment()).collect(),
                });
            }
            continue;
        }
        let by_value: Vec<&ArgSpec> = spec
            .inputs
            .iter()
            .filter(|i| i.source == InputSource::Argument)
            .collect();
        // For a `Sink`, ANY by-value input, required or not (ledger #339). The rule used
        // to fire only when one was required, which is inverted from the risk: a Sink
        // whose inputs are all optional is the one a caller most often drives with a
        // pipe alone, and `urn:log:write` shipped reading the piped `content` as an
        // undeclared fallback for its optional `msg` with this check clean. A `Delete`
        // keeps the old rule: it has no body, and optional inputs beside it (a `reason`,
        // an `author`) are metadata about the deletion, not where a pipe's value goes.
        let has_required = by_value.iter().any(|i| i.required);
        let has_content = by_value.iter().any(|i| i.name == "content");
        let fires = if spec.verb == Verb::Sink {
            !by_value.is_empty()
        } else {
            has_required
        };
        if fires && !has_content {
            let all_optional = by_value.iter().all(|i| !i.required);
            report.findings.push(Finding::new(
                &description.id,
                Some(spec.verb),
                Check::Pipeline,
                format!(
                    "declares by-value inputs ({}) but none named `content`: a pipeline's \
                     upstream value and a top-level `sink`'s body both arrive as `content`, \
                     so this action cannot receive either{}",
                    by_value
                        .iter()
                        .map(|i| format!("`{}`", i.name))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if all_optional {
                        ". Every one of them is optional, so a pipe alone is the likeliest \
                         way a caller drives it, and its value lands in an input this action \
                         does not declare. Declare `content`, or waive PIPELINE with the \
                         reason it takes no body"
                    } else {
                        ""
                    }
                ),
            ));
        }
    }
}

// ----- the invoking checks, per action ----------------------------------------

/// A `Tracer` collecting one resolution's events.
#[derive(Default)]
struct Collector(Mutex<Vec<TraceEvent>>);

impl Tracer for Collector {
    fn record(&self, event: TraceEvent) {
        self.0.lock().expect("collector lock").push(event);
    }
}

/// One [`Suite::enforced_under`] probe for the action at hand: which declaration it
/// is, and the IRI its bindings formed at this binding.
struct UnderProbe<'a> {
    index: usize,
    probe: &'a Fixture,
    target: Iri,
}

/// One [`Suite::fails_under`] probe for the action at hand: which declaration it is, the
/// class it states, and the IRI its bindings formed at this binding.
struct FailsProbe<'a> {
    index: usize,
    probe: &'a Fixture,
    class: FailureClass,
    target: Iri,
}

/// Which resolution of an action an observation of [`Check::Failures`] came from: what a
/// finding names, and what decides which check owns the observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    /// The minimal resolution under the action's caller (and CACHEABLE's second one).
    Minimal,
    /// An extra RDF face's `as=` resolution.
    Face,
    /// The pipeline probe's firing of an action declaring `content`.
    Pipeline,
    /// REQUIRED's baseline firing of a mutating action.
    Baseline,
    /// The no-grants probe ENFORCED and AUTHORITY share.
    NoGrants,
    /// An `enforced_under` probe.
    EnforcedUnder,
    /// A forced recomputation of RECOMPUTE.
    Recompute,
    /// A direct endpoint call or body collect for BODIES.
    Bodies,
    /// A BODY-INPUTS transport comparison.
    BodyInputs,
    /// A `fails_under` probe.
    FailsUnder,
}

impl Probe {
    fn described(self) -> &'static str {
        match self {
            Probe::Minimal => "the minimal resolution",
            Probe::Face => "an RDF face's `as=` resolution",
            Probe::Pipeline => "the pipeline probe's firing",
            Probe::Baseline => "REQUIRED's baseline firing",
            Probe::NoGrants => "the no-grants probe",
            Probe::EnforcedUnder => "an `enforced_under` probe",
            Probe::Recompute => "a forced recomputation",
            Probe::Bodies => "a direct BODIES invocation or collect",
            Probe::BodyInputs => "a BODY-INPUTS transport comparison",
            Probe::FailsUnder => "a `fails_under` probe",
        }
    }
}

/// One resolution of an action, as [`Check::Failures`] judges it: where it came from,
/// and the error when it failed.
struct Observed {
    probe: Probe,
    error: Option<Error>,
}

/// One action under examination: the memoized minimal resolution under root,
/// what the pipeline probe got back when it fired the action, and whether the
/// minimal resolution's failure has already been reported (once, under the first
/// check that needed it).
struct Action<'a> {
    suite: &'a Suite,
    kernel: &'a Kernel,
    id: &'a str,
    target: &'a Iri,
    spec: &'a ActionSpec,
    /// The description declares a `Sink` or `Delete`, so a write lands on `target`
    /// through this endpoint and the kernel's auto-cut fires on its thread.
    written_here: bool,
    minimal: Option<Result<(Representation, Vec<TraceEvent>), Error>>,
    fired: Option<Result<Representation, Error>>,
    /// What the action did under a capability holding no grants. ENFORCED and
    /// AUTHORITY read the same one resolution: two probes would fire a Sink twice.
    no_grants: Option<Result<Representation, Error>>,
    /// Whether CACHEABLE's second resolution repeated the first's bytes; `None` when it
    /// made none. RECOMPUTE stands down on `Some(false)`: the finding is already made.
    repeated: Option<bool>,
    /// The [`Suite::enforced_under`] probes for this action, each with its target.
    under: Vec<UnderProbe<'a>>,
    /// The probes that ran (by index), for DECLARATIONS.
    under_ran: Vec<usize>,
    /// The probes the kernel's floor refused before the endpoint ran, with the scope
    /// the caller lacked.
    under_floor: Vec<(usize, String)>,
    failure_reported: bool,
    /// Who every answer-judging probe resolves as: root, or the capability a
    /// [`Fixture::capability`] named for this action. Never the no-grants probe's.
    caller: Capability,
    /// Whether any probe actually resolved under `caller` — what DECLARATIONS reads to
    /// say a fixture's capability reached nothing.
    caller_used: bool,
    /// The one WAIVED check this action is re-run for, with its findings set aside,
    /// so the report can say a waiver suppressed nothing ([`Action::set_aside`]).
    /// `None` on the walk proper, where every check asks [`Suite::runs`].
    shadow: Option<Check>,
    /// Set when a set-aside run needed a firing of a `Sink` or a `Delete` and was
    /// refused it ([`Action::may_fire`]): that waiver could not be judged.
    shadow_blocked: bool,
    /// Set when a set-aside run of a waived check needed the minimal resolution and
    /// it failed: the check could observe nothing, so that waiver could not be judged
    /// either. (The failure itself is `RESOLVES`'s, or was reported under the needing
    /// check on the walk proper.)
    shadow_unresolved: bool,
    /// The [`Suite::fails_under`] probes for this action, each with its target.
    fails: Vec<FailsProbe<'a>>,
    /// The probes that ran (by index), for DECLARATIONS.
    fails_ran: Vec<usize>,
    /// Every resolution of this action the walk issued, answers and failures both, in
    /// order: what [`Check::Failures`] holds to the declaration.
    observed: Vec<Observed>,
}

impl<'a> Action<'a> {
    fn request(&self, args: &Args) -> Request {
        build_request(self.spec.verb, self.target, args)
    }

    /// Record one resolution this action answered, for [`Check::Failures`]. Called only
    /// where a request was really issued, never for a memoized answer.
    fn observe<T>(&mut self, probe: Probe, result: &Result<T, Error>) {
        self.observed.push(Observed {
            probe,
            error: result.as_ref().err().cloned(),
        });
    }

    fn finding(&self, check: Check, detail: impl Into<String>) -> Finding {
        Finding::new(self.id, Some(self.spec.verb), check, detail)
    }

    /// Whether `check` runs for this action — selected, and not waived for this id.
    /// During a set-aside run, exactly the one waived check being judged.
    fn runs(&self, check: Check) -> bool {
        match self.shadow {
            Some(judged) => judged == check,
            None => self.suite.runs(self.id, check),
        }
    }

    /// Whether this action may be fired now. Always on the walk proper. During a
    /// set-aside run a read may be (it is what the waived check would have fired), and
    /// a `Sink` or a `Delete` never: a waiver is no license for a write the walk would
    /// not otherwise make, so the waiver is left unjudged instead. A memoized answer
    /// is read without asking.
    fn may_fire(&mut self) -> bool {
        if self.shadow.is_some() && self.spec.verb.is_mutating() {
            self.shadow_blocked = true;
            return false;
        }
        true
    }

    /// Re-run one WAIVED check on this action with its findings set aside: how many
    /// it would have reported, or `None` when that could not be told — it needed a
    /// mutating firing, or could not observe the action (an `unprobed` of its own).
    /// Called after the walk proper is done with the action, so it reads what the
    /// walk already resolved and changes nothing the report prints.
    async fn set_aside(&mut self, check: Check) -> Option<usize> {
        let mut scratch = Report::scratch(self.suite.checks);
        let mut coverage = Coverage::default();
        let (caller_used, failure_reported) = (self.caller_used, self.failure_reported);
        self.shadow = Some(check);
        self.shadow_blocked = false;
        self.shadow_unresolved = false;
        self.failure_reported = false;
        match check {
            Check::Enforced => self.enforced(&mut scratch).await,
            Check::Authority => self.authority(&mut scratch).await,
            Check::SkolemRdf | Check::Vocabulary | Check::Datatypes => {
                self.rdf_faces(&mut scratch, &mut coverage).await
            }
            Check::Cacheable => self.cacheable(&mut scratch, &mut coverage).await,
            Check::Recompute => self.recompute(&mut scratch).await,
            Check::Bodies => self.bodies(&mut scratch).await,
            Check::BodyInputs => self.body_inputs(&mut scratch).await,
            Check::Pipeline => self.pipeline(&mut scratch).await,
            Check::Required => self.required(&mut scratch).await,
            Check::Outputs => self.outputs(&mut scratch).await,
            Check::Resolves => self.resolves(&mut scratch).await,
            Check::Failures => self.failures(&mut scratch).await,
            _ => {}
        }
        self.shadow = None;
        self.caller_used = caller_used;
        self.failure_reported = failure_reported;
        if self.shadow_blocked
            || self.shadow_unresolved
            || scratch.unprobed.iter().any(|u| u.check == check)
        {
            return None;
        }
        Some(scratch.findings.len())
    }

    /// Resolve with minimal inputs as the caller (root, or the fixture's capability),
    /// traced. Memoized: the RDF and the cache checks both need it, and a second
    /// resolution is what the cache check measures.
    async fn resolve_minimal(
        &mut self,
        again: bool,
    ) -> Result<(Representation, Vec<TraceEvent>), Error> {
        if !again {
            if let Some(memo) = &self.minimal {
                return memo.clone();
            }
        }
        if !self.may_fire() {
            return Err(set_aside_refusal());
        }
        let args = self.suite.args_for(self.id, self.spec);
        let collector = Arc::new(Collector::default());
        let tracer: Arc<dyn Tracer> = collector.clone();
        self.caller_used = true;
        let result = self
            .kernel
            .issue_traced(self.request(&args), &self.caller, tracer)
            .await
            .map(|repr| {
                let events = std::mem::take(&mut *collector.0.lock().expect("collector lock"));
                (repr, events)
            });
        self.observe(Probe::Minimal, &result);
        self.minimal = Some(result.clone());
        result
    }

    /// Report a minimal-resolution failure, once per action, under
    /// [`Check::Resolves`] when it is selected and under `needing` (the check that
    /// needed the resolution) when it is not (ledger #338). `args` are the by-value
    /// arguments of the request that failed, so the finding can say when the `content`
    /// in it was the suite's generic sample rather than a fixture's, and which value
    /// the walk had to invent for an input that declares no class.
    ///
    /// A set-aside run of a waived check that needed the resolution reports nothing and
    /// marks itself unable to judge the waiver; one of a waived `RESOLVES` reports the
    /// failure, which is what that waiver set aside.
    fn report_failure(&mut self, needing: Check, err: &Error, args: &Args, report: &mut Report) {
        match self.shadow {
            Some(Check::Resolves) => {}
            Some(_) => {
                self.shadow_unresolved = true;
                return;
            }
            None => {}
        }
        if self.failure_reported {
            return;
        }
        self.failure_reported = true;
        let label = if !self.suite.checks.contains(Check::Resolves) {
            needing
        } else if self.shadow.is_none() && self.suite.waived(self.id, Check::Resolves) {
            return;
        } else {
            Check::Resolves
        };
        let (id, verb) = (self.id, self.spec.verb);
        // The refusal's KIND beside its text: a reader telling a missing fixture
        // (`invalid-argument`) from a missing principal (`denied`) or a target this kernel
        // does not bind (`not-found`) should not have to parse prose for it.
        let refused = format!("refused as `{}`: {err}", err.kind());
        let sampled = self.sampled_content(args);
        let mut detail = match sampled {
            // The endpoint named `content` as what it refused: the sample was the cause.
            Some(sample) if matches!(err, Error::InvalidArgument { name, .. } if name == "content") =>
            {
                format!(
                    "did not resolve with the minimal inputs its ArgSpecs allow ({refused}): it \
                     refused `content`, and the `content` it was sent is the suite's generic \
                     sample `{sample}`, not a value from a Fixture. Supply one it accepts with \
                     `Fixture::new(\"{id}\", Verb::{verb:?}).arg(\"content\", …)` \
                     (`.arg_bytes(\"content\", …)` for a byte input), or opt out with a reason"
                )
            }
            // Refused some other way, which may or may not be the sample: say so, since
            // a parser's error rarely names the argument it was reading.
            Some(sample) => format!(
                "did not resolve with the minimal inputs its ArgSpecs allow ({refused}); the \
                 `content` it was sent is the suite's generic sample `{sample}`, not a value \
                 from a Fixture; if it parses `content`, that sample is the likely cause: \
                 supply `Fixture::new(\"{id}\", Verb::{verb:?}).arg(\"content\", …)` \
                 (`.arg_bytes(\"content\", …)` for a byte input) with a value it accepts, or \
                 opt out with a reason"
            ),
            // Refused under root, which holds every principal and names none: an
            // endpoint about its caller refuses exactly this, so name the caller.
            None if matches!(err, Error::Denied(_)) && self.caller.is_root() => format!(
                "did not resolve with the minimal inputs its ArgSpecs allow ({refused}); it was \
                 probed under root, which names no principal. If it answers about its \
                 caller, name one with `Fixture::new(\"{id}\", Verb::{verb:?}).capability(…)` \
                 (the scopes the caller holds plus `urn:cap:principal:<iri>`); otherwise \
                 supply inputs that work, or opt out with a reason"
            ),
            None => format!(
                "did not resolve with the minimal inputs its ArgSpecs allow ({refused}); supply a \
                 `Fixture::new(\"{id}\", Verb::{verb:?})` with inputs that work, or opt out \
                 with a reason"
            ),
        };
        if label == Check::Resolves {
            // Which check needed it: the one the failure used to be reported under.
            detail.push_str(&format!(
                ". Needed by {needing}, and by every check that reads the same resolution"
            ));
        }
        // The BECAUSE edge (ledger #338's second ask): an input with no class is one the
        // walk can only send a placeholder for, so its ARGSPECS finding is the likely
        // cause of this one, and fixing it fixes both.
        let mut invented = self.invented_untyped(args);
        if sampled.is_some() {
            // Already named above, with the sample the walk sent.
            invented.retain(|n| *n != "content");
        }
        if !invented.is_empty() {
            detail.push_str(&format!(
                ". The walk had no value to send for {}: {} no class, so it sent the \
                 placeholder `{PLACEHOLDER}` (the ARGSPECS finding on this action). A class \
                 the walk can sample, or a Fixture, is the likely fix for both",
                invented
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if invented.len() == 1 {
                    "it declares"
                } else {
                    "they declare"
                }
            ));
        }
        report.findings.push(self.finding(label, detail));
    }

    /// The inputs whose value in `args` the walk INVENTED because the input declares
    /// no class: required, by value, with no default and no `one_of`, and not supplied
    /// by a fixture. Their `ARGSPECS` finding is the likely cause of a failed
    /// resolution, so the failure names them.
    fn invented_untyped(&self, args: &Args) -> Vec<&'a str> {
        let fixture = self.suite.fixture_for(self.id, self.spec.verb);
        self.spec
            .inputs
            .iter()
            .filter(|i| {
                i.required
                    && i.source == InputSource::Argument
                    && i.default.is_none()
                    && i.one_of.is_empty()
                    && i.class.is_none()
                    && args.contains_key(&i.name)
                    && !fixture.is_some_and(|f| f.args.contains_key(&i.name))
            })
            .map(|i| i.name.as_str())
            .collect()
    }

    /// The `content` value in `args` when it is the suite's generic sample: the action
    /// declares `content` by value with no default and no `one_of` (either of which
    /// would make the value the endpoint's own choice), and no fixture supplied it.
    fn sampled_content<'b>(&self, args: &'b Args) -> Option<&'b str> {
        let sent = std::str::from_utf8(args.get("content")?).ok()?;
        let spec = self.content_spec()?;
        if spec.default.is_some() || !spec.one_of.is_empty() {
            return None;
        }
        let fixture_sets_it = self
            .suite
            .fixture_for(self.id, self.spec.verb)
            .is_some_and(|f| f.args.contains_key("content"));
        (!fixture_sets_it).then_some(sent)
    }

    /// The `content` input the action declares by value, where a pipe's value lands.
    fn content_spec(&self) -> Option<&'a ArgSpec> {
        self.spec
            .inputs
            .iter()
            .find(|i| i.name == "content" && i.source == InputSource::Argument)
    }

    /// The arguments the pipeline probe fires with: the minimal ones, plus `content`
    /// (the fixture's if it gave one, else a sample) where the action declares it.
    /// Where `content` is a member of an exactly-one or at-most-one group, the piped
    /// value REPLACES the group's named alternative, as the engine routes it: a pipe
    /// fills a group's slot only when no member is named.
    fn pipeline_args(&self, content: &ArgSpec) -> Args {
        let mut args = self.suite.args_for(self.id, self.spec);
        for group in &self.spec.groups {
            if matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtMostOne)
                && group.has("content")
            {
                for member in group.members.iter().filter(|m| *m != "content") {
                    args.remove(member);
                }
            }
        }
        args.entry("content".to_string())
            .or_insert_with(|| sample_value(content).into_bytes());
        args
    }

    // ----- ENFORCED -----------------------------------------------------------

    /// Resolve with minimal inputs under a capability holding NO grants, memoized.
    /// ENFORCED asks what a declared scope did; AUTHORITY asks what an undeclared
    /// one let through. One resolution answers both, and a mutating action is fired
    /// here exactly once however many of the two are selected.
    async fn probe_no_grants(&mut self) -> Result<Representation, Error> {
        if let Some(memo) = &self.no_grants {
            return memo.clone();
        }
        if !self.may_fire() {
            return Err(set_aside_refusal());
        }
        let args = self.suite.args_for(self.id, self.spec);
        let none = Capability::scoped(Vec::<String>::new());
        let result = self.kernel.issue(self.request(&args), &none).await;
        self.observe(Probe::NoGrants, &result);
        self.no_grants = Some(result.clone());
        result
    }

    async fn enforced(&mut self, report: &mut Report) {
        if !self.runs(Check::Enforced) {
            return;
        }
        let result = self.probe_no_grants().await;
        if !self.spec.requires.is_empty() {
            match result {
                Ok(_) => report.findings.push(self.finding(
                    Check::Enforced,
                    format!(
                        "declares requires {} but resolved under a capability holding no \
                         grants: declared is not enforced",
                        requires_list(self.spec)
                    ),
                )),
                Err(Error::Denied(_)) => {}
                Err(other) => report.findings.push(self.finding(
                    Check::Enforced,
                    format!(
                        "declares requires {}; under no grants expected a typed `Denied`, got \
                         `{other}`",
                        requires_list(self.spec)
                    ),
                )),
            }
        } else if let Err(Error::Denied(msg)) = result {
            report.findings.push(self.finding(
                Check::Enforced,
                // The axis is not known from here (ledger #362): a typed `Denied` from an
                // action declaring nothing is a CAPABILITY it does not declare, or a STATE
                // gate no capability can express (a window another resource opens). Both
                // over-offer, and their fixes differ, so the finding names both.
                format!(
                    "declares no capability but refused with `Denied` under no grants ({msg}): \
                     an undeclared gate, so the manifold over-offers it. The gate is a \
                     capability or a state, and the fixes differ: a capability it enforces is \
                     declared (`.requires(\"urn:cap:…\")` on the action); a state no capability \
                     expresses (refused until another resource opens it: an enrollment window, \
                     a session) is documented, and ENFORCED waived with that reason \
                     (`Suite::opt_out_check(id, Check::Enforced, …)`)"
                ),
            ));
        }
        for i in 0..self.under.len() {
            self.enforced_under(i, report).await;
        }
    }

    /// Fire one [`Suite::enforced_under`] probe and judge what it got: a typed
    /// `Denied` on the module's path, or a finding. Where the refusal came from is
    /// read from the trace: a `denied` note on the ROOT event is the kernel's floor
    /// (the endpoint never ran), a `failed` one is the endpoint's own refusal, and a
    /// refusal on a deeper event is a sub-request's.
    async fn enforced_under(&mut self, i: usize, report: &mut Report) {
        if !self.may_fire() {
            return;
        }
        let (index, probe, target) = {
            let u = &self.under[i];
            (u.index, u.probe, u.target.clone())
        };
        let Some(caller) = probe.capability.clone() else {
            return;
        };
        let mut args = self.suite.args_for(self.id, self.spec);
        for (name, value) in &probe.args {
            args.insert(name.clone(), value.clone());
        }
        let collector = Arc::new(Collector::default());
        let tracer: Arc<dyn Tracer> = collector.clone();
        let result = self
            .kernel
            .issue_traced(
                build_request(self.spec.verb, &target, &args),
                &caller,
                tracer,
            )
            .await;
        let events = std::mem::take(&mut *collector.0.lock().expect("collector lock"));
        self.observe(Probe::EnforcedUnder, &result);
        self.under_ran.push(index);
        let shown = Fixture {
            capability: None,
            ..probe.clone()
        }
        .to_string();
        let holding: Vec<String> = caller
            .scopes()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let (refusal, refused_by) = match &result {
            Ok(_) => (None, None),
            Err(Error::Denied(_)) => (Some("denied".to_string()), Some(refused_by(&events))),
            Err(other) => (Some(other.kind().to_string()), None),
        };
        match (&result, &refused_by) {
            (Ok(_), _) => report.findings.push(self.finding(
                Check::Enforced,
                format!(
                    "resolved under the `enforced_under` caller holding {} (`{shown}`): \
                     that caller should be refused by the module's own rule, and it was let \
                     through. Declared is not enforced for this target",
                    shown_scopes(&holding)
                ),
            )),
            (Err(Error::Denied(_)), Some(RefusedBy::Floor { lacking })) => {
                self.under_floor.push((index, lacking.clone()));
            }
            (Err(Error::Denied(_)), _) => {}
            (Err(other), _) => report.findings.push(self.finding(
                Check::Enforced,
                format!(
                    "under the `enforced_under` caller holding {} (`{shown}`) expected a typed \
                     `Denied` from the module's own rule, got `{}`: {other}. A refusal that \
                     is not typed `Denied` (an `Error::Endpoint`, a refusal wrapped into a \
                     message) reads to a caller and to an agent as a failure, not as \
                     \"you may not\"; return `Error::Denied`, and pass a sub-request's \
                     `Denied` through unchanged",
                    shown_scopes(&holding),
                    other.kind()
                ),
            )),
        }
        report.declared.evidence.enforced_under.push(EnforcedUnder {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            probe: shown,
            holding,
            refusal,
            refused_by,
        });
    }

    // ----- AUTHORITY ----------------------------------------------------------

    /// A mutation that a caller holding no authority at all performed.
    ///
    /// ENFORCED walks three cells of a 2x2 — declares and is refused (silent),
    /// declares and resolves (a finding), declares nothing and is refused (a
    /// finding: an enforced scope the manifold hides). The fourth cell, declares
    /// nothing and resolves, is correct for a `Source`: a public read is a decision
    /// a module makes. For a `Sink` or a `Delete` it is the hole this check closes.
    /// Nothing gates the write, so nothing can be withheld: a party that should
    /// read and report cannot be given read alone, because read is all there is.
    ///
    /// Evidence, not declaration. A mutating action declaring nothing that fails
    /// under no grants for an unrelated reason is recorded as unprobed — what an
    /// ungranted caller could do through it was not observed, and an unobserved
    /// probe must not read as a pass.
    async fn authority(&mut self, report: &mut Report) {
        if !self.runs(Check::Authority) || !self.spec.verb.is_mutating() {
            return;
        }
        // A declared scope is ENFORCED's half, in both directions.
        if !self.spec.requires.is_empty() {
            return;
        }
        match self.probe_no_grants().await {
            Ok(_) => report.findings.push(self.finding(
                Check::Authority,
                "declares no `requires` and mutated under a capability holding no grants: \
                 authority over this write cannot be withheld from anyone who can reach the \
                 endpoint, so no caller can be given read without also getting write. Declare \
                 the scope it should require (`.requires(\"urn:cap:…\")` on the action, \
                 enforced by the kernel), or waive it with the reason it is deliberately open",
            )),
            // Refused while declaring nothing: ENFORCED's over-offer finding, not this one.
            Err(Error::Denied(_)) => {}
            Err(err) => self.unprobed(
                report,
                Check::Authority,
                format!(
                    "declares no `requires` and did not resolve under no grants ({err}), so \
                     whether an ungranted caller can mutate through it was not observed: \
                     supply a `Fixture` with inputs that work, or say why it is open"
                ),
            ),
        }
    }

    // ----- SKOLEM-RDF + VOCABULARY + DATATYPES ---------------------------------

    async fn rdf_faces(&mut self, report: &mut Report, coverage: &mut Coverage) {
        let skolem = self.runs(Check::SkolemRdf);
        let vocab = self.runs(Check::Vocabulary);
        let datatypes = self.runs(Check::Datatypes);
        if !skolem && !vocab && !datatypes {
            return;
        }
        // Failures of the face itself (unresolvable, not RDF, unparseable) are
        // SkolemRdf's when it runs, Vocabulary's next, Datatypes' otherwise — a
        // report never loses them to the selection.
        let face_check = if skolem {
            Check::SkolemRdf
        } else if vocab {
            Check::Vocabulary
        } else {
            Check::Datatypes
        };
        let faces: Vec<String> = self
            .spec
            .outputs
            .iter()
            .filter(|o| rdf::is_rdf_face(o))
            .map(|o| rdf::bare_media_type(o))
            .collect();
        for face in faces {
            let mut args = self.suite.args_for(self.id, self.spec);
            let result = if self.spec.outputs.len() > 1 {
                // Several faces: `as=` picks this one, the universal conneg selector.
                args.entry("as".to_string())
                    .or_insert_with(|| face.clone().into_bytes());
                if !self.may_fire() {
                    return;
                }
                self.caller_used = true;
                let result = self.kernel.issue(self.request(&args), &self.caller).await;
                self.observe(Probe::Face, &result);
                result
            } else {
                self.resolve_minimal(false).await.map(|(repr, _)| repr)
            };
            let repr = match result {
                Ok(repr) => repr,
                Err(err) => {
                    self.report_failure(face_check, &err, &args, report);
                    continue;
                }
            };
            let got = rdf::bare_media_type(&repr.repr_type.media_type);
            if !rdf::is_rdf_face(&got) {
                report.findings.push(self.finding(
                    face_check,
                    format!(
                        "asked for the `{face}` face and got `{}`: a declared output the action \
                         does not serve",
                        repr.repr_type
                    ),
                ));
                continue;
            }
            let triples = match rdf::parse(&got, &repr.bytes) {
                Ok(triples) => triples,
                Err(e) => {
                    report
                        .findings
                        .push(self.finding(face_check, format!("the `{face}` face {e}")));
                    continue;
                }
            };
            // Positive evidence: this face was reached, served and parsed. A clean
            // report is otherwise indistinguishable from a never-probed one, and the
            // triple count says whether "clean" meant anything — a face that parses
            // to zero triples passes both RDF checks vacuously.
            report.probed.push(Probed {
                endpoint: self.id.to_string(),
                verb: self.spec.verb,
                face: face.clone(),
                triples: triples.len(),
                bytes: repr.bytes.len(),
            });
            coverage.faces_probed = true;
            if skolem {
                let blank = rdf::blank_nodes(&triples);
                if !blank.is_empty() {
                    let shown: Vec<&str> = blank.iter().take(3).map(String::as_str).collect();
                    report.findings.push(self.finding(
                        Check::SkolemRdf,
                        format!(
                            "the `{face}` face has {} blank node(s) ({}{}): skolemize — mint a \
                             stable IRI per node (`urn:ikigai:endpoint:{{id}}:…`, \
                             `urn:event:{{uid}}`), never a counter",
                            blank.len(),
                            shown.join(", "),
                            if blank.len() > 3 { ", …" } else { "" }
                        ),
                    ));
                }
            }
            if vocab {
                for term in rdf::terms(&triples) {
                    if rdf::is_defined(&term, &[]) {
                        continue;
                    }
                    // A vocabulary the module ships answers for its namespace
                    // exactly, ahead of any registration that would waive it.
                    let shipped = self
                        .suite
                        .vocabularies
                        .iter()
                        .enumerate()
                        .find_map(|(i, v)| {
                            let v = v.as_ref().ok()?;
                            Some((i, v, v.covering(&term)?))
                        });
                    if let Some((i, v, ns)) = shipped {
                        coverage.vocabularies_used.insert(i);
                        if !v.defines(&term) {
                            report.findings.push(self.finding(
                                Check::Vocabulary,
                                format!(
                                    "the `{face}` face uses `{term}`, which is under `{ns}`, the \
                                     namespace of the vocabulary the module ships \
                                     (`Suite::vocabulary`), and that graph does not define it: \
                                     add it to the vocabulary, or fix the term"
                                ),
                            ));
                        }
                        continue;
                    }
                    // WHICH registered namespace accounted for it, not merely
                    // whether one did: a namespace that accounts for nothing is a
                    // standing waiver nobody needs, and DECLARATIONS says so.
                    // Asking `is_defined` one namespace at a time keeps the `ik:`
                    // rule intact — an undefined `ik:` term is covered by no
                    // registration.
                    let covering = self
                        .suite
                        .namespaces
                        .iter()
                        .find(|ns| rdf::is_defined(&term, std::slice::from_ref(*ns)));
                    match covering {
                        Some(ns) => {
                            coverage.namespaces_used.insert(ns.clone());
                        }
                        None => {
                            report.findings.push(self.finding(
                                Check::Vocabulary,
                                format!(
                                    "the `{face}` face uses `{term}`, which ikigai-vocab does not \
                                 define and no well-known or registered namespace covers: an \
                                 invented term with no definition (define it in the vocabulary, \
                                 or `Suite::vocabulary` the graph the module ships, or `Suite::namespace` one it \
                                 documents as its own)"
                                ),
                            ));
                        }
                    }
                }
            }
            if datatypes {
                for m in rdf::datatype_mismatches(&triples) {
                    let short = |iri: &str| {
                        iri.replace(ikigai_vocab::NS, "ik:")
                            .replace("http://www.w3.org/2001/XMLSchema#", "xsd:")
                    };
                    let shorthand = rdf::TURTLE_SHORTHAND.contains(&m.datatype.as_str());
                    report.findings.push(self.finding(
                        Check::Datatypes,
                        format!(
                            "the `{face}` face writes `{}` as `{}`, but its rdfs:range is `{}`, \
                             the datatype ikigai-vocab's JSON-LD context coerces it to: \
                             value-equal, not term-equal, so the two faces of one graph diff \
                             where nothing differs. Tag it `^^{}`{}",
                            short(&m.predicate),
                            short(&m.datatype),
                            short(&m.range),
                            short(&m.range),
                            if shorthand {
                                " (an untagged Turtle literal can only be xsd:integer, \
                                 xsd:decimal, xsd:double, xsd:boolean or xsd:string)"
                            } else {
                                ""
                            }
                        ),
                    ));
                }
            }
        }
    }

    /// Record what this action served, whatever the media type — the evidence a
    /// module with no graph face otherwise has none of. Reads the representation
    /// the checks above already obtained; resolves nothing of its own, so an action
    /// no check invoked stays unprobed rather than being fired for a report line.
    ///
    /// A face the RDF checks already recorded (same id, verb and media type) is not
    /// recorded twice: there the triple count is the stronger statement.
    fn record_probe(&self, report: &mut Report) {
        // The same representation OUTPUTS reads: the pipeline probe's firing for a
        // mutating verb, the minimal resolution otherwise.
        let served = match (self.spec.verb.is_mutating(), self.fired.as_ref()) {
            (true, Some(Ok(repr))) => Some(repr),
            (true, Some(Err(_))) => None,
            _ => match self.minimal.as_ref() {
                Some(Ok((repr, _))) => Some(repr),
                _ => None,
            },
        };
        let Some(repr) = served else {
            return;
        };
        self.record_answer(repr, report);
    }

    fn record_answer(&self, repr: &Representation, report: &mut Report) {
        let face = rdf::bare_media_type(&repr.repr_type.media_type);
        if report
            .probed
            .iter()
            .any(|p| p.endpoint == self.id && p.verb == self.spec.verb && p.face == face)
        {
            return;
        }
        report.probed.push(Probed {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            face,
            triples: 0,
            bytes: repr.bytes.len(),
        });
    }

    // ----- CACHEABLE ----------------------------------------------------------

    async fn cacheable(&mut self, report: &mut Report, coverage: &mut Coverage) {
        if !self.runs(Check::Cacheable) || !self.spec.verb.is_cacheable() {
            return;
        }
        // Past the guard is exactly where `live` and `cacheable` are read, so it is
        // exactly where DECLARATIONS may stop calling them inert.
        coverage.cacheable_ran.insert(self.id.to_string());
        let declared_cacheable = self.suite.cacheable.iter().any(|c| c == self.id);
        let declared_live = self.suite.live.iter().any(|l| l == self.id);
        if declared_cacheable && declared_live {
            report.findings.push(self.finding(
                Check::Cacheable,
                "declared both cacheable (`Suite::cacheable`) and live (`Suite::live`): \
                 the two are opposite promises about the same endpoint, so one of them \
                 is certainly false — keep the one the module means",
            ));
        }
        let (first, first_events) = match self.resolve_minimal(false).await {
            Ok(first) => first,
            Err(err) => {
                // Nothing can be said about `pure` here: the endpoint might well be
                // cacheable once its inputs are right. DECLARATIONS must not add an
                // "inert declaration" line on top of a resolution failure.
                coverage.cacheable_unresolved.insert(self.id.to_string());
                let args = self.suite.args_for(self.id, self.spec);
                self.report_failure(Check::Cacheable, &err, &args, report);
                return;
            }
        };
        if first.expiry != Expiry::Always {
            // The result IS cacheable, so the golden-thread rule applies and
            // `Suite::pure` — which exempts an endpoint from it — is in play.
            // Recorded here rather than at the rule itself, so neither a `live`
            // violation below nor a failed second resolution makes a declaration
            // that WAS consulted read as inert.
            coverage.pure_consulted.insert(self.id.to_string());
            if self.suite.pure.iter().any(|p| p == self.id) {
                self.pure_carries_no_foreign_thread(&first, &first_events, report)
                    .await;
            }
        }
        if declared_live && first.expiry != Expiry::Always {
            report.findings.push(self.finding(
                Check::Cacheable,
                format!(
                    "declared live (`Suite::live`) but the kernel returned it cacheable \
                     ({}): a resource that must be read fresh is now served from the cache \
                     until something cuts a thread. Effective expiry propagates, so this is \
                     usually a dependency that became cacheable, or a `.cacheable()` added \
                     to a result that reads live state",
                    match first.expiry {
                        Expiry::Never => "`Expiry::Never` — permanently".to_string(),
                        other => format!("`{other:?}`"),
                    }
                ),
            ));
            return;
        }
        if first.expiry == Expiry::Always {
            // The kernel hands back the EFFECTIVE expiry, so this is either "never
            // marked cacheable" (the recipe's "when in doubt, don't") or "marked
            // cacheable over a volatile dependency" — indistinguishable from here.
            // Only a declaration separates them — and `Suite::live` is the other
            // half: an endpoint declared live and returned `Always` is the outcome
            // its module promised, so the silence here is a verdict, not a gap.
            if declared_cacheable {
                report.findings.push(self.finding(
                    Check::Cacheable,
                    "declared cacheable (`Suite::cacheable`) but the kernel returned it \
                     uncacheable: the effective expiry is the least cacheable part's, so a \
                     dependency resolved on the way is volatile — every read recomputes",
                ));
            }
            return;
        }
        let (second, events) = match self.resolve_minimal(true).await {
            Ok(second) => second,
            Err(err) => {
                report.findings.push(self.finding(
                    Check::Cacheable,
                    format!("cacheable, but the second identical resolution failed: {err}"),
                ));
                return;
            }
        };
        let repeated = second.bytes == first.bytes;
        if !repeated {
            report.findings.push(self.finding(
                Check::Cacheable,
                "marked cacheable but two identical resolutions returned different bytes: \
                 not a function of its inputs",
            ));
        }
        // Two witnesses: the trace's own `cache_hit` on the root event, or the
        // kernel's read-only probe saying the request is now cached. The second is
        // needed because the kernel's intrinsic `urn:kernel:*` path stores a
        // cacheable result but records no trace event at all — hit or miss.
        let traced_hit = events
            .iter()
            .filter(|e| e.parent.is_none())
            .any(|e| e.cache_hit);
        let args = self.suite.args_for(self.id, self.spec);
        let probed = self.kernel.is_cached(&self.request(&args), &self.caller);
        if !traced_hit && !probed {
            report.findings.push(self.finding(
                Check::Cacheable,
                "marked cacheable but the second resolution recomputed (the trace records no \
                 cache hit): a dependency is uncacheable — effective expiry is the least \
                 cacheable part's — or the result carries `Expiry::At` on a clockless kernel",
            ));
        }
        // Purity is "no thread but its own name", not an empty set. Since core 0.1.73
        // the kernel hangs every cacheable `Source`/`Exists` answer on the thread
        // named for its own canonical target (formalism R4.4, ledger #512 hole A), so
        // `threads().is_empty()` can never be observed and a check spelled that way
        // fires for nothing (ledger #549). The own-name thread is cut only by a write
        // through that same name, so it answers "what cuts this?" exactly when the
        // endpoint takes writes there; otherwise the result still has nothing to cut
        // it. A thread the module declared that happens to equal its own name is
        // indistinguishable from the kernel's, and reads the same way.
        //
        // Only `Expiry::Never` is unbounded. `Always` returned above; an `At` deadline
        // is a bound in TIME — the kernel stops serving the answer when the deadline
        // passes, cut or not — so "served forever" is false of it and a clock reading
        // (a `tz-now` cacheable to the minute) owes no `pure` declaration (ledger
        // #1000). Every `At` counts, however distant: the suite judges the kind of
        // bound, not its length. On a clockless kernel an `At` answer is never cached
        // at all, which the recomputation finding above already names.
        //
        // "Its own name" is the CANONICAL target, which is not always the name the
        // walk asked for: a space that lists a logical name and reports a rewrite
        // (`Resolved::canonical`) has the kernel hang the read on the BACKING name.
        // Compared against the listed name alone, that thread read as foreign and the
        // rule could not fire for any endpoint behind such a space (ledger #596). The
        // root trace event names the canonical target; the requested name stays in
        // the set for the intrinsic `urn:kernel:*` path, which records no event.
        let mut own: BTreeSet<Thread> = BTreeSet::from([Thread::from(self.target.as_str())]);
        own.extend(
            first_events
                .iter()
                .chain(events.iter())
                .filter(|e| e.parent.is_none())
                .map(|e| Thread::from(e.target.as_str())),
        );
        let foreign = first.threads().iter().any(|t| !own.contains(t));
        if first.expiry == Expiry::Never
            && !foreign
            && !self.written_here
            && !self.suite.pure.iter().any(|p| p == self.id)
        {
            report.findings.push(self.finding(
                Check::Cacheable,
                "cacheable with no golden thread but its own name: it will be served forever \
                 with nothing to cut it. The kernel hangs every cacheable read on the name it \
                 was read through, and only a write through that same name cuts it — this \
                 endpoint declares no `Sink` or `Delete`. `Suite::pure(id)` if it is a pure \
                 function of its inputs; otherwise `depends_on` the thread of the state it reads",
            ));
        }
        // RECOMPUTE reads this: when the second resolution came back different, the
        // finding is made here and a forced recomputation would only repeat it.
        self.repeated = Some(repeated);
    }

    // ----- BODY-INPUTS --------------------------------------------------------

    async fn body_inputs(&mut self, report: &mut Report) {
        if !self.runs(Check::BodyInputs) {
            return;
        }
        for input in self.spec.inputs.iter().filter(|i| i.reads_body) {
            if input.source != InputSource::Argument {
                self.unprobed(
                    report,
                    Check::BodyInputs,
                    format!(
                        "`{}` declares reads_body on a binding, not an argument",
                        input.name
                    ),
                );
                continue;
            }
            if !self.may_fire() {
                return;
            }
            let mut args = self.suite.args_for(self.id, self.spec);
            let bytes = args
                .entry(input.name.clone())
                .or_insert_with(|| sample_value(input).into_bytes())
                .clone();
            complete_relations(self.spec, &mut args);
            // An anonymous, private corridor provides just the sample resource. All
            // variants share this scope and retain the host kernel and caller, so
            // module subrequests and host policies continue to apply. A fresh scope
            // also prevents a previous suite run from hiding a transport defect.
            let mut reference_name = format!(
                "urn:ikigai:conformance:body-input:{}",
                ikigai_core::ContentId::of(&bytes)
            );
            while reference_name == self.target.as_str() {
                reference_name.push_str(":input");
            }
            let reference = Iri::parse(&reference_name).expect("generated sample IRI");
            let sample = bytes.clone();
            let space = ikigai_core::EndpointSpace::new().bind(
                ikigai_core::Exact::new(reference_name),
                ikigai_core::FnEndpoint::new("body-input-sample", move |_| {
                    Ok(Representation::new(
                        ikigai_core::ReprType::new("application/octet-stream"),
                        sample.clone(),
                    )
                    .cacheable())
                }),
            );
            let scope = Scope::empty().with(Arc::new(space));
            self.caller_used = true;
            let baseline = self
                .kernel
                .issue_with(
                    self.request(&args),
                    &self.caller,
                    ikigai_core::BodyIssue::new().in_scope(scope.clone()),
                )
                .await;
            self.observe(Probe::BodyInputs, &baseline);
            let baseline = match baseline {
                Ok(answer) => answer,
                Err(err) => {
                    self.report_failure(Check::BodyInputs, &err, &args, report);
                    self.unprobed(
                        report,
                        Check::BodyInputs,
                        format!(
                            "`{}` reads_body: inline baseline failed; no transports compared",
                            input.name
                        ),
                    );
                    continue;
                }
            };
            self.record_answer(&baseline, report);
            let mut request = self.request(&args);
            request
                .args
                .insert(input.name.clone(), ArgRef::Reference(reference));
            let referenced = self
                .kernel
                .issue_with(
                    request,
                    &self.caller,
                    ikigai_core::BodyIssue::new().in_scope(scope.clone()),
                )
                .await;
            self.observe(Probe::BodyInputs, &referenced);
            self.compare_body_input(&input.name, "reference", &baseline, referenced, report);

            if let Some(store) = &self.suite.body_input_store {
                match store.put(bytes.clone()) {
                    Ok(held) => {
                        let mut request = self.request(&args);
                        request
                            .args
                            .insert(input.name.clone(), ArgRef::Content(held.id()));
                        let stored = self
                            .kernel
                            .issue_with(
                                request,
                                &self.caller,
                                ikigai_core::BodyIssue::new().in_scope(scope.clone()),
                            )
                            .await;
                        self.observe(Probe::BodyInputs, &stored);
                        self.compare_body_input(
                            &input.name,
                            "stored content id",
                            &baseline,
                            stored,
                            report,
                        );
                        drop(held);
                    }
                    Err(err) => self.unprobed(
                        report,
                        Check::BodyInputs,
                        format!("`{}` reads_body: could not store sample: {err}", input.name),
                    ),
                }
            } else {
                self.unprobed(report, Check::BodyInputs,
                    format!("`{}` reads_body: stored content id was not compared; supply the kernel's shared store to Suite::body_input_store", input.name));
            }

            let mut request = self.request(&args);
            request.args.remove(&input.name);
            // Multiple chunks exercise actual stream consumption. Body::once has no
            // content validator: the kernel must pass a live, one-shot attachment.
            let split = bytes.len() / 2;
            let body = ikigai_core::Body::once(futures::stream::iter([
                Ok(ikigai_core::Bytes::copy_from_slice(&bytes[..split])),
                Ok(ikigai_core::Bytes::copy_from_slice(&bytes[split..])),
            ]));
            let attached = self
                .kernel
                .issue_with(
                    request,
                    &self.caller,
                    ikigai_core::BodyIssue::new()
                        .in_scope(scope)
                        .attach(&input.name, body),
                )
                .await;
            self.observe(Probe::BodyInputs, &attached);
            self.compare_body_input(
                &input.name,
                "one-shot attached body",
                &baseline,
                attached,
                report,
            );
        }
    }

    fn compare_body_input(
        &self,
        name: &str,
        transport: &str,
        baseline: &Representation,
        answer: Result<Representation, Error>,
        report: &mut Report,
    ) {
        let detail = match answer {
            Err(err) => format!("`{name}` declares reads_body: inline succeeded but {transport} failed ({}: {err})", err.kind()),
            Ok(answer) if answer.bytes != baseline.bytes || answer.repr_type != baseline.repr_type =>
                format!("`{name}` declares reads_body: {transport} differs from inline ({} versus {} bytes; media types `{}` versus `{}`)",
                    answer.bytes.len(), baseline.bytes.len(), answer.repr_type, baseline.repr_type),
            Ok(_) => {
                report.declared.evidence.body_inputs.push(crate::report::BodyInputComparison {
                    endpoint: self.id.to_string(), verb: self.spec.verb,
                    input: name.to_string(), transport: transport.to_string(),
                });
                return;
            }
        };
        report
            .findings
            .push(self.finding(Check::BodyInputs, detail));
    }

    // ----- BODIES -------------------------------------------------------------

    async fn bodies(&mut self, report: &mut Report) {
        if !self.runs(Check::Bodies) || self.spec.verb != Verb::Source {
            return;
        }
        let Some(space) = &self.suite.body_space else {
            self.unprobed(report, Check::Bodies,
                "the module space is unavailable: pass the kernel's shared root to Suite::body_space; kernel calls both dispatch invoke_body and cannot prove endpoint agreement");
            return;
        };
        let args = self.suite.args_for(self.id, self.spec);
        let (baseline, baseline_events) = match self.resolve_minimal(false).await {
            Ok(answer) => answer,
            Err(err) => {
                self.report_failure(Check::Bodies, &err, &args, report);
                return;
            }
        };
        if self.suite.live.iter().any(|id| id == self.id)
            || baseline.expiry != Expiry::Never
            || self.repeated == Some(false)
        {
            self.unprobed(report, Check::Bodies,
                "uncacheable, time-varying, or volatile answer: independent calls cannot establish method agreement (Suite::live, expiry or CACHEABLE's byte comparison)");
            return;
        }
        let request = self.request(&args);
        let Resolution::Hit(resolved) = space.0.resolve(&request, &Scope::empty()) else {
            self.unprobed(
                report,
                Check::Bodies,
                "the request did not resolve to an endpoint in Suite::body_space",
            );
            return;
        };
        // Direct calls bypass the kernel's default injection. Give both methods the
        // same declared defaults a kernel invocation would receive, without replacing
        // a fixture argument or a captured binding.
        let mut direct_request = request.clone();
        let mut bindings = resolved.bindings;
        for input in &self.spec.inputs {
            let Some(default) = &input.default else {
                continue;
            };
            match input.source {
                InputSource::Argument => {
                    direct_request
                        .args
                        .entry(input.name.clone())
                        .or_insert_with(|| ArgRef::Inline(default.as_bytes().to_vec()));
                }
                InputSource::Binding if bindings.get(&input.name).is_none() => {
                    bindings.insert(input.name.clone(), default.clone());
                }
                _ => {}
            }
        }
        // Separate invocations and pools: neither call can spend the other's budget.
        const MAX_BYTES: usize = 64 * 1024 * 1024;
        let inv = Invocation::with_issuer(&direct_request, &bindings, &self.caller, self.kernel);
        let buffered_inv = inv.with_memory_limit(MAX_BYTES as u64);
        let buffered = resolved.endpoint.invoke(&buffered_inv).await;
        let inv = Invocation::with_issuer(&direct_request, &bindings, &self.caller, self.kernel);
        let body_inv = inv.with_memory_limit(MAX_BYTES as u64);
        let streamed = match resolved.endpoint.invoke_body(&body_inv).await {
            Ok(body) => body.collect(MAX_BYTES).await,
            Err(err) => Err(err),
        };
        self.observe(Probe::Bodies, &buffered);
        self.observe(Probe::Bodies, &streamed);
        let (buffered, streamed) = match (buffered, streamed) {
            (Ok(buffered), Ok(streamed)) => (buffered, streamed),
            (buffered, streamed) => {
                let error = |answer: &Result<Representation, Error>| match answer {
                    Ok(_) => "Ok".to_string(),
                    Err(err) => format!("{}: {err}", err.kind()),
                };
                report.findings.push(self.finding(
                    Check::Bodies,
                    format!(
                        "endpoint `{}`: invoke returned {}; invoke_body/collect returned {}",
                        resolved.endpoint.name(),
                        error(&buffered),
                        error(&streamed)
                    ),
                ));
                return;
            }
        };
        if buffered.bytes != streamed.bytes || buffered.repr_type != streamed.repr_type {
            let difference = if buffered.bytes == streamed.bytes {
                "agree in bytes but disagree in media type".to_string()
            } else {
                let offset = buffered
                    .bytes
                    .iter()
                    .zip(&streamed.bytes)
                    .position(|(a, b)| a != b)
                    .unwrap_or(buffered.bytes.len().min(streamed.bytes.len()));
                format!("disagree at offset {offset}")
            };
            report.findings.push(self.finding(Check::Bodies, format!(
                "endpoint `{}`: invoke and invoke_body {difference} ({} versus {} bytes; media types `{}` versus `{}`)",
                resolved.endpoint.name(), buffered.bytes.len(), streamed.bytes.len(),
                buffered.repr_type, streamed.repr_type)));
        }
        if streamed.bytes.len() < 2 {
            return;
        }
        let bound = streamed.bytes.len() - 1;
        // A memory grant alone never lowers the host's base. Carry a run ceiling too,
        // using the original kernel so composites retain their issuer and host setup.
        let mut grants: Vec<String> = self
            .caller
            .scopes()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_else(|| self.spec.requires.clone());
        grants.retain(|g| !g.starts_with("urn:cap:stream:memory:"));
        grants.push(ikigai_core::cap_stream_memory(bound as u64));
        let capability = Capability::scoped(grants);
        // A fixture may already carry exactly this grant, or a prior probe may have
        // populated this capability partition. The bounded invocation must reach the
        // endpoint, rather than collect a full answer computed under a larger run.
        if self.kernel.is_cached(&request, &capability) {
            self.kernel.cut(self.target.as_str());
            for event in baseline_events.iter().filter(|e| e.parent.is_none()) {
                self.kernel.cut(event.target.as_str());
            }
        }
        let limited = self
            .kernel
            .issue_body_with_context(
                request,
                &capability,
                IssueContext::new().with_memory_limit(bound as u64),
            )
            .await;
        let collected = match limited {
            Ok(body) => body.collect(bound).await,
            Err(err) => Err(err),
        };
        match collected {
            Err(Error::OverBudget { .. }) => {}
            Err(err) => report.findings.push(self.finding(Check::Bodies, format!(
                "bounded invoke_body/collect expected OverBudget at {bound} bytes, got {}: {err}", err.kind()))),
            Ok(short) => report.findings.push(self.finding(Check::Bodies, format!(
                "bounded invoke_body truncated: returned {} of {} bytes under a {bound}-byte budget instead of OverBudget",
                short.bytes.len(), streamed.bytes.len()))),
        }
    }

    // ----- RECOMPUTE ----------------------------------------------------------

    /// The forced-recomputation witness's entry (ledger #330): an `Expiry::Never`
    /// answer the cache holds, recomputed after a cut. Late in the walk, after
    /// CACHEABLE, because it cuts: CACHEABLE reads the cache as its two resolutions
    /// left it.
    async fn recompute(&mut self, report: &mut Report) {
        if !self.runs(Check::Recompute) || !self.spec.verb.is_cacheable() {
            return;
        }
        if self.repeated == Some(false) {
            return;
        }
        let (first, first_events) = match self.resolve_minimal(false).await {
            Ok(first) => first,
            Err(err) => {
                let args = self.suite.args_for(self.id, self.spec);
                self.report_failure(Check::Recompute, &err, &args, report);
                return;
            }
        };
        if first.expiry != Expiry::Never {
            return;
        }
        let args = self.suite.args_for(self.id, self.spec);
        if !self.kernel.is_cached(&self.request(&args), &self.caller) {
            // Never stored (a volatile dependency, a cache policy): every read already
            // recomputes, and CACHEABLE names why.
            return;
        }
        // Its own name, listed or canonical (the root trace event names the canonical
        // target), and the name of everything it read.
        let mut own: BTreeSet<Thread> = BTreeSet::from([Thread::from(self.target.as_str())]);
        own.extend(
            first_events
                .iter()
                .filter(|e| e.parent.is_none())
                .map(|e| Thread::from(e.target.as_str())),
        );
        let read: BTreeSet<Thread> = first_events
            .iter()
            .map(|e| Thread::from(e.target.as_str()))
            .chain(own.iter().cloned())
            .collect();
        self.forced_recomputation(&first, &own, &read, report).await;
    }

    /// The forced-recomputation witness (ledger #330): cut every thread of the first
    /// result, and resolve again.
    ///
    /// The cacheable-twice probe cannot see a wrong `.cacheable()` on its own: its
    /// second resolution IS the cache hit, so "byte-identical" compares the first
    /// computation with a copy of itself. `ikigai-encrypt` shipped `.cacheable()` on
    /// age ciphertext (a fresh ephemeral key per call, so two sealings differ) and the
    /// walk reported nothing. Cutting every thread the answer carries, its own name
    /// included (the kernel hangs every cacheable read on it, so a pure result has a
    /// thread to cut too), and the name of everything it read, takes the answer and
    /// what it was built from out of the cache: the third resolution is a genuinely
    /// independent computation, and its bytes must be the first one's. It is done
    /// [`RECOMPUTATIONS`] times, every computation held to the first, because a single
    /// one agrees with the first by chance whenever the defect is an order over few
    /// entries.
    ///
    /// Only an `Expiry::Never` answer is held to it. An `Expiry::At` answer is a
    /// function of its inputs AND the time (a clock reading cacheable to the minute),
    /// so a recomputation may differ by design; the cache serves the first one until
    /// the deadline, which is the bound the endpoint chose.
    ///
    /// Then the narrower half, in the same machinery: every thread the answer carries
    /// that is neither its own name nor the name of something it read is one some
    /// endpoint DECLARED (`depends_on`): each is cut alone, and the recomputation must
    /// repeat the first computation's bytes too. One extra resolution each: that half
    /// asks whether the declared thread's cut REACHES the answer, and whether a
    /// computation repeats the first is already settled above. An answer
    /// still cached right after a cut is not observed ([`Action::still_cached`]).
    ///
    /// Cutting is not a write: nothing is fired, and the module's state is untouched.
    /// It does wake a cut listener a host registered on the kernel under test, as any
    /// write through the cut thread would.
    async fn forced_recomputation(
        &mut self,
        first: &Representation,
        own: &BTreeSet<Thread>,
        read: &BTreeSet<Thread>,
        report: &mut Report,
    ) {
        let args = self.suite.args_for(self.id, self.spec);
        let request = self.request(&args);
        let every: BTreeSet<Thread> = first.threads().union(read).cloned().collect();
        // Several independent computations, each held to the first: one agrees with
        // the first by chance as often as a small map's order does ([`RECOMPUTATIONS`]).
        for nth in 1..=RECOMPUTATIONS {
            for thread in &every {
                self.kernel.cut(thread.clone());
            }
            if self.kernel.is_cached(&request, &self.caller) {
                self.still_cached(&every, report);
                return;
            }
            if !self
                .recomputes_identically(&request, first, &every, Some(nth), report)
                .await
            {
                return;
            }
        }
        // The narrower half: each DECLARED thread alone.
        let declared: Vec<&Thread> = first
            .threads()
            .iter()
            .filter(|t| !read.contains(*t) && !own.contains(*t))
            .collect();
        let n_declared = declared.len();
        for thread in declared {
            if !self.kernel.is_cached(&request, &self.caller) {
                // The recomputation was not stored: there is nothing left to cut.
                return;
            }
            self.kernel.cut(thread.clone());
            let only = BTreeSet::from([thread.clone()]);
            if self.kernel.is_cached(&request, &self.caller) {
                self.still_cached(&only, report);
                return;
            }
            if !self
                .recomputes_identically(&request, first, &only, None, report)
                .await
            {
                return;
            }
        }
        report.declared.evidence.recomputed.push(Recomputed {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            recomputations: RECOMPUTATIONS,
            cut: every.len(),
            declared: n_declared,
        });
    }

    /// The answer was in the cache right after a cut of `cut`. With the kernel this
    /// crate admits that is not the module's doing: core files every cacheable answer
    /// under the threads it carries, and a cut drops it. What can refill it between the
    /// cut and the look is a cut listener the host registered on the kernel under test,
    /// which may re-issue an invalidated read on another thread (the recompute-before-
    /// the-first-reader pattern). So it is reported as not observed, never as a finding:
    /// the witness cannot tell a refill from a cut that did not land.
    fn still_cached(&self, cut: &BTreeSet<Thread>, report: &mut Report) {
        self.unprobed(
            report,
            Check::Recompute,
            format!(
                "the answer was still cached right after a cut of {}: a cut listener on this \
                 kernel may have refilled it, or the cut did not reach it, so whether a second \
                 computation repeats the first was not observed",
                shown_threads(cut)
            ),
        );
    }

    /// Resolve `request` again after `cut` was cut, and hold the bytes to `first`'s.
    /// `nth` is which of the [`RECOMPUTATIONS`] after the full cut this is (`None` for
    /// a declared thread's). `false` when it reported something (or could not
    /// observe), so the caller stops.
    async fn recomputes_identically(
        &mut self,
        request: &Request,
        first: &Representation,
        cut: &BTreeSet<Thread>,
        nth: Option<usize>,
        report: &mut Report,
    ) -> bool {
        let result = self.kernel.issue(request.clone(), &self.caller).await;
        self.observe(Probe::Recompute, &result);
        match result {
            Ok(again) if again.bytes == first.bytes => true,
            Ok(again) => {
                // Which computation differed: a late one says the earlier ones agreed
                // with the first by chance, the signature of an order over few entries.
                let which = match nth {
                    Some(1) | None => String::new(),
                    Some(n) => format!(
                        " (recomputation {n} of {RECOMPUTATIONS}; the {} before it repeated \
                         the first by chance)",
                        n - 1
                    ),
                };
                report.findings.push(self.finding(
                    Check::Recompute,
                    format!(
                        "marked cacheable but a forced recomputation returned different bytes: \
                         not a function of its inputs. The second resolution was the cache hit, \
                         so it could only repeat the first; after a cut of {} the kernel \
                         computed it again{which} and got {} byte(s) where the first \
                         computation served {}. A result that differs per computation (a fresh \
                         nonce or key, a timestamp, a random id, `HashMap` order) is not \
                         cacheable: every reader is served whichever computation the cache kept",
                        shown_threads(cut),
                        again.bytes.len(),
                        first.bytes.len()
                    ),
                ));
                false
            }
            Err(err) => {
                self.unprobed(
                    report,
                    Check::Recompute,
                    format!(
                        "cacheable, but after a cut of {} its recomputation failed ({err}), so \
                         whether a second computation repeats the first was not observed",
                        shown_threads(cut)
                    ),
                );
                false
            }
        }
    }

    /// Hold a `Suite::pure` declaration to what it claims (ledger #1508). `pure`
    /// exempts a cacheable result from the no-thread-but-its-own-name finding, and
    /// before this the suite took the claim on trust: an endpoint declared pure whose
    /// answer hung from some piece of state's thread (`depends_on("urn:example:db")`)
    /// passed clean, because a FOREIGN thread is exactly what silences the rule the
    /// declaration exempts. A pure function hangs from nothing but what it read, so
    /// every thread on its answer must be accounted for by the resolution itself:
    ///
    /// - its own name, listed or canonical (the kernel hangs every cacheable read on
    ///   it);
    /// - the name of every resource it read on the way, at any depth (a sub-request,
    ///   a by-reference argument): the kernel folds each one's own-name thread into
    ///   the composite's set;
    /// - every thread a resource it read carries, read back from the cache, so a
    ///   pure transform over a STATEFUL input (a query over a graph that hangs from
    ///   its store) is not blamed for its input's thread. A kernel face counts like
    ///   any other resource: a catalog rendered as RDF carries the catalog's
    ///   `urn:kernel:bindings` thread, and reads as a function of what is bound.
    ///
    /// What is left is a thread the endpoint named itself for state no request in
    /// this resolution was made to: the declaration is false.
    ///
    /// Reads the trace the first minimal resolution already recorded. The only reads
    /// of its own are of the resources that resolution read directly, and only when
    /// the cache already holds them under the same caller, so each is a cache hit
    /// that computes nothing; and only when a thread is left over at all. When a
    /// thread cannot be attributed — the first resolution was itself a cache hit (no
    /// children in its trace), or a resource it read is not in the cache under a
    /// plain `Source` of its name (it was read with arguments, or under a narrower
    /// capability) — nothing is said: the check refuses only what it can show.
    async fn pure_carries_no_foreign_thread(
        &self,
        first: &Representation,
        first_events: &[TraceEvent],
        report: &mut Report,
    ) {
        let mut accounted: BTreeSet<Thread> = BTreeSet::from([Thread::from(self.target.as_str())]);
        accounted.extend(first_events.iter().map(|e| Thread::from(e.target.as_str())));
        let left = |accounted: &BTreeSet<Thread>| -> Vec<String> {
            first
                .threads()
                .iter()
                .filter(|t| !accounted.contains(*t))
                .map(|t| format!("`{}`", t.as_str()))
                .collect()
        };
        if left(&accounted).is_empty() {
            return;
        }
        let root = first_events.iter().find(|e| e.parent.is_none());
        if root.is_some_and(|r| r.cache_hit) {
            return;
        }
        let children: BTreeSet<&str> = match root {
            Some(r) => first_events
                .iter()
                .filter(|e| e.parent == Some(r.span))
                .map(|e| e.target.as_str())
                .collect(),
            None => BTreeSet::new(),
        };
        for child in children {
            let Ok(iri) = Iri::parse(child) else {
                return;
            };
            let read = Request::new(Verb::Source, iri);
            if !self.kernel.is_cached(&read, &self.caller) {
                return;
            }
            match self.kernel.issue(read, &self.caller).await {
                Ok(repr) => accounted.extend(repr.threads().iter().cloned()),
                Err(_) => return,
            }
        }
        let foreign = left(&accounted);
        let (noun, list) = match foreign.as_slice() {
            [] => return,
            [one] => ("a foreign thread", one.clone()),
            many => ("foreign threads", many.join(", ")),
        };
        report.findings.push(self.finding(
            Check::Cacheable,
            format!(
                "declared pure (`Suite::pure`) but carries {noun} {list}: neither its own name \
                 nor anything it read, so the answer hangs from state that is not one of its \
                 inputs, and a pure function reads none. Drop the `pure` declaration (the thread \
                 already answers \"what cuts this?\"), or drop the dependency if the endpoint \
                 really is a function of its inputs"
            ),
        ));
    }

    // ----- PIPELINE (invoking half) --------------------------------------------

    async fn pipeline(&mut self, report: &mut Report) {
        if !self.runs(Check::Pipeline) {
            return;
        }
        let Some(content) = self.content_spec() else {
            return;
        };
        let args = self.pipeline_args(content);
        if !self.may_fire() {
            return;
        }
        self.caller_used = true;
        let result = self.kernel.issue(self.request(&args), &self.caller).await;
        self.observe(Probe::Pipeline, &result);
        self.fired = Some(result.clone());
        if let Err(Error::MissingArgument(name)) = &result {
            // With `content` and every required input supplied, a missing argument
            // is one the contract does not declare — `content` itself unread and
            // some other name read instead, or a required input marked optional.
            report.findings.push(self.finding(
                Check::Pipeline,
                format!(
                    "declares `content`, yet with `content` and every required input \
                     supplied it reports `{name}` missing: it reads an input its contract \
                     does not declare, so the piped value never reaches it"
                ),
            ));
        }
    }

    // ----- REQUIRED -----------------------------------------------------------

    /// Hold the contract's statement of what a call NEEDS to what the endpoint does
    /// (ledger #329, #15). Five adopters wrote the first half by hand; ARGSPECS reads
    /// `required` and the minimal call supplies every required input, so nothing else
    /// could see a `required` the endpoint does without.
    ///
    /// From the call the walk resolved, it makes the calls the contract's rules are
    /// about: each required by-value input dropped; for each input group (core
    /// 0.1.105), every member dropped, each other member alone, and a second member
    /// beside the first where the group allows at most one. Core's one checker
    /// ([`ActionSpec::check`]) says whether the contract refuses each call (a
    /// violation the baseline call did not already have), and the endpoint must
    /// agree: a call the contract refuses gets a typed refusal (`MissingArgument` or
    /// `InvalidArgument`), and a call it accepts is not refused as missing an argument.
    /// A disagreement is the pre-flight (`urn:kernel:validate`, an MCP tool schema)
    /// refusing a call the endpoint serves, or blessing one it refuses.
    async fn required(&mut self, report: &mut Report) {
        if !self.runs(Check::Required) {
            return;
        }
        let mutating = self.spec.verb.is_mutating();
        if mutating && !self.suite.required_on_mutating {
            return;
        }
        if !required_probes_something(self.spec) {
            return;
        }
        // The call each probe is measured against: the minimal resolution for a read;
        // for a write, the pipeline probe's firing when it made one, else one firing
        // with the minimal inputs (the declaration opted into firings).
        let (args, baseline) = if mutating {
            match (&self.fired, self.content_spec()) {
                (Some(fired), Some(content)) => {
                    (self.pipeline_args(content), fired.clone().map(|_| ()))
                }
                _ => {
                    if !self.may_fire() {
                        return;
                    }
                    let args = self.suite.args_for(self.id, self.spec);
                    self.caller_used = true;
                    let fired = self.kernel.issue(self.request(&args), &self.caller).await;
                    self.observe(Probe::Baseline, &fired);
                    (args, fired.map(|_| ()))
                }
            }
        } else {
            let args = self.suite.args_for(self.id, self.spec);
            let resolved = self.resolve_minimal(false).await.map(|_| ());
            (args, resolved)
        };
        if let Err(err) = baseline {
            self.unprobed(
                report,
                Check::Required,
                format!(
                    "the call each required input is dropped from did not resolve ({err}), so \
                     what a missing input does was not observed"
                ),
            );
            return;
        }
        // What the contract already says about the baseline: a probe's verdict is the
        // violations it ADDS, so a fixture the contract would refuse does not make
        // every probe read as refused.
        let before = self.spec.check(&proposed(&args));
        for probe in self.required_plan(&args) {
            if !self.may_fire() {
                return;
            }
            let refused_by_contract = self
                .spec
                .check(&proposed(&probe.args))
                .into_iter()
                .any(|v| !before.contains(&v));
            self.caller_used = true;
            let outcome = self
                .kernel
                .issue(self.request(&probe.args), &self.caller)
                .await;
            // The evidence, whatever came back: a clean REQUIRED says what it probed.
            report.declared.evidence.required.push(RequiredProbe {
                endpoint: self.id.to_string(),
                verb: self.spec.verb,
                input: probe.input(),
                refusal: outcome.as_ref().err().map(|e| e.kind().to_string()),
                group: probe.group.map(InputGroup::node_segment),
                given: probe.given.clone(),
                contract_refuses: refused_by_contract,
            });
            self.judge_required(&probe, refused_by_contract, outcome.map(|_| ()), report);
        }
    }

    /// The calls [`required`](Self::required) makes from the baseline `args`.
    fn required_plan(&self, args: &Args) -> Vec<RequiredCall<'a>> {
        let spec: &'a ActionSpec = self.spec;
        let mut plan = Vec::new();
        for input in droppable(spec) {
            let mut dropped = args.clone();
            if dropped.remove(&input.name).is_some() {
                plan.push(RequiredCall {
                    args: dropped,
                    shape: CallShape::Dropped(input.name.clone()),
                    group: None,
                    given: Vec::new(),
                });
            }
        }
        let fixture = self.suite.fixture_for(self.id, self.spec.verb);
        let value_of = |name: &str| -> Option<Vec<u8>> {
            fixture.and_then(|f| f.args.get(name).cloned()).or_else(|| {
                self.spec
                    .inputs
                    .iter()
                    .find(|i| i.name == name)
                    .map(|i| sample_value(i).into_bytes())
            })
        };
        for group in &spec.groups {
            let given_now: Vec<&str> = group
                .members
                .iter()
                .map(String::as_str)
                .filter(|m| given(args, m))
                .collect();
            let without: Args = args
                .iter()
                .filter(|(k, _)| !group.has(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let with = |members: &[&str]| -> Option<RequiredCall<'a>> {
                let mut call = without.clone();
                for m in members {
                    let value = match args.get(*m) {
                        Some(v) if given(args, m) => v.clone(),
                        _ => value_of(m)?,
                    };
                    call.insert((*m).to_string(), value);
                }
                Some(RequiredCall {
                    args: call,
                    shape: CallShape::Group,
                    group: Some(group),
                    given: members.iter().map(|m| (*m).to_string()).collect(),
                })
            };
            let wants_one = matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne);
            let at_most_one = matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtMostOne);
            if wants_one && !given_now.is_empty() {
                plan.extend(with(&[]));
                for member in group.members.iter().map(String::as_str) {
                    if !given_now.contains(&member) {
                        plan.extend(with(&[member]));
                    }
                }
            }
            if at_most_one {
                let first = given_now
                    .first()
                    .copied()
                    .or(group.members.first().map(String::as_str));
                let second = group
                    .members
                    .iter()
                    .map(String::as_str)
                    .find(|m| Some(*m) != first);
                if let (Some(first), Some(second)) = (first, second) {
                    plan.extend(with(&[first, second]));
                }
            }
        }
        plan
    }

    /// One [`required`](Self::required) call's outcome against what the contract says
    /// of it.
    fn judge_required(
        &self,
        call: &RequiredCall<'_>,
        refused_by_contract: bool,
        outcome: Result<(), Error>,
        report: &mut Report,
    ) {
        let what = call.described();
        match (refused_by_contract, outcome) {
            (true, Ok(())) => report.findings.push(self.finding(
                Check::Required,
                match &call.shape {
                    CallShape::Dropped(name) => format!(
                        "declares `{name}` required, but resolved without it: declared required, \
                         actually optional. `urn:kernel:validate` and an MCP tool schema both \
                         refuse a valid call that omits it. Mark it `.optional()` (with \
                         `.default_value(…)` if it has one), or refuse its absence \
                         (`inv.inline_str(\"{name}\")?` is a typed `MissingArgument`)"
                    ),
                    CallShape::Group => format!(
                        "declares {}, but resolved {what}: the contract refuses a call the \
                         endpoint serves, so `urn:kernel:validate` and an MCP tool schema refuse \
                         it too. {}",
                        call.rule(),
                        if call.given.is_empty() {
                            "Refuse the call with none of them (a typed `MissingArgument`), or \
                             drop the group"
                                .to_string()
                        } else {
                            "Refuse the pair (a typed `InvalidArgument`), or declare \
                             `InputGroup::at_least_one` if both together are fine"
                                .to_string()
                        }
                    ),
                },
            )),
            (true, Err(Error::MissingArgument(_) | Error::InvalidArgument { .. })) => {}
            (_, Err(err)) if err.is_transient() => self.unprobed(
                report,
                Check::Required,
                format!(
                    "{what} it failed transiently ({err}), so what the endpoint does with that \
                     call was not observed"
                ),
            ),
            (true, Err(err)) => report.findings.push(self.finding(
                Check::Required,
                match &call.shape {
                    CallShape::Dropped(name) => format!(
                        "refused a call without the required `{name}`, but as `{err}`, not as a \
                         typed `MissingArgument` or `InvalidArgument`: an agent cannot tell its own \
                         omission from a failure of the endpoint (`inv.inline_str(\"{name}\")?` \
                         refuses with `MissingArgument`)"
                    ),
                    CallShape::Group => format!(
                        "refused a call {what}, which breaks {}, but as `{err}`, not as a typed \
                         `MissingArgument` or `InvalidArgument`: an agent cannot tell its own \
                         mistake from a failure of the endpoint",
                        call.rule()
                    ),
                },
            )),
            (false, Ok(())) => {}
            (false, Err(Error::MissingArgument(missing))) => report.findings.push(self.finding(
                Check::Required,
                format!(
                    "declares {}, so a call {what} conforms, but the endpoint refused it as \
                     missing `{missing}`: core's pre-flight blesses a call the endpoint refuses \
                     (the false green ledger #15 removed). Read the member the call gave, or \
                     correct the group",
                    call.rule()
                ),
            )),
            (false, Err(err)) => self.unprobed(
                report,
                Check::Required,
                format!(
                    "{what} it was refused as `{err}`, which is not a missing argument: the value \
                     the walk sent may be the cause, so whether the endpoint honors {} was not \
                     observed. A Fixture naming the member decides it",
                    call.rule()
                ),
            ),
        }
    }

    // ----- OUTPUTS ------------------------------------------------------------

    /// The bare media type the action serves with its minimal inputs must be one of
    /// its declared outputs. Reads the resolution the other checks already made —
    /// for a mutating verb, the pipeline probe's firing (or an RDF face's minimal
    /// resolution) — so it never fires a Sink or Delete itself; what it could not
    /// observe is recorded as unprobed.
    async fn outputs(&mut self, report: &mut Report) {
        if !self.runs(Check::Outputs) {
            return;
        }
        // The arguments of whichever resolution is read, so a failure can say what
        // it was sent: the pipeline probe's when it fired, the minimal ones otherwise.
        let mut args = self.suite.args_for(self.id, self.spec);
        let probed = if self.spec.verb.is_mutating() {
            match (&self.fired, self.content_spec()) {
                (Some(fired), Some(content)) => {
                    args = self.pipeline_args(content);
                    Some(fired.clone())
                }
                (Some(fired), None) => Some(fired.clone()),
                (None, _) => self.minimal.clone().map(|r| r.map(|(repr, _)| repr)),
            }
        } else {
            Some(self.resolve_minimal(false).await.map(|(repr, _)| repr))
        };
        let repr = match probed {
            None => {
                self.unprobed(
                    report,
                    Check::Outputs,
                    "never fired under root: a mutating action is fired only by the pipeline \
                     probe (PIPELINE, on an action declaring `content`), so what it serves was \
                     not observed",
                );
                return;
            }
            Some(Err(err)) => {
                self.report_failure(Check::Outputs, &err, &args, report);
                self.unprobed(
                    report,
                    Check::Outputs,
                    "the minimal resolution failed, so nothing was served",
                );
                return;
            }
            Some(Ok(repr)) => repr,
        };
        let got = rdf::bare_media_type(&repr.repr_type.media_type);
        let declared: Vec<String> = self
            .spec
            .outputs
            .iter()
            .map(|o| rdf::bare_media_type(o))
            .collect();
        if declared.contains(&got) {
            return;
        }
        // `as=` is the caller's label, not the endpoint's choice: a stylesheet that
        // emits SVG is relabeled `image/svg+xml` by a fixture's `as`, and the
        // declared outputs (what the endpoint chooses by itself) need not list it.
        let args = self.suite.args_for(self.id, self.spec);
        if let Some(label) = args
            .get("as")
            .and_then(|a| std::str::from_utf8(a).ok())
            .map(rdf::bare_media_type)
        {
            if label == got {
                self.unprobed(
                    report,
                    Check::Outputs,
                    format!(
                        "served the caller's `as={got}` label; the endpoint's own choice of \
                         face was not observed"
                    ),
                );
                return;
            }
        }
        // A pass-through (core 0.1.105, ledger #45/#301): `outputs` reads "at least
        // these", and the served type is held to where the contract says it comes from.
        match &self.spec.output_from {
            Some(OutputFrom::Input(name)) => {
                if let Some(detail) = self.output_from_input(name, &got, &args, report).await {
                    report.findings.push(self.finding(Check::Outputs, detail));
                }
                return;
            }
            Some(OutputFrom::Origin) => {
                self.unprobed(
                    report,
                    Check::Outputs,
                    format!(
                        "a declared pass-through (`output_from` origin): served `{got}`, a type \
                         decided outside the kernel at resolve time, so the contract states \
                         nothing to hold it to"
                    ),
                );
                return;
            }
            // `OutputFrom` is non-exhaustive: a form this suite does not know yet is
            // held to the declared outputs, as before.
            _ => {}
        }
        let detail = if declared.is_empty() {
            format!(
                "served `{got}` but declares no output: a face the manifold does not announce \
                 (`.output(\"{got}\")`)"
            )
        } else {
            format!(
                "served `{got}` with its minimal inputs but declares only {}: a face the \
                 manifold does not announce and the RDF checks never saw — declare it \
                 (`.output(\"{got}\")`) or serve what is declared",
                declared
                    .iter()
                    .map(|d| format!("`{d}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        report.findings.push(self.finding(Check::Outputs, detail));
    }

    /// [`outputs`](Self::outputs) for `output_from(Input(name))`: the type served must be
    /// the type of the resource the input names. A finding's detail, or `None` when it
    /// matched or could not be observed (recorded as unprobed).
    async fn output_from_input(
        &mut self,
        name: &str,
        got: &str,
        args: &Args,
        report: &mut Report,
    ) -> Option<String> {
        let input = self.spec.inputs.iter().find(|i| i.name == name);
        if input.is_some_and(|i| i.source == InputSource::Binding) {
            self.unprobed(
                report,
                Check::Outputs,
                format!(
                    "a declared pass-through (`output_from` input `{name}`, a binding): served \
                     `{got}`; the resource a binding determines is the endpoint's to find, so \
                     the walk cannot resolve it to compare"
                ),
            );
            return None;
        }
        let value = args
            .get(name)
            .map(|v| String::from_utf8_lossy(v).into_owned());
        let Some(iri) = value.as_deref().and_then(|v| Iri::parse(v).ok()) else {
            self.unprobed(
                report,
                Check::Outputs,
                format!(
                    "a declared pass-through (`output_from` input `{name}`): served `{got}`, but \
                     the call's `{name}` ({}) is not an IRI the walk can resolve to compare",
                    value.map_or("not sent".to_string(), |v| format!("`{}`", shown(&v)))
                ),
            );
            return None;
        };
        // A read of the input's resource, never a firing of the action: allowed in a
        // set-aside run too.
        self.caller_used = true;
        let source = Request::new(Verb::Source, iri.clone());
        match self.kernel.issue(source, &self.caller).await {
            Ok(origin) => {
                let expected = rdf::bare_media_type(&origin.repr_type.media_type);
                (expected != got).then(|| {
                    format!(
                        "declares its output type comes from `{name}` (`output_from`), and \
                         `{name}` is `{iri}`, which serves `{expected}`; but it served `{got}`, \
                         which is neither that nor a declared output. Serve the input's type, \
                         or declare `{got}` (`.output(\"{got}\")`)",
                        iri = iri.as_str()
                    )
                })
            }
            Err(err) => {
                self.unprobed(
                    report,
                    Check::Outputs,
                    format!(
                        "a declared pass-through (`output_from` input `{name}`): served `{got}`, \
                         but `{name}`'s resource `{}` did not resolve ({err}), so there was no \
                         type to compare",
                        iri.as_str()
                    ),
                );
                None
            }
        }
    }

    // ----- RESOLVES -----------------------------------------------------------

    /// Judge a WAIVED `RESOLVES` for this action, in a set-aside run: read the
    /// resolution the needing checks read (the minimal one for a read, the pipeline
    /// probe's firing for a write) and report its failure. On the walk proper
    /// `RESOLVES` fires nothing of its own: the needing checks report through
    /// [`report_failure`](Self::report_failure). A write no probe fired cannot be
    /// judged without firing it, so that waiver is left unjudged.
    async fn resolves(&mut self, report: &mut Report) {
        let mut args = self.suite.args_for(self.id, self.spec);
        let outcome = if self.spec.verb.is_mutating() {
            match (&self.fired, self.content_spec()) {
                (Some(fired), Some(content)) => {
                    args = self.pipeline_args(content);
                    Some(fired.clone())
                }
                (Some(fired), None) => Some(fired.clone()),
                (None, _) => self.minimal.clone().map(|r| r.map(|(repr, _)| repr)),
            }
        } else {
            Some(self.resolve_minimal(false).await.map(|(repr, _)| repr))
        };
        match outcome {
            None => self.shadow_blocked = true,
            Some(Err(err)) => self.report_failure(Check::Resolves, &err, &args, report),
            Some(Ok(_)) => {}
        }
    }

    // ----- FAILURES -----------------------------------------------------------

    /// Hold every failure the walk observed from this action to what it declares
    /// (ledger #1546), by core's one predicate, [`ActionSpec::admits_failure`] over
    /// [`Error::class`]. Runs the action's `fails_under` probes first, so their failures
    /// are judged with the rest.
    ///
    /// Two observations belong to another check and are left to it: a `Denied` under
    /// the no-grants probe is ENFORCED's (an undeclared gate, whatever the declaration
    /// says), and REQUIRED's drops are never observed here, since the call is wrong on
    /// purpose and the kind of its refusal is REQUIRED's subject.
    async fn failures(&mut self, report: &mut Report) {
        if !self.runs(Check::Failures) {
            return;
        }
        for i in 0..self.fails.len() {
            self.fails_under(i, report).await;
        }
        // A read nothing above resolved (FAILURES selected alone, or every other
        // invoking check waived) is resolved once here; a write is judged on what the
        // walk already fired, and never fired for this check.
        if !self.spec.verb.is_mutating() && self.minimal.is_none() {
            let _ = self.resolve_minimal(false).await;
        }
        let mut failed: Vec<FailureClass> = Vec::new();
        for o in &self.observed {
            if let Some(error) = &o.error {
                if !failed.contains(&error.class()) {
                    failed.push(error.class());
                }
            }
        }
        report.declared.evidence.failures.push(FailuresSeen {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            declared: self.spec.failures.classes().map(<[FailureClass]>::to_vec),
            resolutions: self.observed.len(),
            failed,
        });
        let Some(declared) = self.spec.failures.classes() else {
            return;
        };
        // One finding per undeclared class, naming every probe that produced it.
        let mut undeclared: Vec<(FailureClass, Vec<Probe>, &Error)> = Vec::new();
        for o in &self.observed {
            let Some(error) = &o.error else {
                continue;
            };
            if o.probe == Probe::NoGrants && matches!(error, Error::Denied(_)) {
                continue;
            }
            let class = error.class();
            if self.spec.admits_failure(class) != Some(false) {
                continue;
            }
            match undeclared.iter_mut().find(|(c, _, _)| *c == class) {
                Some((_, probes, _)) => {
                    if !probes.contains(&o.probe) {
                        probes.push(o.probe);
                    }
                }
                None => undeclared.push((class, vec![o.probe], error)),
            }
        }
        let findings: Vec<Finding> = undeclared
            .into_iter()
            .map(|(class, probes, error)| {
                let from = probes
                    .iter()
                    .map(|p| p.described())
                    .collect::<Vec<_>>()
                    .join(", ");
                self.finding(
                    Check::Failures,
                    format!(
                        "declares its failure classes ({}), and {from} failed as `{}`, class \
                         `{}`, which the declaration does not admit (`ActionSpec::admits_failure`: \
                         `permanent` is implied, and `denied` beside a `requires`): {}. A caller \
                         or an agent plans its retries from the declaration, so either return the \
                         variant of a class it declares ({}), or declare this one \
                         (`.fails(FailureClass::{class:?})`)",
                        crate::report::class_words(declared),
                        error.kind(),
                        class.word(),
                        crate::report::visible(&error.to_string()),
                        class_hint(class)
                    ),
                )
            })
            .collect();
        report.findings.extend(findings);
    }

    /// Fire one [`Suite::fails_under`] probe and hold its failure to the stated class.
    async fn fails_under(&mut self, i: usize, report: &mut Report) {
        if !self.may_fire() {
            return;
        }
        let (index, probe, class, target) = {
            let p = &self.fails[i];
            (p.index, p.probe, p.class, p.target.clone())
        };
        let mut args = self.suite.args_for(self.id, self.spec);
        for (name, value) in &probe.args {
            args.insert(name.clone(), value.clone());
        }
        let caller = probe
            .capability
            .clone()
            .unwrap_or_else(|| self.caller.clone());
        let result = self
            .kernel
            .issue(build_request(self.spec.verb, &target, &args), &caller)
            .await;
        self.observe(Probe::FailsUnder, &result);
        self.fails_ran.push(index);
        let shown = probe.to_string();
        let (got, kind) = match &result {
            Ok(_) => (None, None),
            Err(err) => (Some(err.class()), Some(err.kind().to_string())),
        };
        match &result {
            Ok(_) => report.findings.push(self.finding(
                Check::Failures,
                format!(
                    "the `fails_under` probe `{shown}` resolved: it was to fail with class \
                     `{}`, and the input it names did not provoke that failure",
                    class.word()
                ),
            )),
            Err(err) if err.class() != class => report.findings.push(self.finding(
                Check::Failures,
                format!(
                    "the `fails_under` probe `{shown}` was to fail with class `{}`, and got \
                     `{}` (class `{}`): {}. Return the variant of the stated class ({})",
                    class.word(),
                    err.kind(),
                    err.class().word(),
                    crate::report::visible(&err.to_string()),
                    class_hint(class)
                ),
            )),
            Err(_) => {}
        }
        report.declared.evidence.fails_under.push(FailsUnder {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            probe: shown,
            expected: class,
            got,
            kind,
        });
    }

    fn unprobed(&self, report: &mut Report, check: Check, reason: impl Into<String>) {
        report.unprobed.push(Unprobed {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            check,
            reason: reason.into(),
        });
    }
}

/// Which `Error` variants spell a failure class, for a finding's fix: the variants
/// `Error::class` maps to it.
fn class_hint(class: FailureClass) -> &'static str {
    match class {
        FailureClass::Transient => "`Error::Timeout`, `Error::Unavailable`",
        FailureClass::RetryAfter => {
            "`Error::RetryAfter { delay, .. }`, for a rate limit or a window that reopens"
        }
        FailureClass::OverBudget => {
            "`Error::OverBudget`, for an input past a bound the endpoint holds"
        }
        FailureClass::Denied => "`Error::Denied`, under a declared `requires`",
        FailureClass::Refused => {
            "`Error::Refused`, for a remote authority refusing the credential the endpoint presented"
        }
        FailureClass::Permanent => {
            "`Error::InvalidArgument`, `Error::NotFound`, `Error::Conflict`, `Error::Unsupported`"
        }
        FailureClass::Unknown => "`Error::Endpoint`, for a failure the endpoint cannot classify",
        _ => "see `Error::class`",
    }
}

/// Where a traced `Denied` came from (see [`RefusedBy`]).
fn refused_by(events: &[TraceEvent]) -> RefusedBy {
    let denied_note = |e: &TraceEvent| {
        e.notes
            .iter()
            .find(|(k, _)| k == DENIED_NOTE)
            .map(|(_, v)| v.clone())
    };
    let failed_denied = |e: &TraceEvent| {
        e.notes
            .iter()
            .any(|(k, v)| k == FAILED_NOTE && v == "denied")
    };
    let Some(root) = events.iter().find(|e| e.parent.is_none()) else {
        return RefusedBy::Untraced;
    };
    if let Some(lacking) = denied_note(root) {
        return RefusedBy::Floor { lacking };
    }
    // The first refusal recorded below the root: an event is recorded when it
    // finishes, so the deepest refusal finishes first.
    if let Some(sub) = events
        .iter()
        .find(|e| e.parent.is_some() && (denied_note(e).is_some() || failed_denied(e)))
    {
        return RefusedBy::SubRequest {
            target: sub.target.clone(),
        };
    }
    RefusedBy::Module
}

/// A caller's scopes as a finding names them.
fn shown_scopes(scopes: &[String]) -> String {
    if scopes.is_empty() {
        return "nothing".to_string();
    }
    scopes
        .iter()
        .map(|s| format!("`{}`", crate::report::visible(s)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A set of threads as a finding names them: `` `a`, `b` ``, the first five, then how
/// many more.
fn shown_threads(threads: &BTreeSet<Thread>) -> String {
    const SHOWN_THREADS: usize = 5;
    let mut shown: Vec<String> = threads
        .iter()
        .take(SHOWN_THREADS)
        .map(|t| format!("`{}`", t.as_str()))
        .collect();
    if threads.len() > SHOWN_THREADS {
        shown.push(format!("and {} more", threads.len() - SHOWN_THREADS));
    }
    match shown.len() {
        0 => "no thread".to_string(),
        1 => format!("the thread {}", shown[0]),
        _ => format!("the threads {}", shown.join(", ")),
    }
}

// ----- SPACE-NAME helpers -------------------------------------------------------

/// An id as a report shows it: the IRI in backticks, or `no name`.
fn show_id(id: Option<&Iri>) -> String {
    match id {
        Some(id) => format!("`{}`", id.as_str()),
        None => "no name".to_string(),
    }
}

/// A topology as one line per node and door, each with its path from the root —
/// what the first difference between two calls is printed from.
fn lines_of(top: &Topology) -> Vec<String> {
    let mut out = Vec::new();
    push_lines(top, "/", &mut out);
    out
}

fn push_lines(node: &Topology, path: &str, out: &mut Vec<String>) {
    let id = node
        .id
        .as_ref()
        .map(|i| format!(" <{}>", i.as_str()))
        .unwrap_or_default();
    match &node.kind {
        SpaceKind::EndpointSpace { doors } => {
            out.push(format!("{path} endpoint space{id}"));
            for (i, door) in doors.iter().enumerate() {
                out.push(format!(
                    "{path} door {i} `{}` ({}) -> {}",
                    door.pattern,
                    door.kind.keyword(),
                    door.endpoint
                ));
                if let Some(corridor) = &door.confined {
                    push_lines(corridor, &format!("{path}door{i}/confined/"), out);
                }
            }
        }
        other => out.push(format!("{path} {other:?}{id}")),
    }
    for (i, child) in node.children.iter().enumerate() {
        push_lines(child, &format!("{path}{i}/"), out);
    }
}

/// `entries()` as lines, for a space whose topology says less than its listing.
fn entry_lines(entries: Option<Vec<SpaceEntry>>) -> Vec<String> {
    match entries {
        None => vec!["(not enumerable)".to_string()],
        Some(entries) => entries
            .iter()
            .map(|e| format!("entry `{}` -> {}", e.pattern, e.endpoint))
            .collect(),
    }
}

/// The first line where two calls disagree, both sides quoted.
fn first_difference(first: &[String], second: &[String]) -> String {
    let at = first
        .iter()
        .zip(second)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| first.len().min(second.len()));
    let side = |lines: &[String]| {
        lines
            .get(at)
            .map(|l| format!("`{}`", l.trim()))
            .unwrap_or_else(|| "nothing".to_string())
    };
    format!("first call: {}; second call: {}", side(first), side(second))
}

/// Every door in a tree, confined corridors included.
fn count_doors(node: &Topology) -> usize {
    let here = match &node.kind {
        SpaceKind::EndpointSpace { doors } => doors
            .iter()
            .map(|d| 1 + d.confined.as_deref().map_or(0, count_doors))
            .sum(),
        _ => 0,
    };
    here + node.children.iter().map(count_doors).sum::<usize>()
}

/// Every node that reports nothing about what it encloses.
fn count_opaque(node: &Topology) -> usize {
    let here = match &node.kind {
        SpaceKind::EndpointSpace { doors } => doors
            .iter()
            .map(|d| d.confined.as_deref().map_or(0, count_opaque))
            .sum(),
        SpaceKind::Opaque => 1,
        _ => 0,
    };
    here + node.children.iter().map(count_opaque).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_sealed_prefix_is_core_owned() {
        let mut seals = Kernel::new(Arc::new(ikigai_core::EndpointSpace::new())).sealed();
        seals.push(("urn:stream:".into(), SealOwner::Core));
        seals.push(("urn:future-core:".into(), SealOwner::Core));
        seals.push(("urn:host:".into(), SealOwner::Host));
        seals.push((
            "urn:module:".into(),
            SealOwner::Level(Iri::parse("urn:level:module").unwrap()),
        ));
        for pattern in [
            "urn:kernel:catalog",
            "urn:stream:collect",
            "urn:stream:text",
            "urn:stream:range",
            "urn:future-core:op",
        ] {
            assert!(
                core_owned(pattern, &seals),
                "core-owned entry {pattern} must be skipped"
            );
        }
        for pattern in [
            "urn:host:op",
            "urn:module:op",
            "urn:example:op",
            "urn:streaming:op",
        ] {
            assert!(
                !core_owned(pattern, &seals),
                "module/host entry {pattern} must still be walked"
            );
        }
    }

    #[test]
    fn mounted_operations_are_learned_from_every_core_seal() {
        use ikigai_core::{EndpointSpace, Exact, FnEndpoint};

        let kernel = Kernel::new(Arc::new(
            EndpointSpace::new().bind(
                Exact::new("urn:future-core:op"),
                FnEndpoint::new("future-op", |_| unreachable!("description only"))
                    .with_description(Description::new("future-op").verb(Verb::Source)),
            ),
        ));
        let mut seals = kernel.sealed();
        // Simulate a future core catalog without raising this crate's core pin.
        seals.push(("urn:future-core:".into(), SealOwner::Core));
        let ops = KernelOps::of(&kernel, &kernel.entries().unwrap(), &seals);
        let renamed = SpaceEntry::new("urn:edge:future-core:op", "remote").with_origin("peer");
        assert!(
            ops.renamed(&renamed, None),
            "recognize the re-prefixed name without a contract"
        );
        let moved = SpaceEntry::new("urn:edge:other", "remote").with_origin("peer");
        assert!(
            ops.renamed(&moved, Some(&Description::new("future-op"))),
            "recognize the description id even under another name"
        );
        assert!(!ops.renamed(&SpaceEntry::new("urn:edge:future-core:op", "local"), None));
        assert!(!ops.renamed(&moved, Some(&Description::new("module-op"))));
    }

    #[test]
    fn sample_values_follow_the_declared_class() {
        let s = |class: &str| sample_value(&ArgSpec::new("a").class(format!("{XSD}{class}")));
        assert_eq!(s("integer"), "1");
        assert_eq!(s("boolean"), "true");
        assert_eq!(s("dateTime"), "2026-01-01T00:00:00Z");
        assert_eq!(s("anyURI"), PLACEHOLDER_IRI);
        assert_eq!(s("string"), PLACEHOLDER);
        assert_eq!(
            sample_value(&ArgSpec::new("who").class("https://schema.org/Person")),
            PLACEHOLDER_IRI
        );
        assert_eq!(sample_value(&ArgSpec::new("raw")), PLACEHOLDER);
        assert_eq!(
            sample_value(&ArgSpec::new("mode").one_of(["added", "removed"])),
            "added"
        );
        assert_eq!(
            sample_value(&ArgSpec::new("as").one_of(["a", "b"]).default_value("b")),
            "b"
        );
    }

    #[test]
    fn kebab_case_ids() {
        for ok in ["to-upper", "kernel-catalog", "wc", "a11y-config"] {
            assert!(is_kebab(ok), "{ok}");
        }
        for bad in ["toUpper", "urn:cms:graph", "-x", "x-", "a--b", "", "Tag"] {
            assert!(!is_kebab(bad), "{bad}");
        }
    }

    #[test]
    fn template_vars_are_read_from_patterns_only() {
        assert_eq!(template_vars("urn:example:echo"), None);
        assert_eq!(
            template_vars("urn:file:{path}").unwrap(),
            vec!["path".to_string()]
        );
    }
}

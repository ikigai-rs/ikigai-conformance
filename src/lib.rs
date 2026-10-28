//! What "done" means for an ikigai module, as one test.
//!
//! The module recipe in the ikigai field guide is a page of prose every module
//! author must remember. The checkable rules are one line here:
//!
//! ```
//! # use ikigai_core::{ArgSpec, Description, EndpointSpace, Exact, FnEndpoint, Invocation};
//! # use ikigai_core::{Kernel, ReprType, Representation, Verb};
//! # use std::sync::Arc;
//! # fn my_kernel() -> Kernel {
//! #     let upper = FnEndpoint::new("upper", |inv: &Invocation<'_>| {
//! #         Ok(Representation::new(ReprType::new("text/plain"),
//! #             inv.inline_str("in")?.to_uppercase().into_bytes()).cacheable())
//! #     })
//! #     .with_description(Description::new("upper").verb(Verb::Source).verb(Verb::Meta)
//! #         .input(ArgSpec::new("in").class("http://www.w3.org/2001/XMLSchema#string"))
//! #         .output("text/plain"));
//! #     Kernel::new(Arc::new(EndpointSpace::new().bind(Exact::new("urn:text:upper"), upper)))
//! # }
//! # fn main() {
//! ikigai_conformance::Suite::new()
//!     .pure("upper")                       // a pure function needs no golden thread
//!     .run_blocking(&my_kernel())
//!     .into_result()
//!     .unwrap();
//! # }
//! ```
//!
//! — or, with no configuration, `ikigai_conformance::check(&my_kernel()).unwrap()`.
//! Either way the panic message is the whole checklist: [`Report`]'s `Debug`
//! delegates to its `Display`, and [`Report::assert_clean`] is the one-line
//! assertion for a run you want to keep reading (`suite.run_blocking(&k).assert_clean()`).
//!
//! [`check`] walks every endpoint the kernel lists ([`Kernel::entries`] →
//! [`Kernel::describe_pattern`]), every action each declares, and returns **every**
//! violation at once as a [`Report`] — one line per finding, endpoint id first, so a
//! failure is a checklist rather than the first miss. Findings are typed
//! ([`Finding`]), checks are individually selectable ([`Checks`]) so a module can
//! adopt incrementally, and every skipped check is printed as skipped.
//!
//! # The checks
//!
//! Each names the recipe row it mechanizes.
//!
//! | check | recipe row | what it sees |
//! |---|---|---|
//! | [`ArgSpecs`](Check::ArgSpecs) | ArgSpecs from day one | ≥1 action per description; every input has an IRI `class`; a `default` ∈ `one_of` when both exist; input names unique per action; every template variable is a declared binding input |
//! | [`RequiresVerb`](Check::RequiresVerb) | declared = enforced | a `requires` with no verb — enforced on every verb since core 0.1.85, but the catalog cannot say which actions it gates |
//! | [`Enforced`](Check::Enforced) | declared = enforced | under a capability holding no grants, an action with `requires` is refused with a typed `Denied`; an action declaring nothing is not (an undeclared gate: a capability to declare, or a state to document and waive); opt-in, a caller a [`Suite::enforced_under`] probe names (some grant under the prefix, not this target's) is refused with a typed `Denied` by the module's own rule |
//! | [`Authority`](Check::Authority) | declared = enforced | the fourth cell of the same probe: a `Sink` or a `Delete` that declares no `requires` and **mutated anyway** under a capability holding no grants — no scope to withhold, so no caller can be given read without write. A `Source` is not in scope; a mutating action refused for some other reason is unprobed, never a finding |
//! | [`Outputs`](Check::Outputs) | faces are declared | the bare media type the action serves with its minimal inputs (parameters stripped) is one of its declared `outputs` — a wrong declaration hides a face from every consumer that reads outputs, the two RDF checks included; a pass-through (`output_from`, core 0.1.105) is held to the type of the resource its named input names, and an `Origin` one is reported unprobed |
//! | [`SkolemRdf`](Check::SkolemRdf) | skolemize; no blank nodes | every declared RDF face ([`rdf::RDF_FACES`]) resolves with minimal inputs, parses, and has no blank node |
//! | [`Vocabulary`](Check::Vocabulary) | faces use the shared vocabularies | the face **parses**, and every predicate and class in it is defined in `ikigai-vocab`, or under a well-known ([`rdf::WELL_KNOWN_NAMESPACES`]) or module-registered namespace |
//! | [`Cacheable`](Check::Cacheable) | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and — if it never expires (`Expiry::Never`; an `Expiry::At` deadline is a bound) — carries a golden thread besides its own name unless the endpoint is declared pure or takes writes through that name; an endpoint declared pure carries no thread but its own name and what it read; a result declared live ([`Suite::live`]) is `Expiry::Always` |
//! | [`Pipeline`](Check::Pipeline) | pipeline citizenship | a mutating action with by-value inputs (on a `Sink`, required or not) declares `content` (where the pipe's value arrives); an action declaring `content` reads it; a read whose only intake is a pipe-less input group is a `pipeline:` information line ([`Report::named_only`]) |
//! | [`Required`](Check::Required) | ArgSpecs from day one | each input declared required, by value and with no default, dropped from the call the walk resolved, is refused with a typed `MissingArgument` or `InvalidArgument`; a call that succeeds without it is "declared required, actually optional". each input group (core 0.1.105) is probed with every member dropped, each other member alone and a pair, and the endpoint must agree with core's one checker (`ActionSpec::check`) on each call. `Source` and `Exists` by default, `Sink` and `Delete` only under [`Suite::required_on_mutating`] |
//! | [`Names`](Check::Names) | naming convention | the id is a kebab-case noun (the convention `ikigai-core`'s crate docs state) |
//! | [`Validate`](Check::Validate) | a name is spelled into IRIs | `Description::validate()` accepts the description: no id, input name, `class` or `requires` holds a character an `IRIREF` cannot carry, and no name holds a code point no IRI carries (core 0.1.99). Core's emitters encode a name and degrade a bad authored IRI to a literal (core 0.1.100), so the documents parse; this is the strict check on top. `bind` does not validate, so this is where a bad name is caught. Since core 0.1.105 it also refuses a relation the contract cannot honor (a group, `needs`, a value note, an envelope, an example, `output_from`) and a default on a required input (a required binding included; an optional binding may carry one since core 0.1.106), or one the contract refuses |
//! | [`SpaceName`](Check::SpaceName) | a name is a claim: same name, same doors | a space declared self-named ([`Suite::self_named_space`]) claims [`space_iri`](ikigai_core::space_iri)`(module)` under [`SPACE_PREFIX`](ikigai_core::SPACE_PREFIX), on its topology root too, and two calls hold the same doors; a space declared host-named ([`Suite::host_named_space`]) claims nothing |
//! | [`Declarations`](Check::Declarations) | — | every declaration the module made ([`Suite::live`], [`Suite::cacheable`], [`Suite::pure`], [`Suite::namespace`], [`Suite::required_on_mutating`], [`Suite::check_failures`], a [`Suite::fails_under`] probe, a [`Fixture`], an [`opt_out`](Suite::opt_out), an [`opt_out_at`](Suite::opt_out_at), an [`opt_out_check`](Suite::opt_out_check), a declared space) reached the check that would honor it |
//! | [`Recompute`](Check::Recompute) | cacheability | an `Expiry::Never` answer the cache holds is recomputed 24 times, each after a cut of every thread it carries and of everything it read, and every computation must repeat the first one's bytes (a fixture needs enough entries for an order to differ); each thread an endpoint declared is also cut alone. `CACHEABLE`'s second resolution is the cache hit and can only repeat the first, so this is the witness that sees a wrong `.cacheable()` (a fresh nonce, key or timestamp, `HashMap` order) |
//! | [`Resolves`](Check::Resolves) | — | the resolution another check needed resolved; a failure is reported once per action, with the refusal's kind, the check that needed it, and an untyped input the walk had to invent a value for |
//! | [`Datatypes`](Check::Datatypes) | one graph, two faces, the same terms | every literal in an RDF face carries the datatype its property's `rdfs:range` declares in `ikigai-vocab`, the one the generated JSON-LD context coerces it to; a bare Turtle number is `xsd:integer`, so a range the shorthand cannot express (`xsd:positiveInteger`, `xsd:nonNegativeInteger`, `xsd:dateTime`) must be tagged ([`rdf::datatype_mismatches`]) |
//! | [`Failures`](Check::Failures) | declared = produced | **Runs by default since 0.11.0.** An action that states its failure classes (core 0.1.108's `ActionSpec::fails` / `Description::fails`) fails only with a class it admits: every failure the walk observes is held to the declaration by core's `ActionSpec::admits_failure` over `Error::class()` (`permanent` implied, `denied` beside a `requires`); an action that states nothing is listed on a `failures:` line, never a finding; a [`Suite::fails_under`] probe must fail with exactly the class it states |
//! | [`Bodies`](Check::Bodies) | binary body contract | on by default since 0.12.0; with [`Suite::body_space`], directly compare a deterministic `Source` endpoint's `invoke` and collected `invoke_body` bytes and media type, then require `OverBudget` under a smaller kernel body collect budget; missing module space and volatile answers are explicitly unprobed |
//! | [`BodyInputs`](Check::BodyInputs) | declared = enforced | on by default since 0.12.0; each `reads_body` argument answers inline, reference, stored content id and one-shot attached inputs with identical bytes and media type. Supply [`Suite::body_input_store`] for stored content; missing setup is unprobed. Up to four calls per argument on every verb, including mutations |
//!
//! Both RDF checks resolve and **parse** the face, so a mislabeled or malformed
//! graph is reported whichever of the two is selected — under
//! [`SkolemRdf`](Check::SkolemRdf) when it runs, under [`Vocabulary`](Check::Vocabulary)
//! otherwise. A module can rely on `VOCABULARY` alone to prove a new `@prefix` line
//! is well-formed. A face that does not resolve at all is [`Resolves`](Check::Resolves)'s.
//!
//! # What the walk did NOT do
//!
//! A clean report is only worth what it covered, so the report says what it did not
//! reach as loudly as what it found.
//!
//! - **`probed:`** — every face the walk actually resolved, whatever the media
//!   type: `probed: <id> <verb> <face>: N triple(s)` for an RDF face (the count
//!   matters: a face that parsed to **0 triples** says `nothing was checked`,
//!   because the RDF checks pass vacuously over an empty graph) and
//!   `… N byte(s)` for one no check parses. A walk that resolved nothing at all
//!   says so on one line, rather than printing no section — a module with no graph
//!   face used to be indistinguishable from a walk that reached nothing.
//! - **`required:`** — every input [`Required`](Check::Required) dropped and the
//!   refusal kind it got (``required: greet source without `name`: refused
//!   missing-argument``), or ONE line saying it dropped nothing and why (no required
//!   by-value input on `Source`/`Exists`, mutating verbs off, every candidate opted
//!   out or unprobed). A clean `REQUIRED` used to read the same over twelve drops
//!   and over none ([`Report::required_probes`], [`Report::required_nothing`]).
//! - **`unprobed:`** — the actions a check could not observe, with the reason.
//! - **`failures:`** and **`fails under:`** — under [`Failures`](Check::Failures), what
//!   each action that states its failure classes declares and which classes the walk saw
//!   it fail with, one line for every action that states none (compared with nothing),
//!   and each [`Suite::fails_under`] probe's outcome ([`Report::failures_seen`],
//!   [`Report::fails_under_probes`]).
//! - **`note:`** — a [`Suite::opt_out_check`] that suppressed nothing: the waived
//!   check ran with its findings set aside and found none ([`Report::idle_waivers`]).
//!   Information, never a finding.
//! - **[`Report::walked`]** — the description ids the walk reached, by name and in
//!   walk order. An endpoint that stops being bound otherwise makes a report
//!   *cleaner*; this is the list a test can pin.
//! - **`checked:`** — a starred check (`OUTPUTS*`) ran on some endpoints and is
//!   waived on others, with the count on its own line. Without it, a check waived
//!   on five of six endpoints read as a check that ran.
//! - **[`Declarations`](Check::Declarations)** — the same rule turned on the
//!   module's own declarations. A declaration is a promise about a check that will
//!   honor it, and one whose target the walk never reaches is inert: it changes
//!   nothing, fails nothing, and is printed in the report exactly like one that was
//!   consulted. [`Suite::live`] on a `Sink` was the founding instance —
//!   `CACHEABLE` returns early on a non-cacheable verb, so the declaration did
//!   nothing at all while the report said `declared live: notes-write`. The check
//!   covers every declaration, not that one: a fixture whose id is a typo, a
//!   fixture argument naming no input its action declares, a
//!   binding naming no template variable, a waiver for a check that could not have
//!   run, an `opt_out` that excluded nothing, a namespace that accounted for no
//!   term, a [`pure`](Suite::pure) over a result that is never cacheable.
//!
//!   The rule is structural — *could this declaration have applied?* — never "did
//!   it silence a finding today": a standing waiver that happens to be green is
//!   doing its job. A waiver whose CONDITION has passed (the core change it waits
//!   for landed) is the `note:` line instead (0.8.1): the waived check runs with
//!   its findings set aside, and a waiver that set nothing aside is named, beside
//!   its reason, without turning the suite red.
//!
//! # The vocabulary pin
//!
//! [`Vocabulary`](Check::Vocabulary)'s oracle is `ikigai_vocab::VOCABULARY` — a
//! term is defined iff it is a subject there — so this crate's `ikigai-vocab`
//! dependency is load-bearing for DATA, not API, and **the pin tracks the
//! vocabulary HEAD**: it is raised to the newest published version in every release
//! of this crate, changed or not. Nothing enforces that (the crate uses only `NS`
//! and `VOCABULARY`, both ancient, so no compile error can force it up). What a
//! stale pin does: cargo unifies `ikigai-vocab` across the graph, so a module using
//! a term the newest vocabulary defines is told the vocabulary does not define it,
//! and its only local workaround is an `ikigai-vocab` dev-dependency no line
//! imports. If a term you can see in `vocabulary.ttl` is reported as invented,
//! check the resolved version first.
//!
//! # What stays prose
//!
//! Honest residue — what no check here can see, so the field guide keeps it:
//!
//! - **"Declared = enforced" in the converse direction** is approximated, not
//!   proven: a `Denied` from an action declaring nothing is caught, but an endpoint
//!   that enforces an undeclared scope by *succeeding* differently (a filtered
//!   view) is invisible.
//! - **Whether a result *should* be cacheable.** The probe checks a cacheable
//!   result behaves as one; it cannot know that an uncacheable result is a pure
//!   function someone forgot to mark, nor that a marked one reads live state
//!   without a thread — hence [`Suite::pure`], a declaration the module makes.
//!   [`Suite::live`] is the same mechanism for the other polarity: without it, an
//!   endpoint that silently *becomes* cached is invisible, because an undeclared
//!   endpoint is held to nothing in either direction.
//! - **A `.cacheable()` the kernel downgraded.** The kernel returns the
//!   *effective* expiry (the least cacheable of the result and its dependencies),
//!   so an endpoint that marked its result cacheable over a volatile dependency
//!   comes back indistinguishable from one that never marked it. No published core
//!   API exposes the declared expiry; [`Suite::cacheable`] is the declaration that
//!   closes the gap and makes that recomputation a finding.
//! - **Purity and threads** (see above): a result that never expires
//!   (`Expiry::Never`) with no golden thread but its own name is a finding until
//!   declared pure, unless the endpoint declares a `Sink` or `Delete` (a write
//!   through that name cuts the thread the kernel hangs every cacheable read on,
//!   since `ikigai-core` 0.1.73). An `Expiry::At` deadline is a bound and owes
//!   nothing here, however distant it is: the suite judges the kind of bound, not
//!   its length. The
//!   declaration is the mechanism; the judgment is still the author's. A module
//!   that names its state's thread after the endpoint itself is indistinguishable
//!   from one that names nothing: name the state, or opt out with a reason.
//!   The declaration is checked from the other side too (0.8.3): a pure endpoint
//!   whose answer carries a thread that is neither its own name nor the name or a
//!   thread of a resource it read is a finding, since that thread names state it
//!   hangs from. A thread the walk cannot attribute (the first read was a cache
//!   hit, or a resource it read is not in the cache as a plain `Source` of its
//!   name) is never reported.
//! - **Pipeline routing for `Source`.** The engine routes a piped value into the
//!   sole unnamed required input by contract; nothing an endpoint does makes that
//!   right or wrong, so only the mutating verbs' `content` rule and "declares
//!   `content` ⇒ reads it" are checked. Newline-separated list output (the `..`
//!   map convention) is a shape no ArgSpec states.
//! - **A declared golden thread is a promise a host must keep.** [`Cacheable`](Check::Cacheable)
//!   checks the thread set names something besides the endpoint, not that
//!   anything ever cuts a thread in it; a module declaring `urn:file:` threads over
//!   a config home no host watches is clean here and stale in production. The
//!   suite cannot see the host.
//! - **An optional alternative to a required input.** [`Required`](Check::Required)
//!   drops a required input from the call the walk made, which sends no optional
//!   input a fixture did not name. So `content` required beside an optional `in` the
//!   endpoint also accepts (a piped-or-named intake, which `ArgSpec` cannot spell as
//!   "exactly one of") passes, although a call with only `in` succeeds. Declaring the
//!   piped one required is still right: the engine pipes only into a required input.
//! - **A face built over a bundled graph tests that graph too.** A module that
//!   folds `ikigai-vocab` (or its own vocabulary) into every result is probed OVER
//!   it: a blank node arriving in a future vocabulary release fails that module's
//!   walk first.
//! - **A pass-through output.** An action that serves whatever its sub-resolution
//!   or origin says (`urn:httpGet`'s `Content-Type`) has no `Description` spelling
//!   in core — `outputs` is a closed list — so [`Outputs`](Check::Outputs) reports
//!   whatever the fixture's origin serves; such a module subtracts
//!   [`Checks::OUTPUTS`] and says why until core can say "any".
//! - **Reading through the kernel, not `std::fs`** — a lint, not a test.
//! - **Where a fix belongs** (core or module) and **when in doubt, don't cache**.
//!
//! # Before the first run: a walk runs what your endpoints run
//!
//! The invoking checks resolve actions for real: under root with derived inputs, and
//! every action again under a capability holding no grants. Whatever an endpoint
//! does, the walk does. That includes spawning a process, reaching the network,
//! calling a model or writing a calendar. A host that binds such a seam walks
//! [`Checks::STATIC`] first, which resolves nothing and lists every endpoint in
//! [`Report::walked`]. It then writes fixtures and opt-outs for those seams, or
//! scopes dependencies out with [`Suite::own`], before selecting an invoking check. A
//! fixture never reaches a live service: point an LLM endpoint's base URL at a stub
//! the test bound, never at a default like `http://localhost:11434/v1`. The README's
//! "Before the first run" section has the measured incidents.
//!
//! [`Suite::own`] also scopes the walk for a composing host. Endpoints that match no
//! declared id or `*`-prefix are inherited ([`Declarations::inherited`]), and they are not
//! walked. A pattern that matches nothing is a finding no selection or waiver
//! removes.
//!
//! # Fixtures, opt-outs, namespaces
//!
//! The invoking checks resolve each action with the smallest inputs its ArgSpecs
//! admit ([`Suite::fixture`] when the spec cannot say — a path that must exist, a
//! document that must parse, a gzip stream, which is bytes:
//! [`Fixture::arg_bytes`]). They run **against the kernel you pass**, so build it
//! as a test fixture; an action with real side effects opts out by id with a
//! reason ([`Suite::opt_out`]), and the opt-out list, the fixtures, and every other
//! declaration are part of the report. A module that serves its own vocabulary
//! registers the namespace ([`Suite::namespace`]).
//!
//! [`Suite::opt_out`] is coarse — it drops every invoking check for that id — so a
//! rule that is legitimately red on one endpoint takes `ENFORCED`, `CACHEABLE` and
//! `SKOLEM-RDF` down with it. [`Suite::opt_out_check`] waives exactly one check for
//! one endpoint, reason included, and reaches the description-only checks
//! (`NAMES`, `ARGSPECS`) that nothing else could silence per id. Where the waiver
//! can be made exact, prefer that: the [`rdf`] module is public, and
//! [`rdf::parse`] + [`rdf::terms`] + [`rdf::is_defined`] reproduce
//! [`Vocabulary`](Check::Vocabulary) in a hand test, so a module can pin the
//! undefined set as an EXACT list that goes red in both directions — including the
//! day the missing terms land.
//!
//! Every one of those declarations is held to having reached something
//! ([`Declarations`](Check::Declarations)), so a fixture for an id nothing binds
//! and a waiver for a check that could not run are findings rather than lines in a
//! clean report. A declaration that is standing on purpose — a `live` an endpoint
//! will earn next release — says so with
//! `opt_out_check(id, Check::Declarations, "…")`, which puts the reason in the
//! record where the next reader will find it.
//!
//! What a walk fires, under root (or as the caller a [`Fixture::capability`] names
//! for that action: an endpoint about its caller refuses root, which names no
//! principal): a `Source` or `Exists` is resolved once (twice
//! when cacheable — the second is the cache probe), and once more per required
//! by-value input without a default ([`Required`](Check::Required)); each declared
//! RDF face beyond the first is resolved once more with `as=`; a `Sink` or `Delete` declaring
//! `content` is fired **once**, by the pipeline probe, and [`Outputs`](Check::Outputs)
//! reads that same firing; apart from BODY-INPUTS below, a mutating action without `content` is never fired
//! under root ([`Enforced`](Check::Enforced) and [`Authority`](Check::Authority) run
//! under no grants, off **one** shared resolution however many of the two are
//! selected) and is listed as unprobed. Under [`Suite::required_on_mutating`] a
//! `Sink` or `Delete` is also fired once per required by-value input, and once more
//! as the baseline when the pipeline probe did not fire it. Fixture bindings are per entry, not per verb (see [`Fixture::binding`]).
//! [`BodyInputs`](Check::BodyInputs) adds up to four calls per `reads_body` argument
//! on every verb, including mutations. Subtract [`Checks::BODY_INPUTS`] or waive it
//! with a reason when repeated calls cannot produce comparable answers.
//! A check WAIVED for an endpoint ([`Suite::opt_out_check`]) is still run on its
//! reads, after the walk proper, with the findings set aside: the same `Source` and
//! `Exists` resolutions the check fires unwaived, and never a `Sink` or `Delete` the
//! walk would not otherwise fire.
//!
//! **That footprint is per FIRING, and a firing is a request** — the verb, the target
//! IRI and the arguments. The invoking checks are memoized on it, so an endpoint
//! bound N times is fired once for every DISTINCT request those bindings produce, not
//! N times: a space that lists one binding twice (an overlay concatenating its
//! targets' entries) fires it once and says so on a `collapsed:` line. Two bindings at
//! different patterns resolve to different IRIs, so they are two requests and both are
//! probed — including the case that makes `Description::id` the wrong key, two
//! instances of one endpoint type over different state (`urn:file:{path}` jailed to a
//! scratch root, `urn:orgfile:{path}` jailed to a configured directory). A module that
//! wants one of those bindings left alone names it with [`Suite::opt_out_at`]; a module
//! whose two spellings really are ONE resource says so where the kernel can see it —
//! one `Grammar` matching both, or core's `Alias` — because two bindings are two cache
//! entries and two golden threads whatever the suite does.
//!
//! The kernel's own `urn:kernel:*` operations are listed by [`Kernel::entries`] but
//! are core's, not the module's; the walk skips them unless
//! [`Suite::include_kernel_ops`] says otherwise, and a MOUNTED peer's too, under
//! whatever name an alias gave them (`urn:edge:kernel:catalog`). A mounted binding
//! that describes with no action (a peer that cannot answer `Meta`) is not walked, and
//! one finding per mount says so, naming the missing Meta renderer.
#![forbid(unsafe_code)]

mod checks;
pub mod rdf;
mod report;
mod suite;

pub use checks::{Check, Checks};
pub use report::{
    BodyInputComparison, Collapsed, Declarations, DeclaredSpace, DeclaredVocabulary, EnforcedUnder,
    Evidence, Expected, FailsUnder, FailuresSeen, Finding, Inherited, NamedOnly, OptedOut,
    OptedOutAt, OptedOutCheck, Probed, Recomputed, RefusedBy, Report, RequiredProbe, SpaceNaming,
    Unprobed,
};
pub use suite::{Fixture, Suite};

use ikigai_core::Kernel;

/// Run every check against `kernel` on the futures executor: `Ok(report)` when
/// clean, `Err(report)` — the checklist — otherwise. `Suite::new().run_blocking(kernel).into_result()`.
///
/// ```
/// use ikigai_conformance::{check, Check};
/// use ikigai_core::builtins;
/// use ikigai_core::{EndpointSpace, Exact, Kernel};
/// use std::sync::Arc;
///
/// let root = EndpointSpace::new()
///     .bind(Exact::new("urn:example:reverse-list"), builtins::reverse_list());
/// let report = check(&Kernel::new(Arc::new(root))).unwrap_err();
/// // The builtins predate the recipe: an untyped input and — being pure functions
/// // nobody declared pure — a cacheable result with no thread. Their ids became
/// // kebab-case in core 0.1.105, so NAMES has nothing to say.
/// assert!(report.of(Check::ArgSpecs).count() >= 1);
/// assert!(report.of(Check::Names).count() == 0);
/// assert!(report.of(Check::Cacheable).count() == 1);
/// ```
pub fn check(kernel: &Kernel) -> Result<Report, Report> {
    Suite::new().run_blocking(kernel).into_result()
}

/// [`check`] with a selection of checks; the rest are reported as skipped.
///
/// ```
/// use ikigai_conformance::{check_with, Checks};
/// use ikigai_core::builtins;
/// use ikigai_core::{EndpointSpace, Kernel, UriTemplate};
/// use std::sync::Arc;
///
/// let echo = UriTemplate::parse("urn:example:echo/{message}").unwrap();
/// let root = EndpointSpace::new().bind(echo, builtins::echo());
/// let kernel = Kernel::new(Arc::new(root));
/// // Adopt incrementally: only the enforcement checks today.
/// let report = check_with(&kernel, Checks::ENFORCED | Checks::REQUIRES_VERB).unwrap();
/// assert!(report.to_string().contains("skipped: ARGSPECS"), "{report}");
/// ```
pub fn check_with(kernel: &Kernel, checks: Checks) -> Result<Report, Report> {
    Suite::new()
        .checks(checks)
        .run_blocking(kernel)
        .into_result()
}

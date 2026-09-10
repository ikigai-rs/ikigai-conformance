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
//! | [`RequiresVerb`](Check::RequiresVerb) | declared = enforced | a `requires` with no verb — a floor `action_specs()` never yields, so the kernel enforces nothing |
//! | [`Enforced`](Check::Enforced) | declared = enforced | under a capability holding no grants, an action with `requires` is refused with a typed `Denied`; an action declaring nothing is not |
//! | [`Outputs`](Check::Outputs) | faces are declared | the bare media type the action serves with its minimal inputs (parameters stripped) is one of its declared `outputs` — a wrong declaration hides a face from every consumer that reads outputs, the two RDF checks included |
//! | [`SkolemRdf`](Check::SkolemRdf) | skolemize; no blank nodes | every declared RDF face ([`rdf::RDF_FACES`]) resolves with minimal inputs, parses, and has no blank node |
//! | [`Vocabulary`](Check::Vocabulary) | faces use the shared vocabularies | every predicate and class in a face is defined in `ikigai-vocab`, or under a well-known ([`rdf::WELL_KNOWN_NAMESPACES`]) or module-registered namespace |
//! | [`Cacheable`](Check::Cacheable) | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and carries a golden thread unless the endpoint is declared pure |
//! | [`Pipeline`](Check::Pipeline) | pipeline citizenship | a mutating action with by-value inputs declares `content` (where the pipe's value arrives); an action declaring `content` reads it |
//! | [`Names`](Check::Names) | naming convention | the id is a kebab-case noun (the convention `ikigai-core`'s crate docs state) |
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
//! - **A `.cacheable()` the kernel downgraded.** The kernel returns the
//!   *effective* expiry (the least cacheable of the result and its dependencies),
//!   so an endpoint that marked its result cacheable over a volatile dependency
//!   comes back indistinguishable from one that never marked it. No published core
//!   API exposes the declared expiry; [`Suite::cacheable`] is the declaration that
//!   closes the gap and makes that recomputation a finding.
//! - **Purity and threads** (see above): an empty thread set is a finding until
//!   declared pure. The declaration is the mechanism; the judgment is still the
//!   author's.
//! - **Pipeline routing for `Source`.** The engine routes a piped value into the
//!   sole unnamed required input by contract; nothing an endpoint does makes that
//!   right or wrong, so only the mutating verbs' `content` rule and "declares
//!   `content` ⇒ reads it" are checked. Newline-separated list output (the `..`
//!   map convention) is a shape no ArgSpec states.
//! - **A declared golden thread is a promise a host must keep.** [`Cacheable`](Check::Cacheable)
//!   checks the thread set is non-empty, not that anything ever cuts a thread in
//!   it; a module declaring `urn:file:` threads over a config home no host watches
//!   is clean here and stale in production. The suite cannot see the host.
//! - **"Required" that is actually optional.** [`ArgSpecs`](Check::ArgSpecs) reads
//!   the declaration; the minimal call supplies every required input, so an input
//!   marked required that the endpoint would happily do without is invisible.
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
//! # Fixtures, opt-outs, namespaces
//!
//! The invoking checks resolve each action with the smallest inputs its ArgSpecs
//! admit ([`Suite::fixture`] when the spec cannot say — a path that must exist, a
//! document that must parse). They run **against the kernel you pass**, so build it
//! as a test fixture; an action with real side effects opts out by id with a
//! reason ([`Suite::opt_out`]), and the opt-out list, the fixtures, and every other
//! declaration are part of the report. A module that serves its own vocabulary
//! registers the namespace ([`Suite::namespace`]).
//!
//! What a walk fires, under root: a `Source` or `Exists` is resolved once (twice
//! when cacheable — the second is the cache probe); each declared RDF face beyond
//! the first is resolved once more with `as=`; a `Sink` or `Delete` declaring
//! `content` is fired **once**, by the pipeline probe, and [`Outputs`](Check::Outputs)
//! reads that same firing; a mutating action without `content` is never fired
//! under root ([`Enforced`](Check::Enforced) runs under no grants) and is listed as
//! unprobed. Fixture bindings are per entry, not per verb (see [`Fixture::binding`]).
//!
//! The kernel's own `urn:kernel:*` operations are listed by [`Kernel::entries`] but
//! are core's, not the module's; the walk skips them unless
//! [`Suite::include_kernel_ops`] says otherwise.
#![forbid(unsafe_code)]

mod checks;
pub mod rdf;
mod report;
mod suite;

pub use checks::{Check, Checks};
pub use report::{Declarations, Finding, OptedOut, Report, Unprobed};
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
///     .bind(Exact::new("urn:example:reverseList"), builtins::reverse_list());
/// let report = check(&Kernel::new(Arc::new(root))).unwrap_err();
/// // The builtins predate the recipe: an untyped input, a pre-convention id, and —
/// // being pure functions nobody declared pure — a cacheable result with no thread.
/// assert!(report.of(Check::ArgSpecs).count() >= 1);
/// assert!(report.of(Check::Names).count() == 1);
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

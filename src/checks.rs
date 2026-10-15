//! The checks, individually selectable so a module can adopt incrementally.

use std::fmt;
use std::ops::{BitOr, Sub};

/// Declares [`Check`] and derives [`Check::ALL`] from the same variant list, so a
/// variant cannot exist without being selectable. A hand-kept `ALL` drifted silently:
/// a variant left out of it compiled, passed every test, and was never selected by
/// [`Checks::all`] nor listed by [`Checks::skipped`] (ledger #1035).
macro_rules! declare_checks {
    (
        $(#[$meta:meta])*
        pub enum Check {
            $( $(#[$vmeta:meta])* $variant:ident, )+
        }
    ) => {
        $(#[$meta])*
        pub enum Check {
            $( $(#[$vmeta])* $variant, )+
        }

        impl Check {
            /// Every check, in report order: declaration order, because this array is
            /// generated from the declaration.
            pub const ALL: [Check; [$(Check::$variant),+].len()] = [$(Check::$variant),+];
        }
    };
}

declare_checks! {
/// One rule of the module recipe, as a check. Each names the recipe row it
/// mechanizes; the crate docs state what each can and cannot see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Check {
    /// **ArgSpecs from day one.** Every description declares at least one action;
    /// every declared input has a `class`; a `default` is one of the `one_of` values
    /// when both exist; names are unique per action; every template variable is a
    /// declared binding input.
    ArgSpecs,
    /// **`requires` implies a verb.** A description carrying `requires` and no verb
    /// (other than `Meta`): since ikigai-core 0.1.85 the kernel enforces it on every verb
    /// (before, `action_specs()` iterated verbs and the scope was silently inert), but the
    /// catalog still cannot say which actions it gates, so declare the verb.
    RequiresVerb,
    /// **Declared = enforced, the half a test can see.** Every action with a `requires`
    /// is refused with a typed `Denied` under a capability holding no grants; an
    /// action declaring nothing is not. That refusal is an undeclared gate, and the
    /// finding names both things it can be, since the fixes differ (0.8.4, ledger
    /// #362): a capability the action enforces (declare it), or a STATE no capability
    /// expresses, such as a window another resource opens (document it and waive).
    ///
    /// The no-grants probe proves the KERNEL's floor: a declared `urn:cap:net:*` with
    /// no grant under the prefix is refused before the endpoint runs. A module's own
    /// rule (per host, per name, per prefix) is reached only by a caller holding some
    /// grant under the prefix but not this target's, which a module names with
    /// [`Suite::enforced_under`](crate::Suite::enforced_under) (opt-in, ledger #331); the
    /// action must refuse it with a typed `Denied`.
    Enforced,
    /// **A mutation no authority gated.** The fourth cell of the same 2x2
    /// [`Enforced`](Check::Enforced) walks: a `Sink` or a `Delete` that declares no
    /// `requires` and **resolved anyway** under a capability holding no grants. The
    /// write happened for a caller who held nothing, so there is no scope to withhold
    /// from anyone — read and write cannot be told apart on that action, and no
    /// caller can be given one without the other.
    ///
    /// A `Source` is not in scope: a public read is a decision a module gets to make.
    /// Evidence, not declaration — a mutating action that declares nothing and is
    /// refused for some *other* reason is recorded as unprobed, never as a finding,
    /// because what an ungranted caller can do through it was not observed.
    Authority,
    /// **Declared outputs = served outputs.** The bare media type of the minimal
    /// resolution (parameters such as `;charset=` stripped) is one of the action's
    /// declared `outputs`. A wrong declaration hides a face from every consumer that
    /// reads outputs — [`SkolemRdf`](Check::SkolemRdf) and
    /// [`Vocabulary`](Check::Vocabulary) included, which filter the declaration for
    /// RDF faces before probing. What the check could not observe (a mutating
    /// action never fired under root, a failed minimal resolution, a caller's `as=`
    /// label) is listed in the report as unprobed, never as a finding.
    Outputs,
    /// **Skolemize; no blank nodes.** Every declared RDF face resolves, parses, and
    /// carries no blank node.
    SkolemRdf,
    /// **Vocabulary terms only.** Every predicate and class in an RDF face is defined
    /// in `ikigai-vocab`, or lives under a well-known or module-registered namespace.
    ///
    /// It **parses the face** to see them
    /// ([`rdf::parse`](crate::rdf::parse)), so it also reports a face that is served
    /// as something other than what it declared, or is malformed — under its own name
    /// when [`SkolemRdf`](Check::SkolemRdf) is not selected. (A face that does not
    /// resolve at all is [`Resolves`](Check::Resolves)'s, when that is selected.) A
    /// module can therefore rely on `VOCABULARY` alone to prove a hand-written
    /// `@prefix` line is well-formed. The oracle is `ikigai_vocab::VOCABULARY` at
    /// the version cargo resolves for the whole graph: a term this crate's pin is
    /// too old to know is reported as invented (see the crate docs' vocabulary-pin
    /// note).
    Vocabulary,
    /// **The cacheable-twice probe.** A result marked cacheable is served from the
    /// cache the second time, byte-identical, and — unless the endpoint is declared
    /// pure or takes writes through its own name — depends on at least one golden
    /// thread besides the one the kernel hangs it on (its own name, core ≥ 0.1.73).
    /// That last rule is about a result served FOREVER, so it holds only
    /// `Expiry::Never`: an `Expiry::At` deadline is a bound in time, whatever its length.
    /// Both polarities are declarations the module makes: [`Suite::cacheable`](crate::Suite::cacheable)
    /// holds an endpoint to being cacheable, [`Suite::live`](crate::Suite::live) to
    /// being `Expiry::Always`. An endpoint declaring neither is held to neither.
    /// And a [`Suite::pure`](crate::Suite::pure) is held to its claim: a cacheable
    /// result of an endpoint declared pure carries no thread but its own name and
    /// what it read (the name, and the threads, of each resource it resolved).
    ///
    /// Its second resolution is the cache hit, so "byte-identical" compares the first
    /// computation with a copy of itself; [`Recompute`](Check::Recompute) is the
    /// witness that compares two computations.
    Cacheable,
    /// **Pipeline citizenship.** A mutating action with by-value inputs declares
    /// `content` (where a pipe's value arrives), and an action declaring `content`
    /// reads it. On a `Sink`, any by-value input counts, required or not (0.8.4, ledger
    /// #339): a Sink whose inputs are all optional is the one a pipe alone most often
    /// drives, and before 0.8.4 it was the one the rule skipped. A `Delete` is held to
    /// it only when an input is required: its optional inputs (a `reason`) are metadata.
    Pipeline,
    /// **Required is required.** Each input an action declares required, by value and
    /// with no default, is dropped from the call the walk resolved, one at a time, and
    /// the call must then be refused with a typed `MissingArgument` or
    /// `InvalidArgument`. A call that SUCCEEDS without it means "declared required,
    /// actually optional", a false negative in the selection funnel:
    /// `urn:kernel:validate` and an MCP tool schema both refuse a valid call that
    /// omits it. A refusal of another kind is a finding too (an agent cannot tell its
    /// own omission from a failure of the endpoint), unless it is transient.
    ///
    /// One extra resolution per required input. `Source` and `Exists` by default;
    /// `Sink` and `Delete` only under
    /// [`Suite::required_on_mutating`](crate::Suite::required_on_mutating), because
    /// each drop is one more firing of a mutating action and one that SUCCEEDS has
    /// mutated. When the call it drops from does not resolve, nothing is observed and
    /// the action is listed as unprobed.
    Required,
    /// **Naming convention.** A description id is a short noun in `kebab-case`
    /// (the convention `ikigai-core`'s crate docs state).
    Names,
    /// **A description core itself would refuse.** `Description::validate()` refuses
    /// the description: an id, an input name, an input `class` or a `requires` holding a
    /// character an RDF `IRIREF` cannot carry (one of ``<>"{}|^`\``, a space, a
    /// control character), or a NAME holding a code point no IRI carries (DEL, a C1 control,
    /// private use, a noncharacter; ikigai-core 0.1.99). Since ikigai-core 0.1.100 the
    /// emitters encode such a name and degrade such an authored IRI to a literal, so the
    /// endpoint's node, its contract IRIs and its `Meta` Turtle still parse, but no longer
    /// say what was written. This is the strict check on top of that encoding: it refuses
    /// the authoring mistake rather than keeping a document whole.
    ///
    /// It lives here, not in `bind`: binding stays infallible and does not validate, so a
    /// bad name is caught where it is AUTHORED, by the suite every module's CI already
    /// runs. It is not [`Names`](Check::Names), which is a convention a module may waive;
    /// this one is a mistake core refuses, not a matter of style. It reports the refusal,
    /// never its error variant, and `validate` stops at the first refusal, so a
    /// description with two bad names shows the second once the first is fixed.
    Validate,
    /// **A name is a claim: same name, same doors.** A module states what kind of
    /// space its constructor builds, and the check holds it to that
    /// ([`Suite::self_named_space`](crate::Suite::self_named_space),
    /// [`Suite::host_named_space`](crate::Suite::host_named_space)):
    ///
    /// - **self-named** (a configuration-free `space()`): `id()` is
    ///   [`space_iri`](ikigai_core::space_iri)`(module)`, under
    ///   [`SPACE_PREFIX`](ikigai_core::SPACE_PREFIX); two calls claim the same name
    ///   and hold the same doors (equal `topology()` and equal `entries()`); the
    ///   topology's root node carries the name. Two declarations under one name hold
    ///   the same doors too.
    /// - **host-named** (an instance-built or parameterized constructor): `id()` is
    ///   `None` and the topology root is anonymous, so the module cannot claim a name
    ///   for doors that depend on what it was handed. The host names it.
    ///
    /// It builds spaces and reads them; it resolves nothing, and it never sees the
    /// kernel the walk is given. A module that declares no space is held to nothing,
    /// and the report says so on a `space:` line.
    SpaceName,
    /// **Every declaration reached the check that would honor it.** A
    /// [`Suite`](crate::Suite) declaration — `live`, `cacheable`, `pure`,
    /// `namespace`, a [`Fixture`](crate::Fixture), an
    /// [`opt_out`](crate::Suite::opt_out), an
    /// [`opt_out_check`](crate::Suite::opt_out_check) or a declared space — that
    /// the walk never consulted is reported, because a declaration printed in a clean report reads
    /// as a check that ran.
    ///
    /// The hole it closes: `Suite::live("notes-write")` on a `Sink` did nothing at
    /// all (`CACHEABLE` returns early on a non-cacheable verb), and the report said
    /// `declared live: notes-write` anyway. Same shape for a fixture whose id is a
    /// typo, a fixture argument naming no declared input, a waiver for an endpoint nothing binds, and a namespace no probed face
    /// used.
    ///
    /// This check reads declarations against what the walk covered; it invokes
    /// nothing. Subtract it ([`Checks::DECLARATIONS`]) for a suite that keeps
    /// standing declarations on purpose — a `live` an endpoint will earn next
    /// release, say — and say why in the same place you would say it to a reviewer.
    Declarations,
    /// **The resolution a check needed did not resolve** (0.8.4, ledger #338). The
    /// invoking checks that judge an answer (`OUTPUTS`, `SKOLEM-RDF`, `VOCABULARY`,
    /// `CACHEABLE`) all read one resolution of the action with its minimal inputs (a
    /// declared RDF face's own `as=` resolution, for a description with several faces;
    /// the pipeline probe's firing, for a `Sink`). When it is refused, nothing else can
    /// be judged, and the refusal is reported ONCE per action under this label, with
    /// the refusal's kind, its text, the check that needed it, and, where the walk had
    /// to invent a value for an input that declares no `class`, that input by name:
    ///
    /// ```text
    /// sparql  RESOLVES  source: did not resolve with the minimal inputs its ArgSpecs allow (refused as `invalid-argument`: …); supply a … Needed by SKOLEM-RDF, and by every check that reads the same resolution. The walk had no value to send for `query`: it declares no class, so it sent the placeholder `x` (the ARGSPECS finding on this action). …
    /// ```
    ///
    /// Before 0.8.4 the same failure was reported under whichever of those checks
    /// reached it first, so it read as a caching finding on an endpoint whose caching
    /// was fine, and two adopters describing one defect named different checks.
    ///
    /// It fires nothing of its own: it reports a resolution another selected check
    /// needed, so it reports nothing for an action no such check runs on (waive
    /// `OUTPUTS` on a `Sink` whose target this kernel does not bind, and nothing is
    /// said). Waive it per endpoint like any check. **Not selected** (a selection
    /// written before 0.8.4 never names it), the failure is reported under the check
    /// that needed it, exactly as before.
    Resolves,
    /// **A cacheable answer is a function of its inputs, by a forced recomputation**
    /// (ledger #330). [`Cacheable`](Check::Cacheable)'s second resolution is the cache
    /// hit, so it can only repeat the first: `ikigai-encrypt` shipped `.cacheable()` on
    /// age ciphertext (a fresh ephemeral key per call) and the walk said nothing. For an
    /// `Expiry::Never` answer the cache holds, this check cuts every thread the answer
    /// carries and the name of everything it read, so the next resolution is a
    /// genuinely independent computation, and holds its bytes to the first one's. It
    /// does that 24 times, every computation held to the first, because one agrees with
    /// the first by chance as often as an order over few entries does: a two-entry
    /// `HashMap` agrees about half the time, so a single recomputation missed it about
    /// one walk in two (ledger #1573). Then it cuts each thread an endpoint DECLARED
    /// (`depends_on`) alone, and does the same once. A fresh nonce, key, timestamp or
    /// random id, or a `HashMap` iteration order, in a cacheable answer is caught here.
    /// An order is only caught if the fixture gives it something to order: a map with
    /// one entry serializes the same way every time, so a fixture should have enough
    /// entries (nodes, statements, keys) for order to matter, and three or more beat two.
    ///
    /// An `Expiry::At` answer is not held to it: a deadline makes the answer a function
    /// of its inputs AND the time, so a recomputation may differ by design. An answer
    /// still cached right after a cut (a host's cut listener can refill it) is listed
    /// as unprobed, never a finding. An answer whose recomputations all repeated the
    /// first is a `recomputed:` line.
    ///
    /// It costs 24 resolutions per cacheable answer, plus one per declared thread, and
    /// each cut re-reads what the answer was built from: a test that pins how often a
    /// fixture is READ counts those too. Subtract it suite-wide
    /// (`Checks::all() - Checks::RECOMPUTE`) for a pinned footprint, or waive it per
    /// endpoint with the reason an answer differs by design
    /// ([`Suite::opt_out_check`](crate::Suite::opt_out_check)). A selection that does not
    /// name it (any written before it existed) does not run it.
    Recompute,
}
}

impl Check {
    /// The short upper-case label a report line carries.
    pub fn label(self) -> &'static str {
        match self {
            Check::ArgSpecs => "ARGSPECS",
            Check::RequiresVerb => "REQUIRES-VERB",
            Check::Enforced => "ENFORCED",
            Check::Authority => "AUTHORITY",
            Check::Outputs => "OUTPUTS",
            Check::SkolemRdf => "SKOLEM-RDF",
            Check::Vocabulary => "VOCABULARY",
            Check::Cacheable => "CACHEABLE",
            Check::Pipeline => "PIPELINE",
            Check::Required => "REQUIRED",
            Check::Names => "NAMES",
            Check::Validate => "VALIDATE",
            Check::SpaceName => "SPACE-NAME",
            Check::Declarations => "DECLARATIONS",
            Check::Resolves => "RESOLVES",
            Check::Recompute => "RECOMPUTE",
        }
    }

    /// Whether this check resolves endpoints (and so respects opt-outs), as
    /// opposed to reading descriptions only. [`Checks::STATIC`] is every check for
    /// which this is `false`.
    pub const fn invokes(self) -> bool {
        matches!(
            self,
            Check::Enforced
                | Check::Authority
                | Check::Outputs
                | Check::SkolemRdf
                | Check::Vocabulary
                | Check::Cacheable
                | Check::Pipeline
                | Check::Required
                | Check::Resolves
                | Check::Recompute
        )
    }

    fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A set of [`Check`]s. Combine with `|`, remove with `-`:
///
/// ```
/// use ikigai_conformance::{Check, Checks};
///
/// let all = Checks::all();
/// assert!(all.contains(Check::SkolemRdf));
///
/// // Adopt incrementally: everything but the RDF checks.
/// let some = Checks::all() - Checks::RDF;
/// assert!(!some.contains(Check::SkolemRdf));
/// assert!(!some.contains(Check::Vocabulary));
/// assert!(some.contains(Check::ArgSpecs));
///
/// // Or opt in one at a time.
/// let one = Checks::ARGSPECS | Checks::NAMES;
/// assert_eq!(one.iter().count(), 2);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Checks(u16);

impl Checks {
    /// [`Check::ArgSpecs`].
    pub const ARGSPECS: Checks = Checks(1 << (Check::ArgSpecs as u16));
    /// [`Check::RequiresVerb`].
    pub const REQUIRES_VERB: Checks = Checks(1 << (Check::RequiresVerb as u16));
    /// [`Check::Enforced`].
    pub const ENFORCED: Checks = Checks(1 << (Check::Enforced as u16));
    /// [`Check::Authority`].
    pub const AUTHORITY: Checks = Checks(1 << (Check::Authority as u16));
    /// [`Check::Outputs`].
    pub const OUTPUTS: Checks = Checks(1 << (Check::Outputs as u16));
    /// [`Check::SkolemRdf`].
    pub const SKOLEM_RDF: Checks = Checks(1 << (Check::SkolemRdf as u16));
    /// [`Check::Vocabulary`].
    pub const VOCABULARY: Checks = Checks(1 << (Check::Vocabulary as u16));
    /// [`Check::Cacheable`].
    pub const CACHEABLE: Checks = Checks(1 << (Check::Cacheable as u16));
    /// [`Check::Pipeline`].
    pub const PIPELINE: Checks = Checks(1 << (Check::Pipeline as u16));
    /// [`Check::Required`].
    pub const REQUIRED: Checks = Checks(1 << (Check::Required as u16));
    /// [`Check::Names`].
    pub const NAMES: Checks = Checks(1 << (Check::Names as u16));
    /// [`Check::Validate`].
    pub const VALIDATE: Checks = Checks(1 << (Check::Validate as u16));
    /// [`Check::SpaceName`].
    pub const SPACE_NAME: Checks = Checks(1 << (Check::SpaceName as u16));
    /// [`Check::Declarations`].
    pub const DECLARATIONS: Checks = Checks(1 << (Check::Declarations as u16));
    /// [`Check::Resolves`].
    pub const RESOLVES: Checks = Checks(1 << (Check::Resolves as u16));
    /// [`Check::Recompute`]. Sixteen checks fill the `u16` this set is: the next one
    /// widens it (a private field, so no consumer sees the change).
    pub const RECOMPUTE: Checks = Checks(1 << (Check::Recompute as u16));
    /// Both RDF-face checks: [`Check::SkolemRdf`] and [`Check::Vocabulary`] — the
    /// pair a module without a graph face subtracts.
    pub const RDF: Checks = Checks(Checks::SKOLEM_RDF.0 | Checks::VOCABULARY.0);
    /// Every check that reads descriptions and declarations only, and resolves
    /// nothing: the safe FIRST run over a kernel whose endpoints spawn processes,
    /// reach the network or call a model (ledger #348). A walk under these resolves
    /// no endpoint, under root or under no grants; the report says so on its
    /// `probed: nothing` line. Derived from [`Check::invokes`], so a new check lands
    /// on the right side without anyone remembering to put it there.
    ///
    /// ```
    /// use ikigai_conformance::{Check, Checks};
    ///
    /// assert!(Checks::STATIC.contains(Check::ArgSpecs));
    /// assert!(Checks::STATIC.contains(Check::Declarations));
    /// assert!(!Checks::STATIC.contains(Check::Enforced));
    /// assert!(!Checks::STATIC.contains(Check::Pipeline)); // its probe fires a Sink
    /// assert_eq!(Checks::STATIC | Checks::INVOKING, Checks::all());
    /// ```
    pub const STATIC: Checks = Checks::selecting(false);
    /// Every check that resolves endpoints: the complement of [`STATIC`](Self::STATIC).
    pub const INVOKING: Checks = Checks::selecting(true);

    /// The checks whose [`Check::invokes`] is `invoking`.
    const fn selecting(invoking: bool) -> Checks {
        let mut bits = 0u16;
        let mut i = 0;
        while i < Check::ALL.len() {
            let check = Check::ALL[i];
            if check.invokes() == invoking {
                bits |= 1 << (check as u16);
            }
            i += 1;
        }
        Checks(bits)
    }

    /// Every check.
    pub fn all() -> Checks {
        Check::ALL
            .iter()
            .fold(Checks(0), |acc, c| Checks(acc.0 | c.bit()))
    }

    /// No check (build up with `|`).
    pub fn none() -> Checks {
        Checks(0)
    }

    /// Whether `check` is selected.
    pub fn contains(self, check: Check) -> bool {
        self.0 & check.bit() != 0
    }

    /// The selected checks, in report order.
    pub fn iter(self) -> impl Iterator<Item = Check> {
        Check::ALL.into_iter().filter(move |c| self.contains(*c))
    }

    /// The checks NOT selected, in report order — what a report prints as skipped.
    pub fn skipped(self) -> impl Iterator<Item = Check> {
        Check::ALL.into_iter().filter(move |c| !self.contains(*c))
    }
}

impl From<Check> for Checks {
    fn from(check: Check) -> Self {
        Checks(check.bit())
    }
}

impl BitOr for Checks {
    type Output = Checks;
    fn bitor(self, rhs: Checks) -> Checks {
        Checks(self.0 | rhs.0)
    }
}

impl BitOr<Check> for Checks {
    type Output = Checks;
    fn bitor(self, rhs: Check) -> Checks {
        Checks(self.0 | rhs.bit())
    }
}

impl Sub for Checks {
    type Output = Checks;
    fn sub(self, rhs: Checks) -> Checks {
        Checks(self.0 & !rhs.0)
    }
}

impl Sub<Check> for Checks {
    type Output = Checks;
    fn sub(self, rhs: Check) -> Checks {
        Checks(self.0 & !rhs.bit())
    }
}

impl fmt::Debug for Checks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_is_every_variant_in_declaration_order() {
        // `ALL` is generated from the enum (`declare_checks!`), so it cannot omit a
        // variant. This pins what report order and `Checks` rely on beyond that: no
        // duplicate, position = discriminant (the bit a `Checks` holds), and every
        // discriminant fits the `u16` bitset.
        for (i, c) in Check::ALL.iter().enumerate() {
            assert_eq!(*c as usize, i, "{c} is at {i} in Check::ALL");
        }
        assert!(Check::ALL.len() <= u16::BITS as usize);
    }

    #[test]
    fn all_selects_every_check_and_none_selects_nothing() {
        for c in Check::ALL {
            assert!(Checks::all().contains(c));
            assert!(!Checks::none().contains(c));
        }
        assert_eq!(Checks::all().iter().count(), Check::ALL.len());
        assert_eq!(Checks::all().skipped().count(), 0);
    }

    #[test]
    fn subtraction_removes_and_skipped_reports_it() {
        let without = Checks::all() - Check::Cacheable;
        assert!(!without.contains(Check::Cacheable));
        assert_eq!(
            without.skipped().collect::<Vec<_>>(),
            vec![Check::Cacheable]
        );
        assert_eq!((Checks::all() - Checks::RDF).skipped().count(), 2);
    }

    #[test]
    fn the_declarations_check_reads_declarations_and_invokes_nothing() {
        assert!(!Check::Declarations.invokes());
        assert!(Checks::all().contains(Check::Declarations));
        assert!(!(Checks::all() - Checks::DECLARATIONS).contains(Check::Declarations));
    }

    #[test]
    fn labels_are_distinct() {
        let mut labels: Vec<&str> = Check::ALL.iter().map(|c| c.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Check::ALL.len());
    }

    #[test]
    fn every_check_has_its_own_bit() {
        // Fourteen checks do not fit a u8; a shared bit would make one check
        // select another silently.
        let mut bits: Vec<u16> = Check::ALL.iter().map(|c| c.bit()).collect();
        bits.sort_unstable();
        bits.dedup();
        assert_eq!(bits.len(), Check::ALL.len());
        assert_eq!(
            Checks::OUTPUTS.iter().collect::<Vec<_>>(),
            vec![Check::Outputs]
        );
    }
}

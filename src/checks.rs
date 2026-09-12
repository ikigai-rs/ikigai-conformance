//! The checks, individually selectable so a module can adopt incrementally.

use std::fmt;
use std::ops::{BitOr, Sub};

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
    /// (other than `Meta`) declares a floor the kernel never enforces: `action_specs()`
    /// iterates verbs, so the scope is silently inert.
    RequiresVerb,
    /// **Declared = enforced, the half a test can see.** Every action with a `requires`
    /// is refused with a typed `Denied` under a capability holding no grants; an
    /// action declaring nothing is not.
    Enforced,
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
    /// ([`rdf::parse`](crate::rdf::parse)), so it also reports a face that does not
    /// resolve, is served as something other than what it declared, or is malformed
    /// — under its own name when [`SkolemRdf`](Check::SkolemRdf) is not selected. A
    /// module can therefore rely on `VOCABULARY` alone to prove a hand-written
    /// `@prefix` line is well-formed. The oracle is `ikigai_vocab::VOCABULARY` at
    /// the version cargo resolves for the whole graph: a term this crate's pin is
    /// too old to know is reported as invented (see the crate docs' vocabulary-pin
    /// note).
    Vocabulary,
    /// **The cacheable-twice probe.** A result marked cacheable is served from the
    /// cache the second time, byte-identical, and — unless the endpoint is declared
    /// pure — depends on at least one golden thread. Both polarities are
    /// declarations the module makes: [`Suite::cacheable`](crate::Suite::cacheable)
    /// holds an endpoint to being cacheable, [`Suite::live`](crate::Suite::live) to
    /// being `Expiry::Always`. An endpoint declaring neither is held to neither.
    Cacheable,
    /// **Pipeline citizenship.** A mutating action with by-value inputs declares
    /// `content` (where a pipe's value arrives), and an action declaring `content`
    /// reads it.
    Pipeline,
    /// **Naming convention.** A description id is a short noun in `kebab-case`
    /// (the convention `ikigai-core`'s crate docs state).
    Names,
    /// **Every declaration reached the check that would honour it.** A
    /// [`Suite`](crate::Suite) declaration — `live`, `cacheable`, `pure`,
    /// `namespace`, a [`Fixture`](crate::Fixture), an
    /// [`opt_out`](crate::Suite::opt_out) or an
    /// [`opt_out_check`](crate::Suite::opt_out_check) — that the walk never
    /// consulted is reported, because a declaration printed in a clean report reads
    /// as a check that ran.
    ///
    /// The hole it closes: `Suite::live("notes-write")` on a `Sink` did nothing at
    /// all (`CACHEABLE` returns early on a non-cacheable verb), and the report said
    /// `declared live: notes-write` anyway. Same shape for a fixture whose id is a
    /// typo, a waiver for an endpoint nothing binds, and a namespace no probed face
    /// used.
    ///
    /// This check reads declarations against what the walk covered; it invokes
    /// nothing. Subtract it ([`Checks::DECLARATIONS`]) for a suite that keeps
    /// standing declarations on purpose — a `live` an endpoint will earn next
    /// release, say — and say why in the same place you would say it to a reviewer.
    Declarations,
}

impl Check {
    /// Every check, in report order.
    pub const ALL: [Check; 10] = [
        Check::ArgSpecs,
        Check::RequiresVerb,
        Check::Enforced,
        Check::Outputs,
        Check::SkolemRdf,
        Check::Vocabulary,
        Check::Cacheable,
        Check::Pipeline,
        Check::Names,
        Check::Declarations,
    ];

    /// The short upper-case label a report line carries.
    pub fn label(self) -> &'static str {
        match self {
            Check::ArgSpecs => "ARGSPECS",
            Check::RequiresVerb => "REQUIRES-VERB",
            Check::Enforced => "ENFORCED",
            Check::Outputs => "OUTPUTS",
            Check::SkolemRdf => "SKOLEM-RDF",
            Check::Vocabulary => "VOCABULARY",
            Check::Cacheable => "CACHEABLE",
            Check::Pipeline => "PIPELINE",
            Check::Names => "NAMES",
            Check::Declarations => "DECLARATIONS",
        }
    }

    /// Whether this check resolves endpoints (and so respects opt-outs), as
    /// opposed to reading descriptions only.
    pub fn invokes(self) -> bool {
        matches!(
            self,
            Check::Enforced
                | Check::Outputs
                | Check::SkolemRdf
                | Check::Vocabulary
                | Check::Cacheable
                | Check::Pipeline
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
    /// [`Check::Names`].
    pub const NAMES: Checks = Checks(1 << (Check::Names as u16));
    /// [`Check::Declarations`].
    pub const DECLARATIONS: Checks = Checks(1 << (Check::Declarations as u16));
    /// Both RDF-face checks: [`Check::SkolemRdf`] and [`Check::Vocabulary`] — the
    /// pair a module without a graph face subtracts.
    pub const RDF: Checks = Checks(Checks::SKOLEM_RDF.0 | Checks::VOCABULARY.0);

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
        // Ten checks no longer fit a u8; a shared bit would make one check select
        // another silently.
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

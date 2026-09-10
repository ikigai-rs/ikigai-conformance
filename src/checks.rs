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
    /// **Skolemize; no blank nodes.** Every declared RDF face resolves, parses, and
    /// carries no blank node.
    SkolemRdf,
    /// **Vocabulary terms only.** Every predicate and class in an RDF face is defined
    /// in `ikigai-vocab`, or lives under a well-known or module-registered namespace.
    Vocabulary,
    /// **The cacheable-twice probe.** A result marked cacheable is served from the
    /// cache the second time, byte-identical, and — unless the endpoint is declared
    /// pure — depends on at least one golden thread.
    Cacheable,
    /// **Pipeline citizenship.** A mutating action with by-value inputs declares
    /// `content` (where a pipe's value arrives), and an action declaring `content`
    /// reads it.
    Pipeline,
    /// **Naming convention.** A description id is a short noun in `kebab-case`
    /// (the convention `ikigai-core`'s crate docs state).
    Names,
}

impl Check {
    /// Every check, in report order.
    pub const ALL: [Check; 8] = [
        Check::ArgSpecs,
        Check::RequiresVerb,
        Check::Enforced,
        Check::SkolemRdf,
        Check::Vocabulary,
        Check::Cacheable,
        Check::Pipeline,
        Check::Names,
    ];

    /// The short upper-case label a report line carries.
    pub fn label(self) -> &'static str {
        match self {
            Check::ArgSpecs => "ARGSPECS",
            Check::RequiresVerb => "REQUIRES-VERB",
            Check::Enforced => "ENFORCED",
            Check::SkolemRdf => "SKOLEM-RDF",
            Check::Vocabulary => "VOCABULARY",
            Check::Cacheable => "CACHEABLE",
            Check::Pipeline => "PIPELINE",
            Check::Names => "NAMES",
        }
    }

    /// Whether this check resolves endpoints (and so respects opt-outs), as
    /// opposed to reading descriptions only.
    pub fn invokes(self) -> bool {
        matches!(
            self,
            Check::Enforced | Check::SkolemRdf | Check::Vocabulary | Check::Cacheable
        ) || self == Check::Pipeline
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
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
pub struct Checks(u8);

impl Checks {
    /// [`Check::ArgSpecs`].
    pub const ARGSPECS: Checks = Checks(1 << (Check::ArgSpecs as u8));
    /// [`Check::RequiresVerb`].
    pub const REQUIRES_VERB: Checks = Checks(1 << (Check::RequiresVerb as u8));
    /// [`Check::Enforced`].
    pub const ENFORCED: Checks = Checks(1 << (Check::Enforced as u8));
    /// [`Check::SkolemRdf`].
    pub const SKOLEM_RDF: Checks = Checks(1 << (Check::SkolemRdf as u8));
    /// [`Check::Vocabulary`].
    pub const VOCABULARY: Checks = Checks(1 << (Check::Vocabulary as u8));
    /// [`Check::Cacheable`].
    pub const CACHEABLE: Checks = Checks(1 << (Check::Cacheable as u8));
    /// [`Check::Pipeline`].
    pub const PIPELINE: Checks = Checks(1 << (Check::Pipeline as u8));
    /// [`Check::Names`].
    pub const NAMES: Checks = Checks(1 << (Check::Names as u8));
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
    fn labels_are_distinct() {
        let mut labels: Vec<&str> = Check::ALL.iter().map(|c| c.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Check::ALL.len());
    }
}

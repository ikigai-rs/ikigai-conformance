//! The report: every finding at once, one line each, endpoint id first — so a
//! failure is a checklist, not the first miss.

use std::fmt;

use ikigai_core::Verb;

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

/// One report line: `<endpoint>  <CHECK>  [<verb>:] <detail>`.
impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}  ", self.endpoint, self.check.label())?;
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
/// assert!(first.starts_with("toUpper  "), "{first}");
/// assert!(report.findings.iter().any(|f| f.check == Check::ArgSpecs
///     && f.detail.contains("input `in` has no class")));
/// assert_eq!(report.endpoints, 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Every finding, in walk order (endpoints in catalog order, checks in
    /// [`Check::ALL`] order within an endpoint).
    pub findings: Vec<Finding>,
    /// How many descriptions were walked: distinct description ids, so two
    /// patterns binding one endpoint (`urn:a11y:config` and
    /// `urn:a11y:config:{app}`) count once.
    pub endpoints: usize,
    /// How many actions were examined: one per bound ENTRY per verb, so the two
    /// patterns above count their verbs twice — each is a place the action can be
    /// reached, with its own template variables.
    pub actions: usize,
    /// The checks that ran.
    pub checks: Checks,
    /// What the module declared to the suite — echoed so the report says what was
    /// waived, not only what was found. Boxed: `Report` is the `Err` of
    /// [`check`](crate::check) and must stay small enough to return by value.
    pub declared: Box<Declarations>,
    /// The actions a check could not observe, with reasons — the gaps in the
    /// walk's coverage, printed beside the findings (`unprobed: …`).
    pub unprobed: Vec<Unprobed>,
}

/// What a module declared when it configured the [`Suite`](crate::Suite),
/// echoed in the [`Report`]: every waiver is printed beside the findings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declarations {
    /// The actions the module opted out of the invoking checks, with reasons.
    pub opted_out: Vec<OptedOut>,
    /// The endpoints the module declared pure (exempt from the golden-thread half
    /// of [`Check::Cacheable`]).
    pub pure: Vec<String>,
    /// The endpoints the module declared cacheable (held to it by
    /// [`Check::Cacheable`]).
    pub cacheable: Vec<String>,
    /// The namespaces the module registered as its own for [`Check::Vocabulary`].
    pub namespaces: Vec<String>,
    /// The fixtures the module supplied, printed one per line (`fixture: file
    /// source path="README.md"`) so a reader can tell which inputs a walk ran over.
    pub fixtures: Vec<Fixture>,
}

impl Report {
    /// `true` when no check found anything.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
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
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        let ran: Vec<&str> = self.checks.iter().map(Check::label).collect();
        writeln!(f, "checked: {}", ran.join(" "))?;
        let skipped: Vec<&str> = self.checks.skipped().map(Check::label).collect();
        if !skipped.is_empty() {
            writeln!(f, "skipped: {}", skipped.join(" "))?;
        }
        let declared = &self.declared;
        for out in &declared.opted_out {
            match out.verb {
                Some(verb) => writeln!(
                    f,
                    "opted out: {} {}: {}",
                    out.endpoint,
                    verb_name(verb),
                    out.reason
                )?,
                None => writeln!(f, "opted out: {}: {}", out.endpoint, out.reason)?,
            }
        }
        if !declared.pure.is_empty() {
            writeln!(f, "declared pure: {}", declared.pure.join(" "))?;
        }
        if !declared.cacheable.is_empty() {
            writeln!(f, "declared cacheable: {}", declared.cacheable.join(" "))?;
        }
        if !declared.namespaces.is_empty() {
            writeln!(f, "module namespaces: {}", declared.namespaces.join(" "))?;
        }
        for fixture in &declared.fixtures {
            writeln!(f, "fixture: {fixture}")?;
        }
        for u in &self.unprobed {
            writeln!(
                f,
                "unprobed: {} {} {}: {}",
                u.endpoint,
                verb_name(u.verb),
                u.check.label(),
                u.reason
            )?;
        }
        Ok(())
    }
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
    fn the_report_prints_skipped_checks_and_opt_outs() {
        let report = Report {
            findings: vec![],
            endpoints: 2,
            actions: 3,
            checks: Checks::all() - Checks::RDF,
            declared: Box::new(Declarations {
                opted_out: vec![OptedOut {
                    endpoint: "email-send".into(),
                    verb: Some(Verb::Sink),
                    reason: "sends real mail".into(),
                }],
                pure: vec!["to-upper".into()],
                cacheable: vec!["to-upper".into()],
                namespaces: vec!["urn:example:ns#".into()],
                fixtures: vec![Fixture::new("file", Verb::Source)
                    .binding("path", "README.md")
                    .arg(
                        "content",
                        "a body that is long enough to be cut short in the report",
                    )],
            }),
            unprobed: vec![Unprobed {
                endpoint: "notes-delete".into(),
                verb: Verb::Delete,
                check: Check::Outputs,
                reason: "never fired under root".into(),
            }],
        };
        // The Err of `check` travels by value: keep it under clippy's large-error bar.
        assert!(std::mem::size_of::<Report>() <= 128);
        let text = report.to_string();
        assert!(text.contains("0 finding(s) across 2 endpoint(s), 3 action(s)"));
        assert!(text.contains("skipped: SKOLEM-RDF VOCABULARY"));
        assert!(text.contains("opted out: email-send sink: sends real mail"));
        assert!(text.contains("declared pure: to-upper"));
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
        assert!(report.is_clean());
        assert!(report.into_result().is_ok());
    }
}

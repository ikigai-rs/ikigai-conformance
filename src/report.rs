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
#[derive(Clone, PartialEq, Eq)]
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
    /// The faces the walk actually reached and served (`probed: …`) — so a
    /// first-run clean report carries positive evidence of coverage rather than
    /// only the absence of findings.
    pub probed: Vec<Probed>,
    /// The description ids the walk reached, in walk order — what `endpoints`
    /// counts, by name. A declaration naming an id that is not here reached no
    /// check at all ([`Check::Declarations`]), and a test can assert the walk
    /// covered exactly the endpoints the module means to bind.
    ///
    /// A boxed slice, not a `Vec`: `Report` is the `Err` of [`check`](crate::check)
    /// and travels by value, and its size is held under clippy's large-error bar
    /// (a `Vec` here puts it exactly AT the 128-byte threshold, which the lint
    /// rejects).
    pub walked: Box<[String]>,
}

/// What a module declared when it configured the [`Suite`](crate::Suite),
/// echoed in the [`Report`]: every waiver is printed beside the findings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Declarations {
    /// The actions the module opted out of the invoking checks, with reasons.
    pub opted_out: Vec<OptedOut>,
    /// The single checks the module waived per endpoint, with reasons
    /// ([`Suite::opt_out_check`](crate::Suite::opt_out_check)).
    pub opted_out_checks: Vec<OptedOutCheck>,
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
    /// The fixtures the module supplied, printed one per line (`fixture: file
    /// source path="README.md"`) so a reader can tell which inputs a walk ran over.
    pub fixtures: Vec<Fixture>,
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
                    p.endpoint,
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
            writeln!(
                f,
                "opted out: {} {}: {reason}",
                ids.join(" "),
                check.label()
            )?;
        }
        if !declared.pure.is_empty() {
            writeln!(f, "declared pure: {}", declared.pure.join(" "))?;
        }
        if !declared.cacheable.is_empty() {
            writeln!(f, "declared cacheable: {}", declared.cacheable.join(" "))?;
        }
        if !declared.live.is_empty() {
            writeln!(f, "declared live: {}", declared.live.join(" "))?;
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
            walked: vec!["cms-graph".to_string(), "ik-context".to_string()].into_boxed_slice(),
            checks: Checks::all() - Checks::RDF,
            declared: Box::new(Declarations {
                opted_out: vec![OptedOut {
                    endpoint: "email-send".into(),
                    verb: Some(Verb::Sink),
                    reason: "sends real mail".into(),
                }],
                opted_out_checks: vec![OptedOutCheck {
                    endpoint: "browse-review".into(),
                    check: Check::Vocabulary,
                    reason: "four ik: terms land in the next vocabulary release".into(),
                }],
                pure: vec!["to-upper".into()],
                cacheable: vec!["to-upper".into()],
                live: vec!["clock-now".into()],
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
        assert!(report.is_clean());
        assert!(report.into_result().is_ok());
    }
}

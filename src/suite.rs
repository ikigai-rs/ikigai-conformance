//! The walk: every endpoint the kernel lists, every action each declares, every
//! selected check — findings accumulated, never short-circuited.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, Description, Error, Expiry, InputSource, Iri, Kernel,
    Representation, Request, SpaceEntry, TraceEvent, Tracer, UriTemplate, Verb,
};

use crate::checks::{Check, Checks};
use crate::rdf;
use crate::report::{Declarations, Finding, OptedOut, OptedOutCheck, Probed, Report, Unprobed};

/// The kernel's own operations are listed by [`Kernel::entries`] ahead of the root
/// space's bindings; they are core's, not the module's, so the walk skips them
/// unless asked ([`Suite::include_kernel_ops`]).
const KERNEL_NS: &str = "urn:kernel:";

/// The XSD namespace, for deriving a plausible scalar from a declared `class`.
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The value substituted for an untyped input or template variable.
const PLACEHOLDER: &str = "x";

/// The IRI substituted for an entity-valued (`rdfs:Class`) or `xsd:anyURI` input.
const PLACEHOLDER_IRI: &str = "urn:example:conformance";

/// The longest fixture value a report line prints before cutting it and saying so.
const SHOWN: usize = 48;

/// The endpoint field of a finding about the suite itself rather than an endpoint —
/// a registered namespace has no id. Matches `(kernel)`, used for a root space that
/// cannot be enumerated.
const SUITE: &str = "(suite)";

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    id: String,
    verb: Verb,
    args: BTreeMap<String, String>,
    bindings: BTreeMap<String, String>,
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
        }
    }

    /// Supply (or override) one by-value argument.
    pub fn arg(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
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
/// after 48 characters with the true length stated.
///
/// ```
/// use ikigai_conformance::Fixture;
/// use ikigai_core::Verb;
///
/// let f = Fixture::new("file", Verb::Sink).binding("path", "scratch.txt").arg("content", "a\nb");
/// assert_eq!(f.to_string(), r#"file sink path="scratch.txt" content="a\nb""#);
/// let long = Fixture::new("doc", Verb::Source).arg("content", "x".repeat(200));
/// assert!(long.to_string().ends_with("… (202 chars)"), "{long}");
/// ```
impl fmt::Display for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id, crate::report::verb_name(self.verb))?;
        for (var, value) in &self.bindings {
            write!(f, " {var}={}", shown(value))?;
        }
        for (name, value) in &self.args {
            write!(f, " {name}={}", shown(value))?;
        }
        Ok(())
    }
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

/// What the walk actually covered, recorded as it goes — the evidence
/// [`Check::Declarations`] reads to say which declarations were never consulted.
///
/// A declaration is a promise about a check that will honour it. When the target of
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
    /// Ids declaring at least one RDF face, the only thing the graph checks probe.
    rdf_faces: BTreeSet<String>,
    /// The template variables of every pattern an id is bound at.
    vars: BTreeMap<String, BTreeSet<String>>,
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
    /// Whether any RDF face was probed at all (an unused namespace means something
    /// different when nothing was parsed).
    faces_probed: bool,
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

    /// Whether every action of `id` was excluded by [`Suite::opt_out`].
    fn wholly_opted_out(&self, id: &str) -> bool {
        let verbs = self.verbs_of(id);
        !verbs.is_empty() && verbs.iter().all(|v| self.is_opted_out(id, Some(*v)))
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
    checks: Checks,
    fixtures: Vec<Fixture>,
    opt_outs: Vec<OptOut>,
    opt_out_checks: Vec<OptOutCheck>,
    namespaces: Vec<String>,
    pure: Vec<String>,
    cacheable: Vec<String>,
    live: Vec<String>,
    kernel_ops: bool,
}

impl Default for Suite {
    fn default() -> Self {
        Suite::new()
    }
}

impl Suite {
    /// Every check, no fixtures, no opt-outs, kernel operations skipped.
    pub fn new() -> Self {
        Suite {
            checks: Checks::all(),
            fixtures: Vec::new(),
            opt_outs: Vec::new(),
            opt_out_checks: Vec::new(),
            namespaces: Vec::new(),
            pure: Vec::new(),
            cacheable: Vec::new(),
            live: Vec::new(),
            kernel_ops: false,
        }
    }

    /// Select the checks to run; the rest are reported as skipped.
    pub fn checks(mut self, checks: Checks) -> Self {
        self.checks = checks;
        self
    }

    /// Supply inputs for one action.
    pub fn fixture(mut self, fixture: Fixture) -> Self {
        self.fixtures.push(fixture);
        self
    }

    /// Exclude one action (`verb`), or every action of an endpoint (`None`), from
    /// the checks that invoke it — for an action with real side effects. The reason
    /// is printed in the report. The description-only checks still run.
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
    /// reported by [`Check::Vocabulary`]. Register only a namespace the module
    /// DEFINES (serves a vocabulary for); an undefined one is what the check exists
    /// to catch.
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

    /// Declare an endpoint a pure function of its inputs, so a cacheable result
    /// with an empty golden-thread set is correct rather than a representation that
    /// caches forever with nothing to cut it.
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

    /// Walk the kernel's own `urn:kernel:*` operations too. They are core's
    /// endpoints, not the module's — off by default so a module's report is about
    /// the module.
    pub fn include_kernel_ops(mut self) -> Self {
        self.kernel_ops = true;
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
                opted_out_checks: self
                    .opt_out_checks
                    .iter()
                    .map(|o| OptedOutCheck {
                        endpoint: o.id.clone(),
                        check: o.check,
                        reason: o.reason.clone(),
                    })
                    .collect(),
                pure: self.pure.clone(),
                cacheable: self.cacheable.clone(),
                live: self.live.clone(),
                namespaces: self.namespaces.clone(),
                fixtures: self.fixtures.clone(),
            }),
            unprobed: Vec::new(),
            probed: Vec::new(),
            walked: Box::default(),
        };

        let Some(entries) = kernel.entries() else {
            report.findings.push(Finding::new(
                "(kernel)",
                None,
                Check::ArgSpecs,
                "the root space is not enumerable (`Kernel::entries()` is `None`): \
                 nothing can be walked — bind the module in an `EndpointSpace`",
            ));
            return report;
        };

        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut walk_order: Vec<String> = Vec::new();
        let mut coverage = Coverage::default();
        for entry in entries {
            if entry.pattern.starts_with(KERNEL_NS) && !self.kernel_ops {
                continue;
            }
            let Some(description) = kernel.describe_pattern(&entry.pattern) else {
                if self.runs(&entry.endpoint, Check::ArgSpecs) {
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
                }
                continue;
            };
            let first_time = seen.insert(description.id.clone());
            if first_time {
                report.endpoints += 1;
                walk_order.push(description.id.clone());
                self.static_checks(&description, &mut report);
            }
            // What the walk covered, recorded from the DESCRIPTION rather than from
            // what was reached: a declaration is inert against the shape the module
            // declared, so a template that fails to expand must not also make every
            // fixture for that id read as inert.
            coverage.walked.insert(description.id.clone());
            if let Some(vars) = template_vars(&entry.pattern) {
                coverage
                    .vars
                    .entry(description.id.clone())
                    .or_default()
                    .extend(vars);
            }
            for spec in description.action_specs() {
                coverage.record_action(&description.id, spec.verb, false);
                if spec.outputs.iter().any(|o| rdf::is_rdf_face(o)) {
                    coverage.rdf_faces.insert(description.id.clone());
                }
            }
            // Per entry, not per id: two patterns binding one endpoint are two
            // places it can be reached, each with its own template variables.
            self.template_checks(&entry, &description, &mut report);

            let target = match self.target_for(&entry, &description) {
                Ok(iri) => iri,
                Err(detail) => {
                    if self.runs(&description.id, Check::ArgSpecs) {
                        report.findings.push(Finding::new(
                            &description.id,
                            None,
                            Check::ArgSpecs,
                            detail,
                        ));
                    }
                    continue;
                }
            };
            for spec in description.action_specs() {
                report.actions += 1;
                if self.opted_out(&description.id, spec.verb) {
                    coverage.record_action(&description.id, spec.verb, true);
                    continue;
                }
                let mut action = Action {
                    suite: self,
                    kernel,
                    id: &description.id,
                    target: &target,
                    spec: &spec,
                    minimal: None,
                    fired: None,
                    no_grants: None,
                    failure_reported: false,
                };
                action.enforced(&mut report).await;
                action.authority(&mut report).await;
                action.rdf_faces(&mut report, &mut coverage).await;
                action.cacheable(&mut report, &mut coverage).await;
                action.pipeline(&mut report).await;
                // Last: it reads what the checks above already resolved and fires
                // nothing new for a mutating verb, so the walk's footprint stays
                // "Source once or twice, Sink once, Delete never".
                action.outputs(&mut report).await;
                // Positive evidence for a face the RDF checks do not cover: what
                // this action actually served, whatever its media type. Fires
                // nothing of its own — it reads what the checks above resolved.
                action.record_probe(&mut report);
            }
        }
        report.walked = walk_order.into_boxed_slice();
        self.declaration_checks(&coverage, &mut report);
        report
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
                 a CACHEABLE result from the empty-golden-thread finding and does nothing else"
            ),
            );
        }
        self.namespace_declarations(coverage, report);
        self.fixture_declarations(coverage, report);
        self.opt_out_declarations(coverage, report);
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
        if !coverage.walked.contains(id) {
            return Some(
                "the walk reached no endpoint with that id (the id is the `Description::id`, \
                 not the bound IRI)"
                    .to_string(),
            );
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
        match check {
            Check::Cacheable if !coverage.verbs_of(id).iter().any(|v| v.is_cacheable()) => {
                Some(format!(
                    "it declares no cacheable verb ({}), and CACHEABLE returns early on a \
                     mutating one",
                    verb_list(&coverage.verbs_of(id))
                ))
            }
            Check::Authority if !coverage.verbs_of(id).iter().any(|v| v.is_mutating()) => {
                Some(format!(
                    "it declares no mutating verb ({}), and AUTHORITY looks only at what a \
                     `Sink` or a `Delete` did under no grants",
                    verb_list(&coverage.verbs_of(id))
                ))
            }
            Check::SkolemRdf | Check::Vocabulary if !coverage.rdf_faces.contains(id) => Some(
                "it declares no RDF face, and the graph checks probe declared RDF outputs only"
                    .to_string(),
            ),
            _ => None,
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

    fn fixture_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for fixture in &self.fixtures {
            let id = &fixture.id;
            if !coverage.walked.contains(id) {
                self.inert(
                    report,
                    id,
                    format!(
                    "fixture `{fixture}` was never used: the walk reached no endpoint with that \
                     id (the first field is the `Description::id`, not the bound IRI)"
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
            if fixture.args.is_empty() {
                // A binding-only fixture is per ENTRY: its verb is ignored by
                // design, so a verb that names no action says nothing about it.
                continue;
            }
            let verb = fixture.verb;
            if !coverage.declares(id, verb) {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` supplies arguments for a {} action `{id}` does not \
                     declare ({}): they were never used",
                        crate::report::verb_name(verb),
                        verb_list(&coverage.verbs_of(id))
                    ),
                );
            } else if coverage.is_opted_out(id, Some(verb)) {
                self.inert(
                    report,
                    id,
                    format!(
                        "fixture `{fixture}` supplies arguments for an action excluded by \
                     `Suite::opt_out`: they were never used"
                    ),
                );
            }
        }
    }

    fn opt_out_declarations(&self, coverage: &Coverage, report: &mut Report) {
        for out in &self.opt_outs {
            let id = &out.id;
            if coverage.is_opted_out(id, out.verb) {
                continue;
            }
            let why = if !coverage.walked.contains(id) {
                "the walk reached no endpoint with that id".to_string()
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
    fn inert(&self, report: &mut Report, id: &str, detail: String) {
        if self.runs(id, Check::Declarations) {
            report
                .findings
                .push(Finding::new(id, None, Check::Declarations, detail));
        }
    }

    // ----- description-only checks -------------------------------------------

    fn static_checks(&self, description: &Description, report: &mut Report) {
        if self.runs(&description.id, Check::ArgSpecs) {
            argspecs(description, report);
        }
        if self.runs(&description.id, Check::RequiresVerb) {
            requires_verb(description, report);
        }
        if self.runs(&description.id, Check::Names) {
            names(description, report);
        }
        if self.runs(&description.id, Check::Pipeline) {
            pipeline_static(description, report);
        }
    }

    /// Every template variable of the entry's pattern must be a declared
    /// `Binding`-source input of every action — otherwise the manifold cannot form
    /// the IRI from the contract and drops the action.
    fn template_checks(&self, entry: &SpaceEntry, description: &Description, report: &mut Report) {
        if !self.runs(&description.id, Check::ArgSpecs) {
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

    /// Whether `check` runs for `id`: selected suite-wide ([`Suite::checks`]) and not
    /// excluded for this endpoint ([`Suite::opt_out_check`]). Every check asks this
    /// rather than `checks.contains`, so a per-id waiver silences one rule and one
    /// endpoint — never a second check by accident.
    fn runs(&self, id: &str, check: Check) -> bool {
        self.checks.contains(check)
            && !self
                .opt_out_checks
                .iter()
                .any(|o| o.id == id && o.check == check)
    }

    fn fixture_for(&self, id: &str, verb: Verb) -> Option<&Fixture> {
        self.fixtures.iter().find(|f| f.id == id && f.verb == verb)
    }

    /// The concrete IRI to resolve for an entry: the pattern itself when it is an
    /// IRI, else the template expanded with fixture bindings or values derived
    /// from the declared binding inputs.
    fn target_for(&self, entry: &SpaceEntry, description: &Description) -> Result<Iri, String> {
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
            let from_fixture = self
                .fixtures
                .iter()
                .filter(|f| f.id == description.id)
                .find_map(|f| f.bindings.get(var).cloned());
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
    /// `Argument`-source input, overlaid with the fixture's arguments.
    fn args_for(&self, id: &str, spec: &ActionSpec) -> BTreeMap<String, String> {
        let mut args: BTreeMap<String, String> = spec
            .inputs
            .iter()
            .filter(|i| i.required && i.source == InputSource::Argument)
            .map(|i| (i.name.clone(), sample_value(i)))
            .collect();
        if let Some(fixture) = self.fixture_for(id, spec.verb) {
            for (k, v) in &fixture.args {
                args.insert(k.clone(), v.clone());
            }
        }
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

/// The smallest plausible value the ArgSpec admits: its default, else the first
/// `one_of`, else a value shaped by its `class`, else a placeholder.
fn sample_value(spec: &ArgSpec) -> String {
    if let Some(default) = &spec.default {
        return default.clone();
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

fn build_request(verb: Verb, target: &Iri, args: &BTreeMap<String, String>) -> Request {
    let mut request = Request::new(verb, target.clone());
    for (name, value) in args {
        request = request.with_arg(name, ArgRef::Inline(value.as_bytes().to_vec()));
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
                "declares requires {} but no verb: `action_specs()` iterates verbs, so this \
                 scope is silently inert — the kernel enforces nothing (add `.verb(…)`)",
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

// ----- PIPELINE (static half) -------------------------------------------------

fn pipeline_static(description: &Description, report: &mut Report) {
    for spec in description.action_specs() {
        if !spec.verb.is_mutating() {
            continue;
        }
        let by_value: Vec<&ArgSpec> = spec
            .inputs
            .iter()
            .filter(|i| i.source == InputSource::Argument)
            .collect();
        let has_required = by_value.iter().any(|i| i.required);
        let has_content = by_value.iter().any(|i| i.name == "content");
        if has_required && !has_content {
            report.findings.push(Finding::new(
                &description.id,
                Some(spec.verb),
                Check::Pipeline,
                format!(
                    "declares by-value inputs ({}) but none named `content`: a pipeline's \
                     upstream value and a top-level `sink`'s body both arrive as `content`, \
                     so this action cannot receive either",
                    by_value
                        .iter()
                        .map(|i| format!("`{}`", i.name))
                        .collect::<Vec<_>>()
                        .join(", ")
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
    minimal: Option<Result<(Representation, Vec<TraceEvent>), Error>>,
    fired: Option<Result<Representation, Error>>,
    /// What the action did under a capability holding no grants. ENFORCED and
    /// AUTHORITY read the same one resolution: two probes would fire a Sink twice.
    no_grants: Option<Result<Representation, Error>>,
    failure_reported: bool,
}

impl Action<'_> {
    fn request(&self, args: &BTreeMap<String, String>) -> Request {
        build_request(self.spec.verb, self.target, args)
    }

    fn finding(&self, check: Check, detail: impl Into<String>) -> Finding {
        Finding::new(self.id, Some(self.spec.verb), check, detail)
    }

    /// Whether `check` runs for this action — selected, and not waived for this id.
    fn runs(&self, check: Check) -> bool {
        self.suite.runs(self.id, check)
    }

    /// Resolve with minimal inputs under root, traced. Memoized: the RDF and the
    /// cache checks both need it, and a second resolution is what the cache check
    /// measures.
    async fn resolve_minimal(
        &mut self,
        again: bool,
    ) -> Result<(Representation, Vec<TraceEvent>), Error> {
        if !again {
            if let Some(memo) = &self.minimal {
                return memo.clone();
            }
        }
        let args = self.suite.args_for(self.id, self.spec);
        let collector = Arc::new(Collector::default());
        let tracer: Arc<dyn Tracer> = collector.clone();
        let result = self
            .kernel
            .issue_traced(self.request(&args), &Capability::root(), tracer)
            .await
            .map(|repr| {
                let events = std::mem::take(&mut *collector.0.lock().expect("collector lock"));
                (repr, events)
            });
        self.minimal = Some(result.clone());
        result
    }

    /// Report a minimal-resolution failure under `check`, once per action.
    fn report_failure(&mut self, check: Check, err: &Error, report: &mut Report) {
        if self.failure_reported {
            return;
        }
        self.failure_reported = true;
        report.findings.push(self.finding(
            check,
            format!(
                "did not resolve with the minimal inputs its ArgSpecs allow ({err}); supply a \
                 `Fixture::new(\"{}\", Verb::{:?})` with inputs that work, or opt out with a \
                 reason",
                self.id, self.spec.verb
            ),
        ));
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
        let args = self.suite.args_for(self.id, self.spec);
        let none = Capability::scoped(Vec::<String>::new());
        let result = self.kernel.issue(self.request(&args), &none).await;
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
                format!(
                    "declares no capability but refused with `Denied` under no grants ({msg}): \
                     an enforced scope the manifold does not declare — it over-offers"
                ),
            ));
        }
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

    // ----- SKOLEM-RDF + VOCABULARY --------------------------------------------

    async fn rdf_faces(&mut self, report: &mut Report, coverage: &mut Coverage) {
        let skolem = self.runs(Check::SkolemRdf);
        let vocab = self.runs(Check::Vocabulary);
        if !skolem && !vocab {
            return;
        }
        // Failures of the face itself (unresolvable, not RDF, unparseable) are
        // SkolemRdf's when it runs, Vocabulary's otherwise — a report never loses
        // them to the selection.
        let face_check = if skolem {
            Check::SkolemRdf
        } else {
            Check::Vocabulary
        };
        let faces: Vec<String> = self
            .spec
            .outputs
            .iter()
            .filter(|o| rdf::is_rdf_face(o))
            .map(|o| rdf::bare_media_type(o))
            .collect();
        for face in faces {
            let result = if self.spec.outputs.len() > 1 {
                // Several faces: `as=` picks this one, the universal conneg selector.
                let mut args = self.suite.args_for(self.id, self.spec);
                args.entry("as".to_string()).or_insert_with(|| face.clone());
                self.kernel
                    .issue(self.request(&args), &Capability::root())
                    .await
            } else {
                self.resolve_minimal(false).await.map(|(repr, _)| repr)
            };
            let repr = match result {
                Ok(repr) => repr,
                Err(err) => {
                    self.report_failure(face_check, &err, report);
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
                                 or `Suite::namespace` one the module serves)"
                                ),
                            ));
                        }
                    }
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
        let (first, _) = match self.resolve_minimal(false).await {
            Ok(first) => first,
            Err(err) => {
                // Nothing can be said about `pure` here: the endpoint might well be
                // cacheable once its inputs are right. DECLARATIONS must not add an
                // "inert declaration" line on top of a resolution failure.
                coverage.cacheable_unresolved.insert(self.id.to_string());
                self.report_failure(Check::Cacheable, &err, report);
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
        if second.bytes != first.bytes {
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
        let probed = self
            .kernel
            .is_cached(&self.request(&args), &Capability::root());
        if !traced_hit && !probed {
            report.findings.push(self.finding(
                Check::Cacheable,
                "marked cacheable but the second resolution recomputed (the trace records no \
                 cache hit): a dependency is uncacheable — effective expiry is the least \
                 cacheable part's — or the result carries `Expiry::At` on a clockless kernel",
            ));
        }
        if first.threads().is_empty() && !self.suite.pure.iter().any(|p| p == self.id) {
            report.findings.push(self.finding(
                Check::Cacheable,
                "cacheable with an empty golden-thread set: it will be served forever with \
                 nothing to cut it. `Suite::pure(id)` if it is a pure function of its inputs; \
                 otherwise `depends_on` the thread of the state it reads",
            ));
        }
    }

    // ----- PIPELINE (invoking half) --------------------------------------------

    async fn pipeline(&mut self, report: &mut Report) {
        if !self.runs(Check::Pipeline) {
            return;
        }
        let Some(content) = self
            .spec
            .inputs
            .iter()
            .find(|i| i.name == "content" && i.source == InputSource::Argument)
        else {
            return;
        };
        let mut args = self.suite.args_for(self.id, self.spec);
        args.entry("content".to_string())
            .or_insert_with(|| sample_value(content));
        let result = self
            .kernel
            .issue(self.request(&args), &Capability::root())
            .await;
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
        let probed = if self.spec.verb.is_mutating() {
            self.fired
                .clone()
                .or_else(|| self.minimal.clone().map(|r| r.map(|(repr, _)| repr)))
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
                self.report_failure(Check::Outputs, &err, report);
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
        if let Some(label) = args.get("as").map(|a| rdf::bare_media_type(a)) {
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

    fn unprobed(&self, report: &mut Report, check: Check, reason: impl Into<String>) {
        report.unprobed.push(Unprobed {
            endpoint: self.id.to_string(),
            verb: self.spec.verb,
            check,
            reason: reason.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

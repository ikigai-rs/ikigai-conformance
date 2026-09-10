//! The walk: every endpoint the kernel lists, every action each declares, every
//! selected check — findings accumulated, never short-circuited.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use ikigai_core::{
    ActionSpec, ArgRef, ArgSpec, Capability, Description, Error, Expiry, InputSource, Iri, Kernel,
    Representation, Request, SpaceEntry, TraceEvent, Tracer, UriTemplate, Verb,
};

use crate::checks::{Check, Checks};
use crate::rdf;
use crate::report::{Declarations, Finding, OptedOut, Report};

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

#[derive(Clone, Debug)]
struct OptOut {
    id: String,
    verb: Option<Verb>,
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
/// assert!(report.is_clean(), "{report}");
/// ```
#[derive(Clone, Debug)]
pub struct Suite {
    checks: Checks,
    fixtures: Vec<Fixture>,
    opt_outs: Vec<OptOut>,
    namespaces: Vec<String>,
    pure: Vec<String>,
    cacheable: Vec<String>,
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
            namespaces: Vec::new(),
            pure: Vec::new(),
            cacheable: Vec::new(),
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

    /// Register a namespace prefix as the module's own, so terms under it are not
    /// reported by [`Check::Vocabulary`]. Register only a namespace the module
    /// DEFINES (serves a vocabulary for); an undefined one is what the check exists
    /// to catch.
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
                pure: self.pure.clone(),
                cacheable: self.cacheable.clone(),
                namespaces: self.namespaces.clone(),
            }),
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
        for entry in entries {
            if entry.pattern.starts_with(KERNEL_NS) && !self.kernel_ops {
                continue;
            }
            let Some(description) = kernel.describe_pattern(&entry.pattern) else {
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
                continue;
            };
            let first_time = seen.insert(description.id.clone());
            if first_time {
                report.endpoints += 1;
                self.static_checks(&description, &mut report);
            }
            // Per entry, not per id: two patterns binding one endpoint are two
            // places it can be reached, each with its own template variables.
            self.template_checks(&entry, &description, &mut report);

            let target = match self.target_for(&entry, &description) {
                Ok(iri) => iri,
                Err(detail) => {
                    if self.checks.contains(Check::ArgSpecs) {
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
                    continue;
                }
                let mut action = Action {
                    suite: self,
                    kernel,
                    id: &description.id,
                    target: &target,
                    spec: &spec,
                    minimal: None,
                    failure_reported: false,
                };
                action.enforced(&mut report).await;
                action.rdf_faces(&mut report).await;
                action.cacheable(&mut report).await;
                action.pipeline(&mut report).await;
            }
        }
        report
    }

    // ----- description-only checks -------------------------------------------

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
        if self.checks.contains(Check::Pipeline) {
            pipeline_static(description, report);
        }
    }

    /// Every template variable of the entry's pattern must be a declared
    /// `Binding`-source input of every action — otherwise the manifold cannot form
    /// the IRI from the contract and drops the action.
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

/// One action under examination: the memoized minimal resolution under root and
/// whether its failure has already been reported (once, under the first check that
/// needed it).
struct Action<'a> {
    suite: &'a Suite,
    kernel: &'a Kernel,
    id: &'a str,
    target: &'a Iri,
    spec: &'a ActionSpec,
    minimal: Option<Result<(Representation, Vec<TraceEvent>), Error>>,
    failure_reported: bool,
}

impl Action<'_> {
    fn request(&self, args: &BTreeMap<String, String>) -> Request {
        build_request(self.spec.verb, self.target, args)
    }

    fn finding(&self, check: Check, detail: impl Into<String>) -> Finding {
        Finding::new(self.id, Some(self.spec.verb), check, detail)
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

    async fn enforced(&mut self, report: &mut Report) {
        if !self.suite.checks.contains(Check::Enforced) {
            return;
        }
        let args = self.suite.args_for(self.id, self.spec);
        let none = Capability::scoped(Vec::<String>::new());
        let result = self.kernel.issue(self.request(&args), &none).await;
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

    // ----- SKOLEM-RDF + VOCABULARY --------------------------------------------

    async fn rdf_faces(&mut self, report: &mut Report) {
        let skolem = self.suite.checks.contains(Check::SkolemRdf);
        let vocab = self.suite.checks.contains(Check::Vocabulary);
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
                    if !rdf::is_defined(&term, &self.suite.namespaces) {
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

    // ----- CACHEABLE ----------------------------------------------------------

    async fn cacheable(&mut self, report: &mut Report) {
        if !self.suite.checks.contains(Check::Cacheable) || !self.spec.verb.is_cacheable() {
            return;
        }
        let (first, _) = match self.resolve_minimal(false).await {
            Ok(first) => first,
            Err(err) => {
                self.report_failure(Check::Cacheable, &err, report);
                return;
            }
        };
        if first.expiry == Expiry::Always {
            // The kernel hands back the EFFECTIVE expiry, so this is either "never
            // marked cacheable" (the recipe's "when in doubt, don't") or "marked
            // cacheable over a volatile dependency" — indistinguishable from here.
            // Only a declaration separates them.
            if self.suite.cacheable.iter().any(|c| c == self.id) {
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
        if !self.suite.checks.contains(Check::Pipeline) {
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

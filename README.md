# ikigai-conformance

What "done" means for an [ikigai](https://github.com/ikigai-rs) module, as one
test:

```rust
#[test]
fn conforms() {
    ikigai_conformance::check(&my_kernel()).unwrap();
}
```

`check` walks every endpoint the kernel binds, every action each declares, and
returns **every violation of the module recipe at once** — one line per finding,
endpoint id first, so a failing test is a checklist rather than the first miss:

```text
tag-suggest  ARGSPECS  source: input `book` has no class: declare an rdfs:Class IRI for an entity or an XSD datatype IRI for a scalar (…)
tag-suggest  CACHEABLE  source: cacheable with no golden thread but its own name: it will be served forever with nothing to cut it. The kernel hangs every cacheable read on the name it was read through, and only a write through that same name cuts it — this endpoint declares no `Sink` or `Delete`. `Suite::pure(id)` if it is a pure function of its inputs; otherwise `depends_on` the thread of the state it reads
link-remove  REQUIRES-VERB  declares requires `urn:cap:fs:write:*` but no verb: since ikigai-core 0.1.85 the kernel enforces it on every verb, but the catalog cannot say which actions it gates (add `.verb(…)`)
cms-graph  SKOLEM-RDF  source: the `text/turtle` face has 4 blank node(s) (_:b0, _:b1, _:b2, …): skolemize — mint a stable IRI per node (`urn:ikigai:endpoint:{id}:…`, `urn:event:{uid}`), never a counter
cms-graph  VOCABULARY  source: the `text/turtle` face uses `https://ikigai-rs.dev/ns#shelf`, which ikigai-vocab does not define and no well-known or registered namespace covers: an invented term with no definition (…)
sparql-construct  OUTPUTS  source: served `text/turtle` with its minimal inputs but declares only `application/sparql-results+json`: a face the manifold does not announce and the RDF checks never saw — declare it (`.output("text/turtle")`) or serve what is declared
link-remove  DECLARATIONS  declared live (`Suite::live`) but nothing held it to it: it declares no cacheable verb (it declares delete), and CACHEABLE returns early on a mutating one. The report prints `declared live: link-remove`, which reads as a check that ran
7 finding(s) across 4 endpoint(s), 4 action(s)
checked: ARGSPECS REQUIRES-VERB ENFORCED AUTHORITY OUTPUTS SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE REQUIRED NAMES VALIDATE SPACE-NAME DECLARATIONS RESOLVES RECOMPUTE
skipped: FAILURES
probed 3 face(s) across 3 endpoint(s)
probed: cms-graph source `text/turtle`: 412 triple(s)
probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was checked
probed: tag-suggest source `text/plain`: 96 byte(s)
required: dropped 1 input(s) across 1 endpoint(s)
required: sparql-construct source without `query`: refused missing-argument
declared live: link-remove
fixture: sparql-construct source query="CONSTRUCT WHERE { ?s ?p ?o }"
unprobed: link-remove delete OUTPUTS: never fired under root: a mutating action is fired only by the pipeline probe (PIPELINE, on an action declaring `content`), so what it serves was not observed
```

The `probed:` lines are the positive half: a clean report would otherwise be
indistinguishable from a never-probed one — an endpoint whose face is undeclared,
unreachable or opted out produces no line and no finding, exactly like one whose
face is perfect. The triple count says whether the pass meant anything, and a
walk that resolved nothing says so rather than printing no section at all. The
`required:` lines do the same for `REQUIRED`: each input it dropped and the refusal
that came back, or one line saying it had nothing to drop and why.

The last finding is the same idea turned on the module's own declarations: a
`live` on a `Delete` reached no check, changed nothing, and was printed in a clean
report exactly like one that had been honored. See
[What the walk did NOT do](#what-the-walk-did-not-do).

The module recipe in the ikigai field guide is a page of prose every author must
remember. The checkable rules are now one test; the prose rule becomes one line
pointing at the check.

## The checks

| check | recipe row | what it sees |
|---|---|---|
| `ARGSPECS` | ArgSpecs from day one | at least one action per description; every input has an IRI `class`; a `default` is one of the `one_of` values when both exist; input names unique per action; every template variable (`urn:file:{path}`) is a declared `.binding()` input |
| `REQUIRES-VERB` | declared = enforced | a `requires` with no verb — enforced on every verb since core 0.1.85 (it was silently inert before), but the catalog cannot say which actions it gates |
| `ENFORCED` | declared = enforced | under a capability holding no grants, every action with a `requires` is refused with a typed `Denied`; an action declaring nothing is not. An undeclared gate makes the manifold over-offer, and the finding names both readings, since their fixes differ: a CAPABILITY it enforces is declared (`.requires(…)`), a STATE no capability expresses (refused until another resource opens a window) is documented and `ENFORCED` waived with that reason. And, opt-in, the module's OWN rule: a caller a `Suite::enforced_under` probe names (some grant under the declared prefix, not the one for this target) is refused with a typed `Denied` on the module's path |
| `AUTHORITY` | declared = enforced | the fourth cell of `ENFORCED`'s own probe: a `Sink` or a `Delete` that declares no `requires` and **mutated anyway** under a capability holding no grants. Nothing gates the write, so nothing can be withheld — a party that should read and report cannot be given read alone, because read is all there is. A `Source` is not in scope (a public read is a decision a module makes); a mutating action refused for some *other* reason is recorded as `unprobed`, never as a finding, because what an ungranted caller could do through it was not observed |
| `OUTPUTS` | faces are declared | the bare media type the action serves with its minimal inputs (`;charset=` and other parameters stripped, no `as=`) is one of its declared `outputs`. A wrong declaration hides a face from every consumer that reads outputs — `SKOLEM-RDF` and `VOCABULARY` included, which filter the declaration for RDF faces before probing; linkeddata's `sparql-construct` declared only `application/sparql-results+json` over Turtle for its whole life and the RDF checks saw nothing. What the check cannot observe (a mutating action never fired under root, a failed minimal resolution, a caller's `as=` label) is printed as `unprobed`, never as a finding. A **pass-through** (core 0.1.105's `output_from`, ledger #45/#301) reads `outputs` as "at least these": `output_from(Input("src"))` holds the served type to the type `src`'s resource serves (the walk resolves it), and `output_from(Origin)` (a type decided outside the kernel, an HTTP origin's) is printed as a declared pass-through, never a finding. Delete the `Check::Outputs` waiver a pass-through used to need |
| `SKOLEM-RDF` | skolemize; no blank nodes | every declared RDF face (`text/turtle`, `application/ld+json`, `application/rdf+xml`, N-Triples, N-Quads, TriG) resolves with the smallest inputs its ArgSpecs allow, parses, and has no blank node |
| `VOCABULARY` | faces use the shared vocabularies | the face **parses**, and every predicate and class in it is defined in `ikigai-vocab`, or under a well-known namespace (rdf, rdfs, xsd, owl, dcterms, Dublin Core Elements 1.1, foaf, schema, prov, ical, skos, sh, and the W3C Web Annotation `oa:`) or one the module registers. Because it parses, it also reports a mislabeled or malformed face — under its own name when `SKOLEM-RDF` is not selected, so `VOCABULARY` alone proves a hand-written `@prefix` line is well-formed (a face that does not resolve at all is `RESOLVES`'s) |
| `CACHEABLE` | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and — if it never expires (`Expiry::Never`; an `Expiry::At` deadline is a bound) — carries a golden thread besides its own name unless the endpoint is declared pure or takes writes through that name; an endpoint declared pure carries no thread but its own name and what it read; a result declared live (`Suite::live`) is `Expiry::Always` |
| `PIPELINE` | pipeline citizenship | a mutating action with by-value inputs declares `content` (where a pipe's value and a `sink`'s body arrive), and on a `Sink` whether or not any of them is required: a Sink whose inputs are all optional is the one a pipe alone most often drives (0.8.4; a `Delete` with only optional inputs, a `reason`, is not held to it); an action declaring `content` reads it. A read whose only intake is an input group with no `pipe` member is printed as a `pipeline: … is named-only` line, information rather than a finding: no pipe can reach it (`InputGroup::pipe` names the member a pipe fills). Where `content` is a member of an exactly-one group, the pipeline probe sends it INSTEAD of the named alternative, as the engine routes a pipe |
| `REQUIRED` | ArgSpecs from day one | each input an action declares **required**, by value and with no default, is dropped from the call the walk resolved, one at a time, and the call must then be refused with a typed `MissingArgument` or `InvalidArgument`. A call that **succeeds** without it is "declared required, actually optional": a false negative in the selection funnel, because `urn:kernel:validate` and an MCP tool schema both refuse a valid call that omits it. Five adopters wrote this check by hand (http, runbook, cms, secret, sexpr shipped the defect). A refusal of another kind is a finding too (an agent cannot tell its own omission from a failure of the endpoint), unless it is transient. `Source` and `Exists` by default; `Sink` and `Delete` only under `Suite::required_on_mutating()`, because each drop is a firing and a wrongly accepted one has mutated. A call that did not resolve in the first place is listed as `unprobed`. Since 0.9.0 (core 0.1.105) it also probes each **input group**: every member dropped, each other member alone, and a second member beside the first where the group allows at most one. Core's one checker (`ActionSpec::check`, the rules behind `urn:kernel:validate`) says whether the contract refuses each call, and the endpoint must agree: a call the contract refuses gets a typed refusal, and a call it accepts is not refused as missing an argument. The second is the false green ledger #15 removed from the pre-flight; the first is a group stricter than the endpoint (`exactly_one` where both together are served: `at_least_one`). The minimal call fills each exactly-one or at-least-one group with its `pipe` member (else its first), and every input a given one `needs` |
| `NAMES` | naming convention | the description id is a kebab-case noun (`tag-suggest`, `kernel-catalog`) — the MCP projection derives an agent's tool name from it |
| `VALIDATE` | a name is spelled into IRIs | `Description::validate()` accepts the description: no id, input name, input `class` or `requires` holds a character an RDF `IRIREF` cannot carry (``<>"{}\|^`\``, a space, a control character), and no id or input name holds a code point no IRI carries (DEL, a C1 control, private use, a noncharacter; core 0.1.99). Since core 0.1.100 the emitters encode a name and degrade an authored IRI that is not one to a literal, so the endpoint's Meta Turtle and contract IRIs still parse, but not as written: this is the strict check on an authoring mistake, on top of that encoding. Since core 0.1.105 it also refuses a contract whose relations it cannot honor: an input group, a `needs`, a value note, an envelope, an example or an `output_from` that names what the contract does not declare, and a **default** on a required input (a required binding included), or one the contract's own checker refuses. Since core 0.1.106 an **optional** binding may carry a default (`.binding().optional()`): the kernel fills it when the matched name leaves its segment out (ledger #1547). A default matters more now: the kernel INJECTS it (ledger #19). See [Why `VALIDATE` is here and not in `bind`](#why-validate-is-here-and-not-in-bind) |
| `SPACE-NAME` | a name is a claim: same name, same doors | a space the module declares **self-named** claims `space_iri("<module>")` (`urn:iki:space:<module>`), on its topology root too, and two calls of its constructor claim the same name over the same doors; a space it declares **host-named** claims nothing. See [A space's name](#a-spaces-name-space-name) |
| `DECLARATIONS` | — | every declaration the module made (`live`, `cacheable`, `pure`, `namespace`, `required_on_mutating`, a `Fixture`, an `opt_out`, an `opt_out_check`, a declared space) reached the check that would honor it. A `live` on a `Sink` reached nothing and the report printed `declared live:` anyway |
| `RESOLVES` | — | the resolution another check needed (the minimal one that `OUTPUTS`, `SKOLEM-RDF`, `VOCABULARY` and `CACHEABLE` read; the pipeline probe's firing of a `Sink`) resolved. A failure is reported ONCE per action under this label, with the refusal's kind, the check that needed it and, where the walk had to invent a value for an input that declares no class, that input by name: fixing it fixes both findings. It fires nothing of its own. Not selected (a selection written before 0.8.4), the failure is reported under the check that needed it, as before |
| `RECOMPUTE` | cacheability | an `Expiry::Never` answer the cache holds is recomputed after a cut of every thread it carries and of everything it read, 24 times, and every computation must repeat the first one's bytes (one recomputation agrees with the first by chance as often as a two-entry map's order does); each thread an endpoint declared (`depends_on`) is also cut alone and recomputed. `CACHEABLE`'s second resolution is the cache hit, so it can only repeat the first: this is the witness that sees a wrong `.cacheable()` (a fresh nonce, key or timestamp, a `HashMap` order). An order is caught only if the fixture has enough entries for order to matter ([below](#a-fixture-gives-an-order-something-to-order)). An `Expiry::At` answer is exempt (a function of the time too). Each clean answer is a `recomputed:` line. Subtract it (`Checks::all() - Checks::RECOMPUTE`) for a test that pins how often a fixture is read, or waive it per endpoint with the reason an answer differs by design |
| `FAILURES` | declared = produced | **Opt-in in 0.10 (`Suite::check_failures()`), the default from 0.11.0.** An action that states its failure classes (core 0.1.108's `.fails(FailureClass::…)` on an `ActionSpec` or a flat `Description`) fails only with a class it admits: every failure the walk observes from it (the minimal resolution, an RDF face's `as=` resolution, the pipeline firing, REQUIRED's baseline firing, the no-grants probe, an `enforced_under` probe, a forced recomputation, a `fails_under` probe) is held to the declaration by core's one predicate, `ActionSpec::admits_failure`, over `Error::class()`. `permanent` is always admitted and `denied` beside a `requires`. An action that states nothing is listed on a `failures:` line, never a finding. A `Denied` under the no-grants probe stays `ENFORCED`'s, and REQUIRED's deliberately broken calls stay `REQUIRED`'s. A `Suite::fails_under` probe must fail with exactly the class it states. It fires nothing a walk does not already fire, but one minimal resolution of a read no other selected check resolved |

### Why `VALIDATE` is here and not in `bind`

`ikigai-core`'s `Space::bind` is infallible and does not validate, by decision
(ledger #1361): a host composing spaces should not have to handle an error for
every door it binds, and a description is fixed when it is written, not when it is
bound. So a bad name is caught where it is AUTHORED, by the suite every module's CI
already runs, and the module never ships it.

`VALIDATE` is not what keeps a document whole. Since core 0.1.100 the emitters
ENCODE a name (`encode_name`: `%`, what a Turtle `IRIREF` cannot spell raw, `[` and
`]`, and every code point no IRI carries), so an id like `http://[::1]/x` is a legal
document, and they degrade an authored IRI (a `class`, a `requires` scope) that is
not an IRI to a literal. Whatever a description holds, its Meta Turtle and contract
IRIs parse. `VALIDATE` is the strict layer on top: a value it refuses is an
authoring mistake that the encoding hides rather than one it breaks on, and the
module should not ship it.

It is not `NAMES`. `NAMES` is a convention a module may waive (a proper-noun id, a
pre-convention builtin); a value `validate` refuses is a mistake core itself rejects,
so waiving `VALIDATE` is publishing a Meta face that does not say what was written. The finding quotes core's refusal, which
names the field and the value; it never depends on which error variant core used.
`validate` stops at the first refusal, so a description with two bad names shows the
second once the first is fixed.

Every check is independently selectable, so a module adopts incrementally, and
every skipped check is printed as skipped:

```rust
use ikigai_conformance::{check_with, Checks};

check_with(&my_kernel(), Checks::all() - Checks::RDF).unwrap();
```

## A space's name (`SPACE-NAME`)

A space's `id()` is a **cache claim**: any space named `n` holds the same doors, the
cache partitions on the name, a corridor built from a named space shares cache
entries with every other instance of it, and `urn:kernel:topology` and the space
diagrams show the node by it. So a name goes on only where the claim is known to be
true (the convention is on `ikigai_core::Space::id`):

- a **configuration-free** `space()` (no parameters, and nothing read while building
  it: no config home, environment, files or ambient platform backend) names itself
  `urn:iki:space:<module>`, the crate name without `ikigai-`;
- an **instance-built or parameterized** constructor (`space(root)`,
  `space_with_budget(..)`, a config) stays anonymous, and the HOST names it, because
  only the host knows which instance it passed in;
- a **different set of doors** gets a different name or none: a part of a module's
  space with doors of its own is `urn:iki:space:<module>:<part>`
  (`ikigai_sexpr::arrangement_space` is `urn:iki:space:sexpr:arrangement`);
- a **stateful** zero-argument constructor (fresh state on every call) stays
  anonymous.

The check is about a CONSTRUCTOR, not the kernel the walk is given, so the module
states which kind each of its constructors is, and the suite holds it to that.

### Adopting it

**In the module**, where `space()` is configuration-free: name the space LAST
(binding a door after naming drops the name, since core 0.1.89), and export the
name.

```rust
use ikigai_core::{space_iri, EndpointSpace};

/// The name [`space`] claims: `urn:iki:space:text`.
pub const SPACE_ID: &str = "urn:iki:space:text";

pub fn space() -> EndpointSpace {
    EndpointSpace::new()
        .bind(/* … every door … */)
        .named(space_iri("text"))
}
```

`Fallback`, `Mount` and every other core combinator have the same `.named(..)`.
Raise the module's pins to `ikigai-core = "0.1.89"` and
`ikigai-conformance = "0.6.0"`.

**In the module's conformance test**, one line on the `Suite` it already builds,
plus one assertion that the exported const is the name the suite checked:

```rust
let report = Suite::new()
    // … the declarations the test already makes …
    .self_named_space("text", ikigai_text::space)
    .run_blocking(&kernel);
report.assert_clean();
assert_eq!(ikigai_core::space_iri("text").as_str(), ikigai_text::SPACE_ID);
```

What the check saw is data too. The field is `report.declared` (a `Box<Declarations>`),
and its `spaces` holds one `DeclaredSpace` per declaration, in declaration order, with
the kind and how many doors were compared. To assert the kind in one line:

```rust
assert_eq!(report.declared.spaces[0].naming, ikigai_conformance::SpaceNaming::SelfNamed);
```

**An instance-built constructor** is declared host-named, by value. Build it once
in an `Arc` and hand the same space to the kernel and the suite (`Arc<S>` is a
`Space`); the label is what findings name it by, so write the call:

```rust
let space = Arc::new(ikigai_fs::space(root.clone()));
let kernel = Kernel::new(space.clone());
let report = Suite::new()
    // … the declarations the test already makes …
    .host_named_space("ikigai_fs::space(root)", space)
    .run_blocking(&kernel);
report.assert_clean();
```

A module with both kinds declares both (`ikigai-sparql`: `space()` self-named,
`space_with_budget(..)` host-named), and a part with doors of its own is just
another self-named space under its own name:
`.self_named_space("sexpr:arrangement", ikigai_sexpr::arrangement_space)`. There is
no third form: `space_iri` takes `<module>:<part>`.

### What it asserts

For a **self-named** space, the constructor is called twice, and:

- `id()` is `space_iri("<module>")`, which lies under `SPACE_PREFIX`
  (`urn:iki:space:`). The older spelling `urn:ikigai:space:…` is reported as
  outside the prefix;
- both calls claim the same name;
- both calls hold **the same doors**: equal `topology()` (the whole tree, compared
  structurally: every door's pattern, match kind and endpoint name, every confined
  corridor, every enclosed space, in order, with the root's own name set aside) and
  equal `entries()` (the second witness, for a space whose topology is opaque).
  The finding prints the first line where the two calls disagree;
- the topology's root node carries the name (`urn:kernel:topology` and the diagram
  read the node, not `id()`);
- any other self-named declaration under the same name holds the same doors, which
  catches a part declared under the whole's name (each passes alone).

For a **host-named** space: `id()` is `None` and the topology root is anonymous.

Findings read like every other check's, labeled by the space (its IRI, or the
host-named label) and naming the rule:

```text
urn:iki:space:example  SPACE-NAME  `my_module::space` claims `urn:ikigai:space:example`, outside `urn:iki:space:`: a module's configuration-free space is named `ikigai_core::space_iri("example")`, which is `urn:iki:space:example`
urn:iki:space:example  SPACE-NAME  two calls of `my_module::space` hold different doors (first call: `/ door 0 `urn:example:echo-0` (exact) -> echo`; second call: `/ door 0 `urn:example:echo-1` (exact) -> echo`): a name is a claim — same name, same doors — and every call answers to `urn:iki:space:example`. A constructor whose doors vary per call (…) is host-named: declare it with `Suite::host_named_space` and drop the name
ikigai_fs::space(root)  SPACE-NAME  claims `urn:iki:space:fs`, but it is declared host-named: its doors depend on what it was handed, so only the host knows which instance it is. Drop `.named(..)` and let the host name it — a name is a claim: same name, same doors
```

and every declared space gets a `space:` line saying what was compared:

```text
space: urn:iki:space:text self-named by `ikigai_text::space`: two calls, 8 door(s) compared
space: ikigai_fs::space(root) host-named
```

**A module that declares no space is held to nothing**, as an endpoint declaring
neither `cacheable` nor `live` is held to neither, and the report says so rather
than reading as a name that was checked:
`space: none declared — SPACE-NAME checked no constructor (…)`. A space declared
while `SPACE-NAME` is not selected is a `DECLARATIONS` finding, and
`opt_out_check("<label>", Check::SpaceName, "why")` waives one declared space.

## Before the first run: a walk runs what your endpoints run

The invoking checks resolve your actions **for real**. A `Source` or `Exists` is
resolved under root with the smallest inputs its ArgSpecs admit (`x`,
`urn:example:conformance`, `1`). Every action is resolved again under a capability
holding no grants (`ENFORCED` and `AUTHORITY`). A `Sink` or `Delete` declaring
`content` is fired under root by the pipeline probe. `REQUIRED` resolves once more
per required input. The contract is stated in verbs, but the hazard is in the
endpoints: **whatever your actions do, the walk does.** On a composing host that
means a subprocess seam runs a command, an outbound HTTP verb reaches the network, an
inference endpoint calls a model, and a calendar `Sink` writes a real calendar. It
has happened. A bare first run over one server really ran `git` through
`urn:system:exec` (the finding was the command's own non-zero exit), shelled out to
`gh` through ten PR facades, and POSTed to a live inference server through
`urn:llm:ollama:ask`. "An action declaring nothing is not refused" also means that
action is **resolved, under no grants, for real**.

So a host that binds a process, network or model seam (a module that binds one
too) takes two steps:

1. **A static walk first.** `Checks::STATIC` is every check that reads descriptions
   and declarations only, and it resolves nothing, under root or under no grants.
   The report says `probed: nothing`, and `Report::walked` lists every endpoint the
   kernel binds, so the fix list is in hand before anything can fire. On the
   largest composing host so far, this pass gave 223 of 338 findings. On
   ikigai-browse it gave 40 of 40.

   ```rust
   use ikigai_conformance::{Checks, Suite};

   let report = Suite::new().checks(Checks::STATIC).run_blocking(&my_kernel());
   println!("{report}"); // every id is in report.walked; nothing ran
   ```

2. **Fixtures and opt-outs before any invoking check.** Write them from that list,
   not from the first invoking report. Every endpoint that spawns a process, reaches
   the network or calls a model gets one of three things. A `Fixture` can point it
   at something the test owns. An `opt_out` or `opt_out_at` can exclude it, with the
   reason. Or `Suite::own` can scope it out of the walk entirely, if it belongs to a
   dependency (see below). **A fixture must never reach a live service.** Build
   an LLM endpoint with its `base_url` at a stub the test bound itself (port 0, the
   port read back from the listener), never at `OpenAiConfig::ollama`'s default
   `http://localhost:11434/v1`, which is somebody's real inference server. Treat an
   HTTP origin, an SMTP relay or a keychain the same way. Then select the invoking
   checks.

`Checks::STATIC` includes `DECLARATIONS`. So once a suite carries declarations for
the invoking checks (`live`, `pure`, a fixture's caller), a static rerun reports them
as reaching nothing. Use `Checks::STATIC - Checks::DECLARATIONS` for that rerun.

## Fixtures, opt-outs, declarations

The invoking checks resolve each action with the smallest inputs its ArgSpecs
admit (`one_of[0]`, a value shaped by the XSD `class`, `urn:example:conformance`
for an entity, `x` otherwise). They run **against the kernel you pass**, so build
it as a test fixture. When the spec cannot say what a valid call is, or an action
has real side effects, say so — and what you said is printed in the report, the
fixtures included (`fixture: file source path="README.md"`). The `x` is what
PIPELINE pipes into a `content` with no fixture, so a Sink whose `content` has
a grammar (TOML, JSON, a log segment) refuses it; when a failed resolution was
sent that generic sample, the finding says so and names the fix,
`Fixture::new(id, verb).arg("content", …)`. An input that is not text (a gzip
stream, an image) takes `arg_bytes`, which sends the bytes exactly as given; the
report prints a value that is not UTF-8 as its first 16 bytes in hex and its length
(`fixture: gunzip source content=0x1f8b0800000000000003030000000000… (20 bytes)`):

```rust
use ikigai_conformance::{Check, Fixture, Suite};
use ikigai_core::Verb;

Suite::new()
    .fixture(Fixture::new("jsonld-expand", Verb::Source).arg("content", "{}"))
    .fixture(Fixture::new("file", Verb::Source).binding("path", "README.md"))
    .fixture(Fixture::new("gunzip", Verb::Source).arg_bytes("content", EMPTY_GZIP))
    .opt_out("email-send", Some(Verb::Sink), "sends real mail")
    .opt_out_at("urn:orgfile:{path}", None, "jailed to a configured dir")
    .opt_out_check("review", Check::Vocabulary, "ik:quote lands in vocab 0.1.70")
    .namespace("https://example.org/cms#")   // a vocabulary this module serves
    .pure("wc")                              // a pure function: no thread but its own name
    .cacheable("catalog")                    // marks .cacheable(): hold it to that
    .live("secret")                          // uncacheable by decision: hold it to that
    .run_blocking(&my_kernel())
    .assert_clean();                         // panics with the whole checklist
```

`Report::assert_clean()` is `assert!(report.is_clean(), "{report}")` in one call;
`into_result().unwrap()` prints the same text, because `Report`'s `Debug`
delegates to its `Display`.

Three more things every adopter used to write by hand (0.8.4, ledger #354):

```rust
use ikigai_conformance::{Check, Expected, Suite};

let report = Suite::new()
    .label("mounted")                  // the report's first line: `run: mounted`
    .run_blocking(&my_kernel());
report.assert_clean_with(              // clean, AND the walk had this shape
    &Expected::new().endpoints(3).actions(4).walked(["upper", "wc", "catalog"]).no_opt_outs(),
);
let keys = report.keys();              // BTreeSet<(&str, Check)>: pin WHICH findings
```

`assert_clean_with` panics listing every expectation the report missed, then the report.
A host that walks one suite over two kernels labels each run, because the declarations a
report echoes are the same in both. And a host that partitions findings by owner keys on
`Finding::owner()`, not `endpoint` (ledger #341): a `DECLARATIONS` finding names the
endpoint its declaration was about, but its fix is in the suite, so its owner is `None`.

**Both cacheability polarities are declarations.** `Suite::cacheable` exists
because the kernel hands back the **effective** expiry — the least cacheable of a
result and its dependencies — so an endpoint that marked its result cacheable over
one volatile sub-resolution comes back indistinguishable from one that never did.
That silent downgrade once turned a 20µs read into a 1s read on every request;
declared, it is a red test. `Suite::live` is its twin and covers the direction
nothing else can see: an endpoint that is *not* declared cacheable and silently
**becomes** cached — a secret, a live platform read, a non-deterministic
generation, host state no `depends_on` could name — is reported by no check,
because the probe returns silently on `Expiry::Always` and an undeclared endpoint
is held to nothing in either direction. Declaring it live also puts "uncacheable
on purpose" in the printed record, where a clean report otherwise cannot
distinguish a decision from an omission. The two contradict each other; declaring
both for one id is itself a finding.

**A per-check opt-out keeps the rest of the endpoint covered.** `opt_out` drops
*every* invoking check for an id, so one legitimately-red rule costs `ENFORCED`,
`CACHEABLE` and `SKOLEM-RDF` on the same endpoint — ikigai-browse paid exactly
that on its most security-relevant endpoint, for a release cycle, because four
`ik:` terms awaited a vocabulary release in another repo. `opt_out_check(id,
check, reason)` waives one rule for one endpoint, reason printed, and reaches the
description-only checks (`NAMES`, `ARGSPECS`) nothing else can silence per id.
Identical `(check, reason)` waivers print on one line listing their ids, and the
`checked:` line stars a check that ran on only some endpoints. The waived check still
runs, with its findings set aside, so a waiver that suppressed nothing is a `note:`
line beside it (a note, never a finding): the upstream fix it waited for has landed,
and the waiver can go. Setting a check aside fires what the check fires on a read,
and never a `Sink` or `Delete` the walk would not otherwise fire; a waiver it could
judge only that way gets no note.

**A `Description::id` is a name for a KIND of endpoint, not for a place one can be
reached** — so `opt_out` excludes an id at *every* pattern it is bound at, and that
is sometimes exactly wrong. `ikigai-cli` binds `ikigai_fs::FileEndpoint` twice:
`urn:file:{path}`, jailed to a scratch root and safe to fire, and
`urn:orgfile:{path}`, jailed to whatever `calendar.json` names — which with no config
at all is the EMPTY path, i.e. the process's own working directory. Both describe as
`file`. `opt_out("file", …)` takes the safe one down with the dangerous one and the
walk loses coverage it was right to have. `opt_out_at(pattern, verb, reason)` names
**one binding**: the pattern exactly as the space reports it, the template and not an
expanded IRI (`urn:file:{path}`, never `urn:file:x`). One that excluded nothing is a
`DECLARATIONS` finding that prints the patterns the walk did reach.

Where the waiver can be made exact, prefer that: **`ikigai_conformance::rdf` is
public**, and `rdf::parse` + `rdf::terms` + `rdf::is_defined` reproduce
`VOCABULARY` exactly. A module can pin its undefined terms as an EXACT list that
goes red in both directions — a new invented term fails, and so does the day the
missing terms land, which makes the waiver self-destructing. That is strictly
better than `Suite::namespace` for a module's own namespace: a registration waives
every term under it forever, including the next one somebody invents.

**What a walk fires, under root** (or as the caller a `Fixture::capability` names
for that action) — the footprint a fixture author can count on.
It is stated **per FIRING, and a firing is a request**: the verb, the target IRI,
and the arguments. Per firing: a `Source` or `Exists` is resolved once (twice when
cacheable; the second is the cache probe; `RECOMPUTE` resolves a cached `Expiry::Never`
answer 24 more times, each after a cut of every thread it carries, which re-reads what it
was built from each time, and once more per thread an endpoint declared on it, cut alone) and once more per
required by-value input
without a default (`REQUIRED`), each declared RDF face beyond the first
is resolved once more with `as=`, and a `Sink` or `Delete` declaring `content` is
fired **once**, by the pipeline probe — `OUTPUTS` reads that same firing rather than
making another. A mutating action without `content` is never fired under root
(`ENFORCED` runs under no grants and, when the gate is declared, never reaches the
endpoint); the report lists it as `unprobed`. Under `Suite::required_on_mutating()`
a `Sink` or `Delete` is also fired once per required by-value input, and once more as
the baseline when the pipeline probe did not fire it. Sinks land: capture what a
fixture reads before the walk and assert effects after it. Each
`Suite::enforced_under` probe fires its action once more, as the probe's caller.

**An endpoint bound N times is fired once per DISTINCT request those bindings
produce, not N times.** The invoking checks are memoized on the request, because
that is the only identity the walk can compute that means "the same firing": two
bindings agreeing on it agree on every byte the walk would send, so firing the
second can differ from firing the first only in the state the first one left. What
follows, and the reasoning is worth having in front of you when a walk surprises
you:

- **Two bindings, identical request** — a space that lists one binding twice, which
  is what an overlay concatenating its targets' entries produces (`ikigai-throttle`'s
  `Failover`: two spaces over ONE state). Fired **once**, counted once in
  `Report.actions`, and named on a `collapsed:` line, because a smaller action count
  with no explanation is not an explanation. The duplication there is in the SPACE's
  enumeration and not in the descriptions, so nothing keyed on a description could
  have seen it at all.
- **Two bindings, different IRIs** — two requests, **both probed**. This is not a
  concession, it is the point: two entries can share a `Description` and be two
  instances over different state (the `file` / `orgfile` pair above), and collapsing
  them by id would either leave the dangerous one unprobed under a green report or
  fire it and lose the safe one's coverage. Both are worse than firing twice.
- **An alias spelled as a second binding** — `urn:iki:ledger:append` beside
  `urn:iki:ledger:{name}:append` are two IRIs, so the walk probes both. **That is the
  kernel's own reckoning, not the suite's**: two bindings are two cache entries and
  two golden threads, so a `Sink` through one spelling does not invalidate a cached
  read of the other. If the two really are one resource, say so where the kernel can
  see it — **one `Grammar` matching both spellings** (`ikigai-ledger`'s answer), or
  core's `Alias`. If they are meant to stay two and you want only one probed, name
  the binding with `opt_out_at`.

**Fixture bindings are per entry, not per verb.** Every action of an entry
resolves the same IRI, so the verb on a binding-only fixture is ignored (the first
fixture for the id that binds the variable wins); arguments are per `(id, verb)`.

**A state-dependent endpoint's fixture sets the STATE, not only the inputs** (ledger
#432). A `Fixture` supplies arguments, bindings and a caller; it cannot put the process
in a state. An endpoint whose answer reads state the host builds (what the process
composed at startup, a seeded store, an open enrollment window) is probed as the kernel
stands when the walk starts. With that state unset it answers about nothing, a
placeholder graph or an empty list, and every check passes over it. `urn:host:posture`
did exactly that on its first walk: green, over a two-triple placeholder with one
undefined term. The suite cannot tell a rich answer from a placeholder, so the test sets
the state on the kernel it passes, before the walk: record the posture, seed the store,
open the window. Then read that endpoint's `probed:` line, whose triple or byte count is
the evidence the walk saw the state you set; a count that matches the empty answer is
the tell.

#### A fixture gives an order something to order

`RECOMPUTE` catches an answer serialized in `HashMap` (or `HashSet`) iteration order
only when the order can differ, and over a fixture with one node, one statement or one
key it cannot. **Give a cacheable endpoint's fixture enough entries for order to
matter: three or more beats two.** Two computations over a two-entry map agree on the
order about half the time (0.508 measured over four million pairs), so one
recomputation missed `ikigai-jsonld`'s two-node defect about one walk in two (5 runs of
12, ledger #1573). The check now recomputes 24 times and holds every computation to
the first, which puts a two-entry miss at about 6 in 10 million walks (111 in 200
million sequences of fresh maps, measured; more than 0.508^24, because the order is
not a fair coin: a two-entry map iterates in insertion order 56% of the time, and 0.563^25
is the same 6 in 10 million). Three
entries agree far less often, and the miss rate falls with them. Content-addressed IRIs and blank-node
labels that number nodes in iteration order count as order too.

### A composing host scopes the walk to its own endpoints (`Suite::own`)

`Suite::run` walks every entry the kernel lists. So anything bound beside the module
is held to every check exactly as the module is: a dependency the host composes, a
fixture endpoint, a mounted peer. `opt_out` drops the invoking checks and keeps the
static ones. Three composing hosts wrote the same 40-60 line partition by hand to
get a report about the module rather than about everything it composes (ledger #328).

`Suite::own(id)` names what the module owns: a description id, or an id prefix ending
in `*`. Call it once per id or prefix. Once one is declared, every endpoint whose id
matches none is **inherited**. It is listed on an `inherited:` line and in
`report.declared.inherited`, and it is **not walked**: no check reads it and no probe fires
it. A dependency's subprocess seam is not run on the module's behalf.

```rust
let report = Suite::new()
    .own("ledger-*")                 // the module's endpoints
    .own("ledger")
    .inherit("file", "ikigai-fs's, which the ledger reads its config through")
    .inherit("sparql-select", "ikigai-linkeddata's; its own suite covers it")
    .inherit("store-graph-update", "ikigai-store's write door, composed by the host")
    .run_blocking(&host_kernel());
report.assert_clean();
```

```text
own: ledger-* ledger (7 endpoint(s) walked)
inherited: file sparql-select store-graph-update — bound beside the module, not its own (`Suite::own`), not walked
inherited because: file: ikigai-fs's, which the ledger reads its config through
inherited because: sparql-select: ikigai-linkeddata's; its own suite covers it
inherited because: store-graph-update: ikigai-store's write door, composed by the host
```

**`Suite::inherit(id, reason)` pins the rest exactly, and keeps why** (0.8.2). Once one
is declared, the inherited list must be exactly the declared ids, compared as a set:
an inherited endpoint no `inherit` names is a `DECLARATIONS` finding (a module crate
that gains an endpoint outside its scope turns the suite red instead of silently
joining the pile), and so is an `inherit` for an id that was not inherited. Each
reason prints as an `inherited because:` line and is listed by
`Report::inherited_because()`. That keeps what the hand partitions bought. They
asserted the union covered the catalog, so a new endpoint turned a test red rather
than joining the inherited pile unseen. Without any `inherit`, nothing is pinned and
the report reads as in 0.8.0.

To pin it by hand instead, compare a SORTED copy. `report.declared.inherited` (like
`report.walked`) is in catalog order, the order the kernel lists its bindings (for an
`EndpointSpace`, the order they were bound), so a slice literal matches only when it
is written in bind order:

```rust
let mut inherited = report.declared.inherited.clone();
inherited.sort();
assert_eq!(inherited, ["file", "sparql-select", "store-graph-update"]);
```

**Migrating a host from opt-outs to `own` means deleting the opt-outs it replaces.**
An `opt_out` left on an id `own` now scopes out excluded nothing, because an inherited
endpoint is not walked, and that is a `DECLARATIONS` finding. The reason each opt-out
carried was the record of why that door sits beside the crate: move it to
`Suite::inherit` rather than losing it.

A host with many exact ids can pass them at once: `Suite::own_all(&OURS)` is `own`
for each, in order. **A pattern that matches no endpoint is a `DECLARATIONS`
finding that no selection or waiver removes**, and it lists the ids the kernel does
bind. A scope that matches nothing turns the suite into a no-op that passes, which is
this crate's founding defect. A host that wants its dependencies' record too runs a
second suite without `own`.

### An endpoint about its caller names one (`Fixture::capability`)

The walk resolves as **root** by default, and root holds every principal and names
none. So an endpoint that answers about WHOEVER ASKS — the principal a door minted,
like `ikigai-layers`'s `urn:layers:me` — and refuses a caller that names nobody
refuses every probe. Before 0.6.2 the checks that judge an answer (`OUTPUTS`,
`SKOLEM-RDF`, `VOCABULARY`, `CACHEABLE`) could only report the refusal or be
waived one by one, and a `live` declaration was held to nothing. Name the caller
instead, minted the way the door mints it — the scopes the caller is granted plus
its principal:

```rust
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{Capability, Verb};

// Core 0.1.93+: Capability::scoped([...]).with_principal(iri)?. On any core, the
// principal is one scope, `urn:cap:principal:<iri>`, so it can be spelled out.
let participant = Capability::scoped([
    "urn:cap:collab:read:demo",
    "urn:cap:principal:urn:example:person:alice",
]);

Suite::new()
    .live("me")                              // now held to the answer it serves
    .fixture(Fixture::new("me", Verb::Source).capability(participant))
    .run_blocking(&my_kernel())
    .assert_clean();
```

The report records it (`fixture: me source as-caller=urn:example:person:alice (2
scopes)`), so a reader sees the action was probed as someone other than root. What
the caller reaches, and what it does not:

- **Per `(id, verb)`, like `arg`.** It is the caller of every probe that judges an
  answer: the minimal resolution, the cache probe's second resolution (the kernel
  keys its cache on the capability, so both are made as the same caller), each
  extra RDF face, and the pipeline probe's firing of a mutating action that declares
  `content`. Every action no fixture names is still probed under root, so adopting
  this changes nothing for any other endpoint.
- **Not `ENFORCED` or `AUTHORITY`.** Those ask what the action does under a
  capability holding no grants, which carries no principal either. An endpoint that
  refuses an anonymous caller declares that gate (`requires`, with
  `urn:cap:principal:*` as the presence test when identity is the whole gate), or
  `ENFORCED` reports the undeclared refusal, exactly as it always has.
- **A caller nothing resolved as is inert.** A capability for a verb the endpoint
  does not declare, for an action opted out, for a mutating action without `content`
  (fired only under no grants), or for an action whose answer-judging checks are all
  waived is a `DECLARATIONS` finding.

A refusal (`Denied`) under root now says so and names this fix, so an adopter
meeting it for the first time is pointed here rather than at `opt_out`.

**Counts.** `Report.endpoints` counts distinct description ids; `Report.actions`
counts one per distinct FIRING — `urn:a11y:config` and `urn:a11y:config:{app}`
sharing one description are one endpoint and, with one `Source` each, two actions,
because they resolve to different IRIs. Two bindings that resolve to the SAME IRI
with the same arguments are one action, and `Report.collapsed` names them.
`Report.walked` is that first count BY NAME, in walk
order, which is catalog order (the order the kernel lists its bindings; for an
`EndpointSpace`, bind order), never sorted, so a test can assert the walk reached exactly the endpoints the module
means to bind (an endpoint that stops being bound is otherwise a report that gets
*cleaner*).

**One honest exception to "every input has a class".** An opaque-bytes input
(`urn:sniff`'s `content`: a PNG is valid) has no XSD datatype that is true of the
value; declare `xsd:string` — the type the wire carries — and say so in a comment.

The kernel's own `urn:kernel:*` operations are core's, not the module's: the walk
skips them unless `Suite::include_kernel_ops()` asks.

### A module's own access rule (`Suite::enforced_under`)

`ENFORCED` probes every action under a capability holding no grants. For an action
declaring a parameterized scope (`urn:cap:net:*`, `urn:cap:secret:read:*`) the kernel's
floor refuses that caller before the endpoint runs, so the typed `Denied` the check sees
is the kernel's, and the module's own rule (per host, per name, per prefix: the real
gate) is never reached. Nine adoptions tested it by hand, and two found real defects: a
per-host refusal returned as `Error::Endpoint` (ikigai-llm), and a refusal flattened
into a string on the way out (ikigai-lisp).

`Suite::enforced_under(probe)` names a caller that passes the floor but should not pass
the module, and the inputs that reach the rule:

```rust
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{Capability, Verb};

let report = Suite::new()
    .enforced_under(
        Fixture::new("http-get", Verb::Source)
            .arg("url", "http://127.0.0.1/")
            .capability(Capability::scoped(["urn:cap:net:example.com"])),
    )
    .run_blocking(&my_kernel());
```

The module must refuse it with a typed `Denied`, on its own path or on a sub-request it
makes. Resolving is an `ENFORCED` finding (the rule let the caller through), and so is
any other refusal (the type was lost). Each probe that ran is an `enforced under:` line
saying where the refusal came from, read from the kernel's trace: `by the endpoint
itself`, `on its sub-request <iri>`, or `by the kernel's floor`, which means the probe's
caller lacks a declared scope and never reached the module's rule (a `DECLARATIONS`
finding, as is a probe with no caller, or one for an action the walk did not run).

Why a `Fixture` and its own list, not a flag on the action's fixture: a gate in a
SUB-RESOLUTION is reachable only by its own input shape (an `http(s)://` stylesheet, a
program that issues a request, a remote context), which is not the input the walk's
happy path wants, and one action can have several shapes worth pinning (ikigai-meeting
tested five). So the probe's arguments are laid over the walk's minimal ones for that
action, its bindings form the target for the probe alone, and the action's own fixture
is untouched.

Opt-in: nothing is probed for an action no `enforced_under` names, so a suite that does
not call it reports exactly what it did. Where the kernel's floor IS the whole gate (an
exact scope, ikigai-encrypt), there is no "some grant, but not this one" caller to name,
and no probe to write.

### A module's failure classes (`FAILURES`, `Suite::fails_under`)

Core 0.1.108 gives every error a **failure class** (`Error::class()`: `transient`,
`retry-after`, `over-budget`, `denied`, `refused`, `permanent`, `unknown`), which says who
may repeat it and when, and lets an action STATE the classes it can produce
(`.fails(FailureClass::RetryAfter)`, on an `ActionSpec` or on a flat `Description`). The
statement is what a caller, an agent and a retry budget plan around, so it has to be true
(ledger #1546).

`FAILURES` holds it to the half a test can see: **produced ⊆ declared ∪ implied**. Every
failure the walk observes from an action that states its classes must be one core's
`ActionSpec::admits_failure` admits, or the finding names the probe that saw it, its kind
and class, and both fixes (return the variant of a declared class, or declare this one):

```text
fetch  FAILURES  source: declares its failure classes (retry-after), and the minimal resolution failed as `endpoint`, class `unknown`, which the declaration does not admit (…): upstream answered 500. A caller or an agent plans its retries from the declaration, so either return the variant of a class it declares (`Error::Endpoint`, …), or declare this one (`.fails(FailureClass::Unknown)`)
```

An action that states nothing (`Failures::Unstated`, every action written before core
0.1.108) has nothing to compare against: it is listed on one `failures:` line and is never
a finding, until a later minor makes stating classes part of "done", the way `ARGSPECS`
made inputs part of it.

The other half, **declared ⊆ producible**, a walk cannot see: minimal inputs never push a
parser past its bound, and nothing in the walk makes an upstream answer 429. A module that
can provoke a class says so with `Suite::fails_under`, a `Fixture` laid over the action's
minimal inputs and the class it must fail with:

```rust
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::{FailureClass, Verb};

let report = Suite::new()
    .check_failures()
    .fails_under(
        Fixture::new("gunzip", Verb::Source).arg_bytes("content", zip_bomb()),
        FailureClass::OverBudget,
    )
    .run_blocking(&my_kernel());
```

A probe that resolves, or fails with another class, is a `FAILURES` finding, and each
probe that ran is a `fails under:` line. Its failure is an observation like any other, so
an action that states its classes must admit the one its own probe provokes.

**Opt-in for one minor.** 0.10.0 adds the check OFF: `Suite::new()` prints `FAILURES` as
skipped, judges nothing and fires nothing more, so re-pinning to 0.10 changes no adopter's
report but that `skipped:` line. `Suite::check_failures()` turns it on (where it is
selected; `Checks::all()` selects it), and a `fails_under` probe or a `check_failures`
that reached nothing is a `DECLARATIONS` finding. **0.11.0 makes it the default**; to keep
it off then, subtract `Checks::FAILURES`.

### Through a mount

A mounted binding carries an `origin` (the mount's alias or connection), and two things
about a walk through one are worth knowing (0.8.4):

- **A peer's kernel operations are skipped under any name** (ledger #335). An alias
  mount re-prefixes the peer's whole catalog, so `urn:kernel:catalog` arrives as
  `urn:edge:kernel:catalog`, and a prefix test does not see it. The walk recognizes a
  mounted binding as one of core's operations by its description id (`kernel-catalog`,
  an id this kernel's own operations use), by its pattern (one of theirs re-prefixed),
  or, for an operation a newer peer has, by a `kernel-…` id under a `:kernel:` segment.
  Before 0.8.4 a module walked through an alias had core's findings reported as its
  own: thirteen of them on one adoption.
- **A peer with no contract fails, loudly** (ledger #334). A peer that cannot answer
  `Meta`, usually because it was built with `Kernel::new` and has no Meta renderer,
  gives every endpoint it serves the same anonymous, action-less placeholder
  (`remote`). The walk used to count that as ONE endpoint with one "declares no
  action" line, so a whole federated kernel read as small rather than broken. Now
  those bindings are not walked, and one `ARGSPECS` finding per mount counts them,
  names them, carries the mount's own reason and names the fix
  (`Kernel::with_meta_renderer` with `ikigai_vocab::TurtleRenderer`). Like an `own`
  pattern that matched nothing, it is not removed by any selection or waiver.

## What the walk did NOT do

A clean report is worth exactly what it covered, so the report says what it did
not reach as loudly as what it found.

```
0 finding(s) across 6 endpoint(s), 9 action(s)
checked: ARGSPECS REQUIRES-VERB ENFORCED AUTHORITY OUTPUTS* SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE REQUIRED NAMES VALIDATE SPACE-NAME DECLARATIONS RESOLVES RECOMPUTE
* OUTPUTS ran on 1 of 6 endpoint(s); waived on the rest (see `opted out:`)
skipped: FAILURES
probed 7 face(s) across 5 endpoint(s)
probed: cms-graph source `text/turtle`: 412 triple(s)
probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was checked
probed: wc source `text/plain`: 3 byte(s)
required: nothing dropped — no `Source` or `Exists` action declares a required by-value input without a default; a `Sink` or `Delete` does, and REQUIRED fires those only under `Suite::required_on_mutating`
opted out: a-face b-face c-face VOCABULARY: ik:madeUp lands in the next vocabulary release
note: the waiver of VOCABULARY for c-face suppressed nothing — the check ran with it set aside and found nothing; drop it if its reason has passed (ik:madeUp lands in the next vocabulary release)
unprobed: notes-delete delete OUTPUTS: never fired under root
```

- **`probed:`** — every face the walk actually resolved, not only the RDF ones.
  For an RDF face the count is triples, and **0 triples says `nothing was
  checked`** (both RDF checks pass vacuously over an empty graph); for a face no
  check parses it is bytes, evidence that the action was reached and served
  something. A walk that resolved nothing at all says so on one line rather than
  printing no section: a module with no graph face used to be indistinguishable
  from a walk that reached nothing.
- **`required:`** — every input `REQUIRED` dropped and the refusal kind it got
  (`refused missing-argument`; `resolved — declared required, actually optional` is
  the finding), or ONE line saying it dropped nothing, and why: no required by-value
  input on `Source`/`Exists`, `Sink`/`Delete` not walked without
  `Suite::required_on_mutating`, every candidate opted out or unprobed. Five
  adopters had to break their own code to learn whether a green `REQUIRED` had
  bitten. As data: `Report::required_probes()` and `Report::required_nothing()`.
  A call about an input group (0.9.0) is one line naming the group, the members it
  gave and what core's checker says of it: `required: lift source
  group:exactly-one:content,in given `in` (the contract accepts it): resolved`.
- **`unprobed:`** — the actions a check could not observe, with the reason.
- **`note:`** — an `opt_out_check` that suppressed nothing: the waived check ran
  with its findings set aside and found none (`Report::idle_waivers()`). A note,
  never a finding, so a waiver that is green today on purpose costs nothing.
- **`collapsed:`** — bindings that would have issued the identical request, fired
  once, named so the smaller action count is an explanation rather than a mystery.
  Nothing was skipped: the second binding was the first one again.
- **`checked:`** — a starred check ran on some endpoints and is waived on others,
  with the count on its own line. A check waived on five of six endpoints used to
  read as a check that ran. Identical waivers are grouped: one `(check, reason)`
  pair prints once, listing its ids, because five copies of one long reason
  buried every other line of the report.
- **`DECLARATIONS`** — the same rule turned on the module's own declarations.

**A declaration that could not apply is a finding.** A declaration is a promise
about a check that will honor it, and one whose target the walk never reaches is
inert: it changes nothing, fails nothing, and prints in the report exactly like
one that was consulted. `Suite::live` on a `Sink` is the founding instance —
`CACHEABLE` returns early on a non-cacheable verb, so the declaration did nothing
at all while the report said `declared live: notes-write`, and an author who
declared `live` across every id got a clean report over declarations that never
ran. The check covers every declaration, not that one:

| inert | reported as |
|---|---|
| `live` / `cacheable` on an id with no cacheable verb, or one nothing binds, or one whose `CACHEABLE` is unselected, waived or wholly opted out | `declared live … but nothing held it to it: …` |
| `pure` over a result that never came back cacheable | `nothing consulted it: no action of it came back cacheable` |
| a `Fixture` whose id names no walked endpoint, or whose arguments are for an action the endpoint does not declare | `fixture … was never used: …` |
| a `Fixture::arg` naming no input its action declares by value (`as`, the conneg selector, excepted), or naming a template variable, which only `Fixture::binding` fills | `fixture … sends \`inn\`, which no input of its source action declares (it declares \`in\`): …` |
| a `Fixture::capability` for an action that does not exist, was opted out, or that no probe resolved as | `… supplies a caller for …` / `names a caller no probe resolved as: …` |
| a `Fixture::binding` naming no template variable of any pattern that id is bound at | `binds \`number\`, which is not a template variable …` |
| an `opt_out` that excluded nothing | `opted out … but excluded nothing: …` |
| an `own` pattern that matched no endpoint (endpoint `(suite)`; **not** removed by any selection or waiver) | `` `Suite::own("uper")` matched no endpoint the kernel binds …`` |
| an inherited endpoint no `Suite::inherit` names, once any is declared (**not** removed by any selection or waiver) | `` inherited, and no `Suite::inherit` gives the reason: …`` |
| a `Suite::inherit` for an id an `own` pattern matches, with no `own` declared, or that nothing binds (**not** removed by any selection or waiver) | `` `Suite::inherit` gives a reason (…) for an id that is not inherited: …`` |
| `required_on_mutating` when `REQUIRED` is unselected or no `Sink`/`Delete` declares a required by-value input (endpoint `(suite)`) | `` `Suite::required_on_mutating` reached nothing: …`` |
| an `opt_out_check` for a check that could not have run for that id anyway | `waived SKOLEM-RDF … but that check could not have run for it anyway: it declares no RDF face` |
| a `namespace` that accounted for no term in any probed face (endpoint `(suite)`) | `registered namespace … accounted for no term: …` |

The rule is structural — *could this declaration have applied?* — never "did it
silence a finding today": a standing waiver that happens to be green is doing its
job. Two consequences worth stating. `opt_out_check(id, Check::Outputs, …)` on an
endpoint that declares **no** outputs is NOT inert: `OUTPUTS` still fires there
(`served text/plain but declares no output`), so the waiver waives a real rule.
And what this cannot see is a waiver whose *condition* has passed — "until core
§20" still passes silently the day §20 lands; a reason is prose, and prose is what
review is for.

A declaration that is standing on purpose says so in the record:

```rust
Suite::new()
    .live("notes-write")
    .opt_out_check("notes-write", Check::Declarations,
                   "earns a Source face next release; the declaration stands until then")
```

or the whole check comes off with `Checks::all() - Checks::DECLARATIONS`.

## The vocabulary pin

`VOCABULARY`'s oracle is `ikigai_vocab::VOCABULARY` — a term is defined iff it is a
subject there — so this crate's `ikigai-vocab` dependency is load-bearing for
**data**, not API, and **the pin tracks the vocabulary HEAD**: it is raised to the
newest published version in every release of this crate, whether anything else
changed or not. Nothing can enforce that; the crate uses only `NS` and
`VOCABULARY`, both ancient, so no compile error will ever push it up and nothing
will say it has gone stale.

What a stale pin does, and it has happened: cargo unifies `ikigai-vocab` across the
whole graph, so a module whose face uses a term the newest vocabulary defines is
told the vocabulary does not define it — against a `vocabulary.ttl` you can read
and see the term in. **If a term you can see is reported as invented, check the
resolved `ikigai-vocab` version first.** The per-module workaround (an
`ikigai-vocab` dev-dependency no line imports, added purely to lift this floor) is
not needed against a current pin; two modules were carrying one.

## What stays prose

Honest residue — what no check here can see:

- **The converse of declared = enforced** is approximated: a `Denied` from an
  action declaring nothing is caught; an endpoint that enforces an undeclared scope
  by *succeeding differently* (a filtered view) is invisible.
- **Whether a result should be cacheable**, and whether it is pure. The probe checks
  a cacheable result behaves like one; purity, "marks cacheable" and "live on
  purpose" are declarations the module makes (`pure`, `cacheable`, `live`). An
  endpoint declaring none of them is held to none of them. A `pure` is checked
  from one side only: an answer that hangs from a thread nothing it read accounts
  for is a finding (0.8.3), but a "pure" endpoint that reads state it names no
  thread for at all (a global, a file read around the kernel) looks exactly like
  a pure function, and a thread the walk cannot attribute to what it read is
  never reported.
- **`Source` pipeline routing.** The engine routes a piped value into the sole
  unnamed required input by contract; nothing an endpoint does makes that right or
  wrong. Newline-separated list output (the `..` map convention) is a shape no
  ArgSpec states.
- **A declared golden thread is a promise a host must keep.** `CACHEABLE` checks
  the thread set names something besides the endpoint, not that anything cuts it; a module declaring
  `urn:file:` threads over a config home no host watches is clean here. The suite
  cannot see the host.
- **An optional alternative to a required input, undeclared.** Core 0.1.105 spells a
  piped-or-named intake (`InputGroup::exactly_one(["content", "in"]).pipe("content")`,
  both members optional), and `REQUIRED` probes it. A module that still declares
  `content` required beside an optional `in` passes, although a call with only `in`
  succeeds: the suite cannot know `in` was meant as the alternative. Declare the group.
- **A face built over a bundled graph tests that graph too.** A module that folds
  `ikigai-vocab` into every result is probed OVER it; a blank node in a future
  vocabulary release fails that module's walk first.
- **A pass-through output through a binding.** `output_from(Input(name))` is held to
  the type of the resource `name` names when `name` is a by-value IRI the walk can
  resolve; when it is a binding (browse's raw file face, by `path`), the resource it
  determines is the endpoint's to find, so the served type is printed as `unprobed`.
- **Whether two DIFFERENT requests reach the same state.** Firing identity is the
  request, and that is exactly as much as the walk can know: the state behind an
  endpoint is not observable from the space at all. Two bindings of one endpoint over
  one store, an overlay that fans out to mirrors, a jail root two templates share —
  all of those are one state under two names, and nothing in `Kernel::entries()` says
  so (it hands back pattern strings and an endpoint NAME; two instances of one type
  report the same name, and pointer identity is not exposed). Core has the concept —
  an `Alias` reports a canonical, and the kernel keys its cache and its threads on
  it — but `Kernel::canonicalize` is private, so a walk cannot ask before it fires.
  Until it can, a module that means two spellings to be one resource says so with one
  `Grammar` matching both, and the suite probes what the kernel treats as two.
- **Whether a self-named constructor is really configuration-free.** `SPACE-NAME`
  calls it twice in one process, so a constructor that reads the config home or the
  environment reads the same thing both times and passes; and it compares doors by
  pattern, match kind and endpoint NAME, so a zero-argument constructor that
  allocates fresh state per call builds equal doors over different state. Both are
  host-named by rule; which kind a constructor is stays the author's declaration,
  and the check proves only that the declaration is consistent.
- Reading through the kernel rather than `std::fs` (a lint, not a test); where a
  fix belongs; when in doubt, don't cache.

## Proof

`tests/violations.rs` binds one endpoint per check that breaks its rule on
purpose and asserts the check sees it — for `OUTPUTS`, an endpoint that declares
`application/sparql-results+json` and serves Turtle with a blank node, asserting
the RDF checks stayed silent and `OUTPUTS` did not; for `live`, one endpoint the
kernel returns cacheable and one it returns `Always`, asserting the declaration is
what separates them; for `opt_out_check`, one endpoint breaking three rules at
once, asserting a waiver of one leaves the other two reported — and one endpoint
that breaks nothing, asserting the suite is clean on it. `AUTHORITY` is proved on
four fronts, because three of them are silences: an ungated `Sink` is caught
while `ENFORCED` says nothing about it, the same `Sink` with a declared scope is
clean, a `Source` declaring no capability is untouched, and a mutating action
that refuses the minimal call for an unrelated reason produces an `unprobed:`
line rather than a pass. Every `DECLARATIONS`
case is falsified the same way — a `live` on a `Sink`, a fixture for an id
nothing binds, a binding naming no template variable, a waiver for a check that
could not run, a namespace covering nothing — each asserting the silence is gone,
and the `live`-on-a-`Sink` test asserts `CACHEABLE` still says nothing, which is
the whole point. `tests/builtins.rs` runs the suite against
`ikigai-core`'s own `to-upper` / `reverse-list` / `echo` (which predate the recipe)
and pins the exact findings: three untyped inputs and three cacheable pure functions
nobody declared pure — and nothing else (core 0.1.105 renamed the two pre-convention
ids, ikigai-rs/ikigai-core#217, so `NAMES` has nothing to say about them).

A `pure` declaration is falsified with an endpoint that hangs from
`urn:example:db` and reads nothing (caught; the same endpoint undeclared stays
clean), one naming two such threads (one finding naming both), and a composite that
reads a resource and names a thread of its own (caught, for that thread alone). Its
silences are proved the same way: a pure composite over a STATEFUL input (whose
answer carries the input's name and the input's own `depends_on`) and one over a
kernel face (`urn:kernel:bindings`) stay clean, and both tests fail the moment the
check stops reading back the threads of what the endpoint read.

`REQUIRED` is falsified with the runbook's shape (`name` declared required,
`unwrap_or("friend")` in the body), which every other check passes, and with an
untyped refusal; it stays silent on a required input carrying a default (ARGSPECS's
finding already), on an optional alternative intake left unsent, and on a call that
did not resolve (an `unprobed:` line instead). A counting `Sink` proves the opt-in:
fired once by default, three times under `required_on_mutating` (the pipeline
probe, then one drop per required input), with the defaulted `title` caught.

The walk's hazard is proved by COUNTING what ran. An `exec`-shaped `Source` and a
`notify`-shaped `Sink` that declare nothing run twice and once under a default walk
(root, then no grants; the `Sink` under no grants). Under `Checks::STATIC` they run
zero times, and every description is still read. `Suite::own` is proved the same
way: the inherited `exec` runs zero times, `walked` and `inherited` are pinned
exactly, a prefix scopes, a pattern matching nothing is a finding even with
`DECLARATIONS` unselected and waived, and a fixture for an inherited id says it is
inherited.

`VALIDATE` is falsified with an id holding a space, one holding `>`, an input
name holding U+0085 (shown escaped in the report, never raw) and an input name on
an `ActionSpec`; a clean endpoint passes, and a waiver silences it per endpoint. An
id with an IPv6 host (`http://[::1]/x`) is pinned: the check passes it AND its Meta
Turtle parses. That holds from core 0.1.100, whose emitters encode `[` and `]` in a
name; on 0.1.99 the Turtle did not parse (`Invalid IRI code point '['`), which is why
the floor is 0.1.100.

`SPACE-NAME` is falsified one failure at a time in `tests/violations.rs`: a
self-named space claiming nothing, a name outside the prefix, a name other than the
one declared, two calls claiming different names, two calls over different doors
(seen through the topology, and again through `entries()` behind an opaque node), a
topology root that does not carry the name, a part declared under the whole's name,
a host-named constructor that names itself, and a host-named space whose topology
names it. `tests/builtins.rs` passes a self-named space, a part under its own name
and a host-named one, and shows that extending a named space with `bind` leaves it
anonymous.

**Firing identity is proved by COUNTING FIRINGS, not by asserting an outcome** — an
outcome test passes for the wrong reason the moment the endpoint's state is
idempotent, which is exactly when a returning double-fire stops being visible. A
`Sink` holding an `AtomicUsize` is bound behind a space that lists its entries twice
(the `Failover` shape) and must be fired **once**; the same `Sink` is bound as two
separate instances sharing one `Description::id` and each must be fired **once**,
which is the assertion that fails — `(1, 0)`, under a clean green report — the moment
anyone memoizes by id.

## Status

0.10.0. Depends only on published crates (`ikigai-core`, `ikigai-vocab`,
`oxrdfio`). Dual-licensed MIT / Apache-2.0.

### 0.9.1 → 0.10.0: `FAILURES` and `Suite::fails_under`, opt-in (ledger #1546)

Core 0.1.108 gives every error a failure class (`Error::class()`) and lets an action state
the classes it can produce (`.fails(FailureClass::…)`). This release holds the statement
to what the walk sees ([A module's failure classes](#a-modules-failure-classes-failures-suitefails_under)):

- **`FAILURES`**, a seventeenth check: every failure observed from an action that states
  its classes must be one core's `ActionSpec::admits_failure` admits. An action that states
  nothing is listed on a `failures:` line, never a finding.
- **`Suite::fails_under(Fixture, FailureClass)`**: an input that must fail with a stated
  class, held to exactly that class (`fails under:` lines).
- **`Suite::check_failures()`** opts into both. `Report::failures_seen()` and
  `Report::fails_under_probes()` carry the evidence (`FailuresSeen`, `FailsUnder`).
- The `ikigai-core` and `ikigai-vocab` floors move to `0.1.108`.

**Will it turn a green suite red?** No, not in 0.10: the check is off unless a suite asks
for it. What changes without asking is the report TEXT. `FAILURES` is printed on the
`skipped:` line, and where nothing was skipped before, that is a new `skipped: FAILURES`
line. A test that pins a whole report, or asserts there is no `skipped:` line, sees it.
Findings and firings are unchanged: `Suite::new()` reports exactly what
`checks(Checks::all() - Checks::FAILURES)` does. Source changes for a consumer: none,
unless it built an `Evidence` by struct literal (it is `#[non_exhaustive]`, so only this
crate can).

**0.11.0 makes `FAILURES` the default.** No adopter states failure classes yet, so turning
it on judges nothing until one does; the declarations ride the next adoption wave. To keep
it off in 0.11, subtract `Checks::FAILURES`.

**The re-pin wave this implies.** A `"0.8.x"` pin is `<0.9.0` under Cargo's 0.x rules, and
a `"0.9.1"` pin is `<0.10.0`, so **no adopter receives 0.10.0 without a deliberate
re-pin**, and none sees 0.9.0's contract-shape checks either unless it moved to 0.9. As
pinned on 2026-10-18 (the checkouts on disk):

- `"0.8.0"`: ikigai-a11y, ikigai-browse, ikigai-cms, ikigai-cms-web, ikigai-compress,
  ikigai-core (`ikigai-vocab`'s dev-dependency), ikigai-diagram, ikigai-encrypt,
  ikigai-fn, ikigai-fs, ikigai-gonk, ikigai-http, ikigai-jsonld, ikigai-layers,
  ikigai-ledger, ikigai-linkeddata (`ikigai-rdf`, `ikigai-sniff`, `ikigai-sparql`),
  ikigai-llm, ikigai-log, ikigai-markdown, ikigai-meeting, ikigai-module, ikigai-name,
  ikigai-nl, ikigai-org, ikigai-repo, ikigai-runbook, ikigai-script, ikigai-secret,
  ikigai-sexpr, ikigai-shacl, ikigai-sign, ikigai-store, ikigai-text, ikigai-xslt;
- `"0.8.1"`: ikigai-lisp;
- `"0.8.2"`: ikigai-bayes, ikigai-web-demo;
- `"0.8.4"`: ikigai-throttle, ikigai-web;
- `"0.9.1"`: ikigai-cli (`ikigai-embedded`, `ikigai-engine`, `ikigai-tz`).

Re-pinning one of the 0.8 adopters to `"0.10.0"` also brings in 0.9.0's checks (the
contract shape, `REQUIRED`'s input groups, `RECOMPUTE`), which CAN turn a suite red; that
is the real cost of the wave, not `FAILURES`. Each re-pin also lifts the adopter's core to
0.1.108 through this crate's floor.

### 0.9.0 → 0.9.1: an optional binding may carry a default (ledger #1882)

Core 0.1.106 lets an **optional** binding declare a default (`.binding().optional()`
with `.default_value(…)`): the kernel fills it when the matched name leaves the segment
out, so ikigai-ledger's bare `items` reads as `default:items` (ledger #1547). `validate`
refuses only a **required** binding's default now. The core and vocab floors move to
0.1.106, and `VALIDATE`'s hint stops saying a binding is never defaulted: for a required
binding it says to drop the default, or make the binding optional and bind the shorter
name too.

**Will it turn a green suite red?** No. It turns one green: the 0.9.0 note below says
`VALIDATE` refuses a default on a binding, and named ikigai-ledger, ikigai-gonk and
ikigai-org. Each of those bindings is built with `.default_value(…)`, which makes the
input optional, so on core 0.1.106 they pass and need no waiver. A required binding with
a default (set `required = true` after the builder) is still a finding. Source changes for
a consumer: none. A patch release, so a `"0.9.0"` pin admits it; raise `ikigai-core` and
`ikigai-vocab` to `"0.1.106"` where a module's own test relies on the filled default.

### 0.8.4 → 0.9.0: core 0.1.105's contract shape (ledger #1547)

Core 0.1.105 is the one release that changed what a contract can say: input groups
(exactly one, at least one, at most one of, with the member a pipe fills), `needs`
between inputs and on a value, list arguments, open and envelope argument sets,
`"one_of": []`, examples, `output_from`, the kernel injecting a declared default, and
one checker (`ActionSpec::check`) behind `urn:kernel:validate`. The suite reads all of it.

**What is new.**

- **`REQUIRED` probes input groups** through core's checker: every member dropped, each
  other member alone, a pair where the group allows at most one. A call the contract
  refuses must get a typed refusal; a call it accepts must not be refused as missing an
  argument. A `required:` line per call says which group, which members, what the
  contract says and what the endpoint did; `RequiredProbe` gains `group`, `given` and
  `contract_refuses`.
- **The minimal call reads the relations.** It fills each exactly-one or at-least-one
  group with its `pipe` member (else its first), adds every input a given input or a
  given value `needs`, sends only an envelope's carrier (unless a fixture proposes
  fields), and takes an input's first declared **example** before guessing from its
  class. A fixture that only supplied a well-formed value can go.
- **`OUTPUTS` honors `output_from`.** `Input("src")`: the served type must be `src`'s
  resource's type (or a declared output). `Origin`: a declared pass-through, printed as
  `unprobed`, never a finding. Delete the `Check::Outputs` waivers pass-throughs carried.
- **`PIPELINE`** prints `pipeline: … is named-only` for a read whose only intake is a
  group with no `pipe` member (`Report::named_only()`, `NamedOnly`), and its probe sends
  `content` instead of an exactly-one group's named alternative.
- **An open argument set** (`open_arguments`) lets a fixture name any argument: the
  DECLARATIONS finding for an undeclared fixture argument applies to closed sets only.
- **An action whose required input declares an empty enumeration** accepts no value, so
  nothing is invoked and one `unprobed:` line says why.
- **`VALIDATE` explains the new refusals**: a default it refuses says the kernel injects
  it now, and that a binding is never defaulted; a relation it refuses says the
  pre-flight trusts it as written.
- **The builtins renamed**: `to-upper` and `reverse-list` (core's rename,
  ikigai-rs/ikigai-core#217), so the tests that used them follow.

**Will it turn a green suite red?** It can, three ways.

1. **`VALIDATE` refuses a default on a binding.** At core 0.1.105's own `validate`, a
   `.binding()` input with `.default_value(…)` is refused ("a binding is part of the name,
   always present, never defaulted"). Found by reading every `describe()` in the
   ecosystem: `ikigai-ledger` (`ledger_arg`, the optional `{ledger}` segment, on about 20
   endpoints), `ikigai-gonk` (`web.rs`: the `ledger` segment of three page families, the
   passkey `op` and the asset `name`), and `ikigai-org` (`org-agenda`'s `period`). Each
   goes red at its next lock update unless the default is dropped (the binding's summary
   already says what the shorter name means) or `VALIDATE` is waived with that reason.
2. **`REQUIRED` holds a group to the endpoint.** Nothing outside core declares a group
   yet; a module that adopts one (sexpr's `content`/`in`, encrypt's `in`/`content`) is
   held to it from its first walk.
3. **A default is injected.** A test that asserted `MissingArgument` for an omitted
   DEFAULTED input now sees the default. None was found in the ecosystem.

**Source changes for a consumer:** none required; `RequiredProbe` and `Evidence` are
`#[non_exhaustive]`, and `NamedOnly` is new. The core floor is 0.1.105, and a `"0.8"`
pin is a ceiling below this release.

**A minor bump, so a re-pin.** Under Cargo's 0.x caret rules an adopter pinned at
`"0.8.0"` is held below 0.9.0 and keeps reporting green against the 0.8 definition of
"done". Raise the pin to `ikigai-conformance = "0.9.0"` in the same PR that raises
`ikigai-core` and `ikigai-vocab` to `"0.1.105"`: this crate lifts the core a module's
tests resolve to 0.1.105 anyway, so the pins should say so.

### 0.8.4 → 0.9.0: `RECOMPUTE`, `Suite::enforced_under` (ledger #330, #331)

**What is new.**

- **`RECOMPUTE`** (ledger #330), a sixteenth check, on by default. `CACHEABLE`'s second
  resolution is the cache hit, so "byte-identical" compared the first computation with a
  copy of itself, and a wrong `.cacheable()` walked clean (`ikigai-encrypt`'s age
  ciphertext, a fresh ephemeral key per call). For a cached `Expiry::Never` answer,
  `RECOMPUTE` cuts every thread the answer carries and the name of everything it read,
  resolves again, and holds the bytes to the first computation's; it does that **24
  times**, every computation held to the first (ledger #1573); then it cuts each thread
  an endpoint declared alone and does the same once. It needs nothing from core: a pure
  answer has a thread to cut too (its own name, core 0.1.73). Each clean answer is a
  `recomputed:` line (`recomputed: upper source 24 times after a cut of 1 thread(s):
  identical`, `Recomputed::recomputations`); an `Expiry::At` answer is exempt.
- **Why 24, not one** (ledger #1573). One recomputation caught a `HashMap`-order defect
  only by luck: two computations over a two-entry map agree on the order about half the
  time, and `ikigai-jsonld`'s two-node fixture was caught in 5 runs of 12. Measured on
  real adopters, one recomputation against 24: `ikigai-jsonld` before its fix (its
  anonymous-document walk) passed 25 of 50 runs against 0 of 50, and `ikigai-rdf`'s
  `rdf-union` (still unfixed) 15 of 50 against 0 of 50. A two-entry miss is now about 6
  walks in 10 million. A finding past the first recomputation says so: `(recomputation 4
  of 24; the 3 before it repeated the first by chance)`. And give a cacheable fixture
  enough entries for order to matter (see
  [A fixture gives an order something to order](#a-fixture-gives-an-order-something-to-order)).
- **`Suite::enforced_under(Fixture)`** (ledger #331), opt-in. `ENFORCED` proves the
  kernel's floor; this reaches the module's own rule with a caller that passes the floor
  but should not pass the module, and requires a typed `Denied`. See
  [A module's own access rule](#a-modules-own-access-rule-suiteenforced_under).

**Will it turn a green suite red?** `Suite::enforced_under` cannot: nothing is probed
for an action no probe names. `RECOMPUTE` was run against 40 adopter repos (every
version-pinned adopter but the retired ikigai-dev-server) before release, with ONE
recomputation: an order defect over a small fixture could pass that sweep by chance, so
these are the ones it found, not necessarily all. It turned five red:

- **Four real defects**, each a cacheable answer whose bytes differ per computation over
  the same input, three of them a `HashMap`-order serialization: `jsonld-flatten`
  (ikigai-jsonld; the node order over a document with blank nodes), `shacl-validate`
  (ikigai-shacl; the statement order AND the content-addressed report IRIs differ per
  run, 6 distinct outputs in 6 runs), `rdf-union` (ikigai-linkeddata's ikigai-rdf; the
  statement order, 4 distinct outputs in 8 runs), and `ledger-doctor` (ikigai-ledger;
  the text face stamps `checked: … at <now>` and is cached `Expiry::Never` until a write,
  so a reader is served the first computation's time, which may be by design: if it is,
  waive `RECOMPUTE` for it with that reason).
- **One footprint pin, not a defect**: ikigai-org counts how often its fixture file is
  read (2), and the cuts re-read it (54 with 24 recomputations; 8 with one). Update the
  count, or subtract `RECOMPUTE`
  (`Checks::all() - Checks::RECOMPUTE`) for that suite.

A selection built from named checks (`Checks::CACHEABLE | …`) runs `RECOMPUTE` only if it
names it; `Checks::all()` and `Checks::INVOKING` are derived, so they include it.

**Source changes for a consumer:** none required. `Check` is `#[non_exhaustive]`, and the
new `Evidence` fields are on a `#[non_exhaustive]` struct.

**Footprint.** 24 more resolutions per cached `Expiry::Never` answer, plus one per
declared thread, and each cut re-reads what the answer was built from; one per
`enforced_under` probe. Measured against one recomputation, on a debug build (as CI
runs it), the conformance test binary's wall time: `ikigai-text` 4.0 → 6.4 ms,
`ikigai-jsonld` 14 → 45 ms, `ikigai-sparql` 0.18 → 0.80 s (four SPARQL endpoints, 24
oxigraph queries each), the last inside a crate whose tests take about 42 s in all.

### 0.8.3 → 0.8.4: `RESOLVES`, walks through a mount, an all-optional `Sink`, report ergonomics

From the ledger's conformance backlog (ledger #334, #335, #338, #339, #341, #354, #362,
#432, #1054).

**What is new.**

- **`RESOLVES`** (ledger #338): a failed minimal resolution is reported once per action
  under its own label, with the refusal's kind, the check that needed it, and an
  untyped input the walk had to invent a value for. It used to be reported under
  whichever check reached it first, so it read as a caching finding on an endpoint whose
  caching was fine. A selection that does not name it (any written before 0.8.4) reports
  the failure where it always did.
- **Through a mount** (ledger #335, #334): a peer's kernel operations are skipped under
  any alias, and a peer that cannot answer `Meta` is one loud finding naming the missing
  renderer instead of one small endpoint. See [Through a mount](#through-a-mount).
- **`PIPELINE`** (ledger #339): a `Sink` whose by-value inputs are all optional is held to
  declaring `content` too. A `Delete` keeps the old rule.
- **`ENFORCED`** (ledger #362): the undeclared-`Denied` finding names a capability or a
  state gate, and the fix for each.
- **Ergonomics** (ledger #354, #341): `Report::keys()`, `Report::assert_clean_with(&Expected)`,
  `Suite::label(..)`, `Finding::owner()`.

**Will it turn a green suite red?** Run against 44 adopter conformance targets in 41 repos
before release, it turned two red, and both on an assertion that pins the defect fixed
here. ikigai-throttle's zero-budget test expects its refusals as `CACHEABLE` and `OUTPUTS`
findings; they are now five `RESOLVES` findings. ikigai-web's alias-mount test pins the
fourteen core findings that leaked through the alias; none leak now. Each is a one-line
change to the test. Elsewhere: a `Sink` whose inputs are all optional and none is
`content`, a mounted peer with no Meta renderer, and a module walked through an alias are
the shapes that can change a report.

**Source changes for a consumer:** none required. `Check` is `#[non_exhaustive]`, so
the new variant breaks no match.

Adopters pinned at `"0.8.0"` get 0.8.4 at their next lock update with no pin move.

### 0.8.2 → 0.8.3: `Suite::pure` is checked, not taken on trust (ledger #1508)

Found by the ikigai-throttle triage (ledger #1498): a mutation that hung every overlay
result from `urn:foreign` left the suite clean although `upper` is declared pure; only
the repo's own hand test caught it. `pure` exempts a cacheable result from the
no-thread-but-its-own-name finding, and a foreign thread is exactly what silences that
finding, so a false `pure` could never be seen.

**What is new.** Under `CACHEABLE`, an endpoint declared pure whose cacheable answer
carries a thread that is neither its own name nor the name or a thread of a resource it
read is a finding:

```text
lookup  CACHEABLE  source: declared pure (`Suite::pure`) but carries a foreign thread `urn:example:db`: neither its own name nor anything it read, so the answer hangs from state that is not one of its inputs, and a pure function reads none. Drop the `pure` declaration (the thread already answers "what cuts this?"), or drop the dependency if the endpoint really is a function of its inputs
```

What counts as read: every resource the first minimal resolution's trace names (a
sub-request, a by-reference argument, a kernel face such as the catalog), and every
thread each of those carries, read back from the cache. So a pure transform over a
stateful input is not blamed for the input's thread. The check reuses the trace
`CACHEABLE` already recorded; its only reads are of what the endpoint read, only when a
thread is left over, and only when the cache already holds it under a plain `Source` of
its name (a cache hit that computes nothing). A thread it cannot attribute (the first
resolution was itself a cache hit, or a resource it read is not in the cache that way)
is never reported. It is on by default with `CACHEABLE`, and it applies only to ids
declared pure.

**Will it turn a green suite red?** Only where a `pure` declaration is false. Run
against 24 adopters' `tests/conformance.rs` before release, it turned one red:
ikigai-script declares `script-version` and `script-run` pure while both hang from the
script's state (`urn:script:{name}`, `urn:script:{name}:runs`, added for ledger #1076 and
#1079 after the declarations were written). Dropping the two declarations is the whole
fix: the threads already satisfy the rule they exempted from.

**Source changes for a consumer:** none.

Adopters pinned at `"0.8.0"` get 0.8.3 at their next lock update with no pin move.

### 0.8.1 → 0.8.2: `Suite::inherit`, `Suite::own_all`, `Finding::is_suite_wide` (ledger #1497)

From the 0.8 adoption wave: `Suite::own` replaced nineteen opt-outs in one host whose
printed reasons were the record of why each door sits beside the crate, and the host
wrote its own helper to keep them and pin the inherited list; another folded `own`
over fifteen ids; another hard-coded the `(suite)` label.

**What is new.**

- **`Suite::inherit(id, reason)`.** The reason prints as `inherited because: <id>:
  <reason>` and is listed by `Report::inherited_because()` (`Inherited`). Once any is
  declared, the inherited list is pinned exactly, as a set; see the `own` section.
- **`Suite::own_all(ids)`.** `own` for each id, for a host with many exact ids.
- **`Finding::SUITE_WIDE` and `Finding::is_suite_wide()`.** The `(suite)` label of a
  finding about the suite's own declarations, by name.
- **The README says to delete the opt-outs `own` replaces**, and the finding for one
  left behind now says so too: `… it is inherited (…), and an inherited endpoint is
  not walked: delete the declaration (an opt-out's reason belongs in
  `Suite::inherit`)`.
- **A refused generic `content` sample names `arg_bytes` too**: the hint reads
  `` .arg("content", …) (`.arg_bytes("content", …)` for a byte input) ``.
- **`Report::walked` and `Declarations::inherited` are documented as catalog order.**
  They were already; the README's example compared a slice literal that matched only
  in bind order, and now sorts. They are NOT returned sorted: that would change the
  order an adopter's existing assertion compares against, at a patch release.

**Will it turn a green suite red?** No. The pin applies only to a suite that declares
`inherit`, which nothing could before; every other finding is unchanged, and the two
reworded details only gain text at the end of a phrase.

**Source changes for a consumer:** none required. `Evidence` (`#[non_exhaustive]`)
gains `inherited_because`, and there is one new public type, `Inherited`.
`Declarations` is unchanged.

Adopters pinned at `"0.8.0"` get 0.8.2 at their next lock update with no pin move.

### 0.8.0 → 0.8.1: `REQUIRED` shows its evidence, bytes fixtures, idle waivers (ledger #1493)

From the 0.8 adoption wave: five adopters found that a clean `REQUIRED` read the same
whether it dropped twelve inputs or none, one had to hand-write `REQUIRED` for
endpoints whose input is binary, and one pinned a waiver with an exact-findings test
because nothing said when it went stale.

**What is new.**

- **`required:` lines.** One per input `REQUIRED` dropped, with the refusal kind
  (`Error::kind`: `missing-argument`, `invalid-argument`, or the untyped kind it
  reports), or one `required: nothing dropped — …` line naming why: no required
  by-value input on `Source`/`Exists`, `Sink`/`Delete` not walked, every candidate
  opted out or unprobed. As data: `Report::required_probes()` (`RequiredProbe`) and
  `Report::required_nothing()`.
- **`Fixture::arg_bytes(name, bytes)`.** A by-value argument as bytes, sent as given,
  so a decompressor or a sniffer can be fixtured and walked by every invoking check.
  A value that is not UTF-8 prints as hex and a length.
- **`note:` lines for an idle waiver.** An `opt_out_check` whose check, run with its
  findings set aside, found nothing is named beside its reason, and listed by
  `Report::idle_waivers()`. A note, never a finding.

**Will it turn a green suite red?** No. Nothing new is a finding: the `required:` and
`note:` lines are information, and every existing finding is reported exactly as
before (the description-only checks and `DECLARATIONS` now run waived and are set
aside, which leaves the findings list identical).

**What a waiver now costs.** A waived check still runs, after the walk proper is
done with the action, so a module with waivers sees the walk fire what the check
fires UNWAIVED on a read: a waived `REQUIRED` fires its drops, a waived `CACHEABLE`
its second resolution, a waived `ENFORCED` its no-grants read when `AUTHORITY` did not
already make it. It never fires a `Sink` or a `Delete` the walk would not otherwise
fire: a waiver that could be judged only that way (a `PIPELINE` waiver on a `Sink`,
say) is left unjudged and gets no note. A `SPACE-NAME` waiver is never judged. If a
read is one a test must not repeat, that endpoint wants `opt_out`, which still fires
nothing.

**Source changes for a consumer:**

- `Declarations` gains one public field, `evidence` (`Evidence`, `#[non_exhaustive]`,
  so the next piece of evidence is a field there and not here). A struct literal of
  `Declarations` needs it, or `..Default::default()`. No adopter builds one; the
  report is read, not written. Read the evidence through the `Report` methods.
- New public types `Evidence` and `RequiredProbe`, both `#[non_exhaustive]`.

Adopters pinned at `"0.8.0"` get 0.8.1 at their next lock update with no pin move.

### 0.7.2 → 0.8.0: `REQUIRED`, `Suite::own`, `Checks::STATIC` (ledger #329, #328, #348, #343, #340)

**What is new.**

- **A fourteenth check, `REQUIRED`** (ledger #329). Each input an action declares
  required, by value and with no default, is dropped from the call the walk resolved,
  and the call must then be refused, typed. A call that succeeds without the input is
  a finding. `Source` and `Exists` by default; `Sink` and `Delete` under
  `Suite::required_on_mutating()`.
- **`Suite::own(id | "prefix*")`** (ledger #328). It scopes the walk to the module's
  own endpoints. The rest are inherited, listed in `Declarations::inherited`, and not
  walked.
- **`Checks::STATIC` / `Checks::INVOKING`** (ledger #348). `STATIC` is the static-only
  first run, which resolves nothing. See
  [Before the first run](#before-the-first-run-a-walk-runs-what-your-endpoints-run).
- **`oa:` and Dublin Core Elements 1.1 are well known** (ledger #343).
- **A fixture argument naming no declared input is a `DECLARATIONS` finding** (ledger
  #340); `as` is exempt.

**Will it turn a green suite red?** It can. That is why this is a minor bump.

- `REQUIRED` reports every action that declares an input required and succeeds
  without it. Five adopters shipped exactly that before writing the check by hand, and
  this crate's own test suite had one (a test double that ignored its required
  `content`). An endpoint that refuses a missing input untyped (`Error::Endpoint`) is a
  finding too. No adopter was surveyed before this release; expect findings on the
  upgrade.
- A fixture whose arguments name an input its action does not declare is now a
  `DECLARATIONS` finding.
- The well-known namespaces only remove findings. A `Suite::namespace` registered for
  `oa:` or `dc:` now accounts for no term, so `DECLARATIONS` reports it as inert:
  drop the registration.

**Source changes for a consumer:**

- `Check::ALL` is `[Check; 14]`, and `Check::Required` sits after `Check::Pipeline`
  in report order. So the discriminants of `Names`, `Validate`, `SpaceName` and
  `Declarations` move up by one. `Check` is `#[non_exhaustive]`; nothing here
  exposes the discriminant.
- `Declarations` gains three public fields: `required_on_mutating`, `own` and
  `inherited`. A struct literal of it needs them, or `..Default::default()`.
- `Check::invokes` is now a `const fn`.

⚠ **Most adopters pin `"0.6.x"`**: 42 of the 44 adopter manifests counted at this
release, and two more pin `"0.2.0"`. Under Cargo's 0.x caret rules `"0.6.0"` means
`<0.7.0`, a ceiling below 0.7 and this release. They stay green against the older definition of
"done" until they move.

### 0.7.1 → 0.7.2: the report escapes the endpoint id (ledger #1392)

**What changed.** The report printed a finding's detail with its control characters
escaped and the endpoint-id column raw, so an id holding a newline split one finding
across two lines. Every place the report prints an endpoint id (a finding, `probed:`,
`opted out:`, `declared pure/cacheable/live:`, `fixture:`, `collapsed:`, `unprobed:`)
now escapes its control characters (`two\u{a}lines`); an ordinary id prints exactly as
declared. `Finding::endpoint` and the other fields keep the id as declared, so
`Report::against` still matches it.

**Will it turn a green suite red?** No. Only the printed text changes, and only for an id
holding a control character.

### 0.7.0 → 0.7.1: `VALIDATE` on core 0.1.100 (ledger #1385)

**What changed.** The `ikigai-core` and `ikigai-vocab` floors are `0.1.100`, the release
whose emitters encode a name (`[` and `]` included) and degrade an authored IRI that is
not one to a literal. An id like `http://[::1]/x` passes `VALIDATE` and its Meta Turtle
parses, and a test pins both. The `VALIDATE` finding no longer says a refused value
breaks the Meta Turtle and contract IRIs: they parse, but not as written, so the check is
the strict layer on an authoring mistake. Core 0.1.100 also quotes the refused value
escaped (`two\nlines`) in the finding.

**Will it turn a green suite red?** No finding changed what it accepts or refuses. A
module pinned below `ikigai-core` 0.1.100 resolves up.

### 0.6.x → 0.7.0: `VALIDATE` (ledger #1361)

**What is new.** A thirteenth check, `VALIDATE`: every description the walk reaches
is run through `Description::validate()`, and a refusal is a finding naming the
endpoint, the field and the value. See
[Why `VALIDATE` is here and not in `bind`](#why-validate-is-here-and-not-in-bind).

**Will it turn a green suite red?** Only for a module with a name core refuses, and a
survey before this release found none. Every endpoint of the published module crates
whose `space()` a scratch crate could build (cms, compress, diagram, encrypt, fn,
jsonld, lisp, markdown, name, repo, runbook, sexpr, shacl, text, sign, xslt, sniff,
rdf, sparql, tz, fs, org, browse with and without its annotation family) passed, and
so do `ikigai-core`'s builtins (`tests/builtins.rs`). Not surveyed, because their
spaces need a host-injected transport, backend or configuration: http, llm, secret,
nl, meeting, time. What can go red on the upgrade is the floor: `ikigai-core` is now
`0.1.99` (the release whose `validate` refuses a code point no IRI carries), so a
module pinned below it resolves up. A module pinned at `"0.6.x"` is ceilinged below
0.7.0 and stays green until it moves.

**Source changes for a consumer:** `Check::ALL` is `[Check; 13]`, and `Check::Validate`
sits after `Check::Names` in report order, so the discriminants of `SpaceName` and
`Declarations` move up by one (`Check` is `#[non_exhaustive]`; nothing here exposes the
discriminant).

### 0.6.1 → 0.6.2: an endpoint about its caller names one (ledger #1317)

**What was wrong.** Every probe that judges an answer resolved under root, and root
holds every principal and names none. An endpoint that answers about whoever asks
and refuses a caller naming nobody (`ikigai-layers`'s `urn:layers:me`) refused every
probe, so `OUTPUTS`, `SKOLEM-RDF`, `VOCABULARY` and `CACHEABLE` could only report
the refusal or be waived one by one. A `live` declaration on it was printed as
`declared live:` while nothing had been checked.

**What changed.** `Fixture::new(id, verb).capability(cap)` names the caller for that
action, and the walk resolves it as that caller. See
[An endpoint about its caller names one](#an-endpoint-about-its-caller-names-one-fixturecapability).
`ENFORCED` and `AUTHORITY` still probe under no grants. A `Denied` under root now
names this fix, and `DECLARATIONS` reports a caller that no probe resolved as.

**Will it turn a green suite red?** No. Root stays the default: an action no fixture
names a caller for is probed exactly as before, and the `DECLARATIONS` rule only
reads fixtures that name one. The one text change any adopter can see is the
`Denied`-under-root finding, which gained a sentence naming the fix. An adopter that
waived the answer-judging checks for an endpoint about its caller can replace those
waivers with one fixture. The `ikigai-vocab` pin moves to the head, 0.1.96, per the
standing rule.

### 0.6.0 → 0.6.1: an `Expiry::At` deadline is a bound (ledger #1000)

**What was wrong.** `CACHEABLE`'s purity rule fired on any cacheable result with no
golden thread but its own name, and said it "will be served forever with nothing to
cut it". That is false of an `Expiry::At` answer: the kernel stops serving it at the
deadline, cut or not. A clock reading cacheable to the minute (`ikigai-tz`'s
`tz-now`) is not a pure function and has no state to name a thread for, so every
such module needed an opt-out or a `pure` declaration it could not honestly make.

**What changed.** The rule holds only `Expiry::Never`, the one expiry that is
unbounded. Every `At` counts as a bound, however distant: the suite judges the kind
of bound, not its length. Nothing else in `CACHEABLE` moved: an `At` answer on a
clockless kernel is still reported as recomputing (the kernel declines to cache a
deadline it cannot read), and the `live` rule still reports an `At` answer as
cacheable.

**Will it turn a green suite red?** No; the check refuses less. A `pure` already
declared on an `At` endpoint is still counted as consulted, not reported inert,
because the same endpoint may answer `Never` under another kernel (a pinned clock).
An `opt_out_check(…, Check::Cacheable, …)` taken only for this reason can go.

**Also in 0.6.1: `Check::ALL` is derived, not hand-kept (ledger #1035).** A `Check`
variant left out of the hand-written `ALL` array compiled, passed every test, and
was silently never selected by `Checks::all()` nor listed as skipped: a check that
existed and never ran. `ALL` is now generated from the enum declaration by one
macro, so it cannot omit a variant. No consumer-visible change: `ALL` is still
`[Check; 12]` in the same order.

**Also in 0.6.1: "its own name" is the canonical target (ledger #596).** The purity
rule compared an answer's threads against the name the walk LISTED. A space that lists
a logical name and resolves it onto a backing one, reporting the rewrite through
`Resolved::canonical`, has the kernel hang the read on the BACKING name, so that
thread read as foreign and the rule could not fire for any endpoint behind such a
space: the one shape of "`CACHEABLE` is dead since core 0.1.73" (ledger #596) that
0.5.0 left standing, and documented there as a limit. The rule now also counts the
canonical target the kernel reports on the resolution's root trace event as its own
name. Core's `Alias` lists backing names, so the walk already probed those; this
reaches a hand-written renaming space. **Will it turn a green suite red?** Only behind
such a space, on an endpoint that caches state it never named: the finding the rule
exists for. The rest of ledger #596's claim does not hold on 0.5.0 or later, and
`tests/violations.rs` pins it: the rule fires on a Source, an Exists, a template-bound
name, a required argument and a `Mount` whose only thread is their own name, and
stays quiet on a real foreign thread.

### 0.5.x → 0.6.0: `SPACE-NAME` (ledger #987)

**What is new.** A twelfth check, `SPACE-NAME`, and the two declarations it reads,
`Suite::self_named_space` and `Suite::host_named_space`. See
[A space's name](#a-spaces-name-space-name).

**Will it turn a green suite red?** Not by itself: a module that declares no space
is held to nothing, and its report gains one `space: none declared` line. What can
go red on the upgrade is the floor: `ikigai-core` is now `0.1.89` (the API minimum,
for `space_iri` and `SPACE_PREFIX`) and `ikigai-vocab` `0.1.89`, so a module pinned
below either resolves up.

**Source changes for a consumer:** `Check::ALL` is `[Check; 12]`; `Declarations`
gained a `spaces` field (a struct literal of it needs one more line), with the new
`DeclaredSpace` and `SpaceNaming` types.

### 0.5.0: purity is "no thread but its own name" (ledger #549), and it may turn your green suite red

**What was wrong.** Since `ikigai-core` 0.1.73 the kernel hangs every cacheable
`Source`/`Exists` answer on the thread named for its own canonical target (ledger
#512 hole A, formalism R4.4), so an empty thread set can no longer be observed on a
cacheable read. `CACHEABLE`'s purity rule was spelled "the thread set is empty", so
on any core past 0.1.72 it **fired for nothing**: an endpoint caching state it never
named, which this rule exists to catch, reported clean. A crate that commits no lock
(this one) resolved past 0.1.72 on its first fresh build, and its own pinned
findings went red.

**What changed.** The rule is now "no thread but its own name": a cacheable result
whose threads are all the endpoint's own name, from an endpoint not declared
`Suite::pure`, is a finding — **unless the endpoint declares a `Sink` or `Delete`**.
A write through that name fires the kernel's auto-cut on exactly the thread the
kernel hung the read on, so a read/write resource needs no `depends_on(itself)` any
more, and is clean without one. `tests/violations.rs` pins the premise against the
core it resolves (the own-name thread is there, and a `Sink` through the name drops
the cached read) as well as the verdict.

**Floor.** `ikigai-core` is raised from `0.1.67` to `0.1.73`. No API this crate calls
changed; the exemption above is right only on a kernel that hangs the read on its
own name, so below 0.1.73 it would pass a read a write never invalidates.

**What to expect.**

- An endpoint that caches state it never named goes red again — the rule was blind
  to it on every core past 0.1.72. Name the state's thread, or declare it pure.
- ⚠ **A thread named after the endpoint itself reads as no thread.** A Source-only
  endpoint that `depends_on` its own IRI (the old recipe for "my state is me") is
  indistinguishable from one that names nothing, and is reported. If something
  outside the endpoint really cuts that name (a watcher, `urn:kernel:cut`), name the
  thread after the state rather than the endpoint, or opt out with a reason.
- A read/write endpoint that was red for lacking `depends_on(itself)` goes clean.
- Own name means the name the walk probed. An endpoint reached through an `Alias`
  hangs from its BACKING name, which reads here as a thread besides its own, so the
  rule cannot see it (`Kernel::canonicalize` is private; see "What stays prose").
  Closed in 0.6.1, which reads the canonical target from the trace (ledger #596).

### 0.3.0 → 0.4.0: the walk fires once per REQUEST, and it may turn your green suite red

**What was wrong.** The walk probed per bound ENTRY. Every invoking check —
`ENFORCED`, `AUTHORITY`, `SKOLEM-RDF`, `VOCABULARY`, `CACHEABLE`, `PIPELINE`,
`OUTPUTS` — ran once per entry, so an endpoint reachable at two bindings had its
destructive actions **fired twice**, and the second firing ran against the state the
first one left. A module that had done nothing wrong was reported red, and the
README's own footprint contract ("a `Sink` or `Delete` declaring `content` is fired
once") was true only for an endpoint bound at exactly one pattern. The contract and
the code disagreed; the code has been changed to the contract.

**What changed.** The invoking checks are now memoized on the **request** the walk
would issue — verb, target IRI, arguments. Nothing else about the walk moved: the
template checks are still per entry (two patterns really are two places, each with
its own variables) and the description-only checks are still per description id.

**Why the request and not `Description::id`.** Because an id is not a unique name for
a thing that can be fired. `ikigai-cli` binds `ikigai_fs::FileEndpoint` at both
`urn:file:{path}` (jailed to a scratch root) and `urn:orgfile:{path}` (jailed to
whatever the config names — with no config at all, the process's working directory).
Both describe as `file`. Memoizing by id collapses those to whichever the walk reaches
first: either the dangerous entry is never probed and the report is green about
something it never looked at, or it is fired and the safe entry's coverage is lost.
Both outcomes are worse than firing twice, because today at least both are visible.
A request is the opposite — two entries sharing one can differ only in the state the
first firing left. `tests/violations.rs` pins both halves by **counting firings**, so
the trap cannot be re-entered quietly.

**What to expect.**

- A suite that was red **because** of double-firing goes green. That is the fix.
- A suite that was accidentally passing because a *second* firing masked something —
  a first write that made the second one's precondition true, a cache the first
  resolution warmed — may go red. Read it: the single firing is the honest one.
- `Report.actions` can come out **lower** than before for a module whose space lists
  a binding twice. It is not covering less; the duplicates were the same request. The
  new `collapsed:` line names them, which is what the report could not say before.
- Nothing changes at all for a module no binding of which is reachable twice.

**New: `Suite::opt_out_at(pattern, verb, reason)`** — the same identity problem on the
exclusion side. `opt_out` is scoped by id, so opting out `file` above would have
dropped the safe binding too; `opt_out_at` names one binding by its pattern, verbatim
as the space reports it. An `opt_out_at` that excluded nothing is a `DECLARATIONS`
finding, like every other declaration.

**API.** Additive except for two shapes that only affect code CONSTRUCTING a report:
`Declarations` gained `opted_out_at`, and `Report.unprobed` is now
`Box<Vec<Unprobed>>` rather than `Vec<Unprobed>` — it derefs, so `push`, `len`,
`is_empty` and `iter` read exactly as before and only a bare `for u in
&report.unprobed` needs `.iter()`. (The box is not taste: `Report` is the `Err` of
`check` and clippy's large-error bar is 128 bytes, which the new `collapsed` field
would otherwise cross.) `Report` gained `collapsed`; `Collapsed` and `OptedOutAt` are
new exports.

### 0.2.0 → 0.3.0: `AUTHORITY`, and what it will find

One new check, on by default, so this is a minor bump for the same reason 0.2.0
was: a new default-on check produces new findings across the fleet, and a patch
would be swallowed silently by every caret pin. `Check::ALL` also grew from
`[Check; 10]` to `[Check; 11]`, which only matters to code that binds it to a
sized array.

**What `AUTHORITY` is.** `ENFORCED` walks a 2x2 — declared or not, refused or
not — and reports three of the four cells. The fourth, *declares nothing and
resolved anyway*, is correct for a `Source`: serving a public read is a decision
a module gets to make. For a `Sink` or a `Delete` it is a hole. The write
happened for a caller holding nothing, so there is no scope to withhold from
anyone, and read and write cannot be told apart on that action: a sub-agent that
should read state and report a verdict cannot be handed read alone, because read
is all the manifold has. That invariant is usually written down as a sentence
asking the other party not to write. A capability is the same sentence the kernel
enforces, and this check is whether the module left one there to enforce.

**What to expect.** It fires only on evidence — a mutating action that declares
nothing **and resolved** under a capability holding no grants. Against the live
host at the time of writing, 4 of 26 mutating actions declare no capability, and
the ones that resolve are the finding. Two shapes come up:

- **A demo, a scratch buffer, a test double.** Waive it and the reason is in the
  record: `opt_out_check(id, Check::Authority, "in-process demo state")`.
- **A real write nobody gated.** Declare the scope on the action
  (`.requires("urn:cap:…")`); the kernel enforces declared scopes, so the same
  edit makes the check silent and the write refusable.

An action that declares nothing and is refused for some other reason is neither:
it is an `unprobed:` line naming what the walk could not see, because a probe
that silently matched nothing reads exactly like a pass.

**No new footprint.** `ENFORCED` already issued this resolution; both checks now
read one shared no-grants probe, so a `Sink` is fired there exactly once
regardless of which of the two you select.

### 0.1.1 → 0.2.0: a deliberate bump, and it may turn your green suite red

**Why a MINOR bump for what looks like a patch.** Two reasons, and the second is
the one that decided it. `Probed` gained a field and became `#[non_exhaustive]`,
so code that CONSTRUCTS one no longer compiles — a breaking change, and under
Cargo's 0.x caret rules a patch release would have been swallowed silently by
every consumer's pin. That is precisely the shape that cost this ecosystem a
published crate for two days when an upstream dependency did it to us, and we do
not get to file the incident and then repeat it. The second reason: `DECLARATIONS`
is expected to produce correct new findings across the fleet, and a minor bump
means each repo adopts it by changing a pin and reading this section, rather than
discovering it at whatever moment CI next runs.

So: **update your pin to `"0.2.0"` deliberately.** `DECLARATIONS` is new and on by default, so a
declaration that never reached a check becomes a finding at the next CI run — and
those are correct findings: the declaration was doing nothing before and the
report said otherwise. What to expect, in the order modules hit it:

- **`Suite::live` on a mutating verb** — the most likely one, since `live` is new
  in 0.1.1 and "declare it on every id" was the obvious first move. Drop it from
  the `Sink` and `Delete` ids; keep it on the cacheable ones.
- **`Suite::pure` on an endpoint whose result is not cacheable.** `pure` only
  exempts a cacheable result from the golden-thread rule; over an uncacheable one
  it exempted nothing. Drop it.
- **A `Suite::namespace` no probed face used** — often because the terms are now
  in `ikigai-vocab`, which is the good case. Drop the registration.
- **A fixture, waiver or opt-out naming an id the walk does not reach** — almost
  always the bound IRI rather than the `Description::id`, or a rename that left
  the declaration behind.

Keeping an inert declaration on purpose is `opt_out_check(id,
Check::Declarations, "why")`, or `Checks::all() - Checks::DECLARATIONS` for the
whole check.

Report text changed in three places, which matters only to a test asserting on it:
identical `(check, reason)` waivers now group onto one line listing their ids (one
id reads exactly as before); `checked:` stars a partially-waived check and adds a
count line; `probed:` covers every face rather than only RDF ones, so the header
reads `probed N face(s)` and a non-RDF line ends `N byte(s)`. One API break, and
only for code that CONSTRUCTS a `Probed` (reading it is unaffected): it gained a
`bytes` field and is now `#[non_exhaustive]`, so future additions are not breaks.

⚠ **0.1.0 → 0.1.1 was a patch, and a repo with no committed lockfile took it
through its existing caret pin** — so `OUTPUTS` arrived with no commit of ours at
all, whenever CI next ran. This release does not behave that way, on purpose: a
minor bump is outside every existing caret, so nothing changes for you until you
raise the pin.

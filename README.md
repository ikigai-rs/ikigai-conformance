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
checked: ARGSPECS REQUIRES-VERB ENFORCED AUTHORITY OUTPUTS SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE NAMES DECLARATIONS
probed 3 face(s) across 3 endpoint(s)
probed: cms-graph source `text/turtle`: 412 triple(s)
probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was checked
probed: tag-suggest source `text/plain`: 96 byte(s)
declared live: link-remove
fixture: sparql-construct source query="CONSTRUCT WHERE { ?s ?p ?o }"
unprobed: link-remove delete OUTPUTS: never fired under root: a mutating action is fired only by the pipeline probe (PIPELINE, on an action declaring `content`), so what it serves was not observed
```

The `probed:` lines are the positive half: a clean report would otherwise be
indistinguishable from a never-probed one — an endpoint whose face is undeclared,
unreachable or opted out produces no line and no finding, exactly like one whose
face is perfect. The triple count says whether the pass meant anything, and a
walk that resolved nothing says so rather than printing no section at all.

The last finding is the same idea turned on the module's own declarations: a
`live` on a `Delete` reached no check, changed nothing, and was printed in a clean
report exactly like one that had been honoured. See
[What the walk did NOT do](#what-the-walk-did-not-do).

The module recipe in the ikigai field guide is a page of prose every author must
remember. The checkable rules are now one test; the prose rule becomes one line
pointing at the check.

## The checks

| check | recipe row | what it sees |
|---|---|---|
| `ARGSPECS` | ArgSpecs from day one | at least one action per description; every input has an IRI `class`; a `default` is one of the `one_of` values when both exist; input names unique per action; every template variable (`urn:file:{path}`) is a declared `.binding()` input |
| `REQUIRES-VERB` | declared = enforced | a `requires` with no verb — enforced on every verb since core 0.1.85 (it was silently inert before), but the catalog cannot say which actions it gates |
| `ENFORCED` | declared = enforced | under a capability holding no grants, every action with a `requires` is refused with a typed `Denied`; an action declaring nothing is not (an undeclared enforced scope makes the manifold over-offer) |
| `AUTHORITY` | declared = enforced | the fourth cell of `ENFORCED`'s own probe: a `Sink` or a `Delete` that declares no `requires` and **mutated anyway** under a capability holding no grants. Nothing gates the write, so nothing can be withheld — a party that should read and report cannot be given read alone, because read is all there is. A `Source` is not in scope (a public read is a decision a module makes); a mutating action refused for some *other* reason is recorded as `unprobed`, never as a finding, because what an ungranted caller could do through it was not observed |
| `OUTPUTS` | faces are declared | the bare media type the action serves with its minimal inputs (`;charset=` and other parameters stripped, no `as=`) is one of its declared `outputs`. A wrong declaration hides a face from every consumer that reads outputs — `SKOLEM-RDF` and `VOCABULARY` included, which filter the declaration for RDF faces before probing; linkeddata's `sparql-construct` declared only `application/sparql-results+json` over Turtle for its whole life and the RDF checks saw nothing. What the check cannot observe (a mutating action never fired under root, a failed minimal resolution, a caller's `as=` label) is printed as `unprobed`, never as a finding |
| `SKOLEM-RDF` | skolemize; no blank nodes | every declared RDF face (`text/turtle`, `application/ld+json`, `application/rdf+xml`, N-Triples, N-Quads, TriG) resolves with the smallest inputs its ArgSpecs allow, parses, and has no blank node |
| `VOCABULARY` | faces use the shared vocabularies | the face **parses**, and every predicate and class in it is defined in `ikigai-vocab`, or under a well-known namespace (rdf, rdfs, xsd, owl, dcterms, foaf, schema, prov, ical, skos, sh) or one the module registers. Because it parses, it also reports an unresolvable, mislabeled or malformed face — under its own name when `SKOLEM-RDF` is not selected, so `VOCABULARY` alone proves a hand-written `@prefix` line is well-formed |
| `CACHEABLE` | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and — if it never expires (`Expiry::Never`; an `Expiry::At` deadline is a bound) — carries a golden thread besides its own name unless the endpoint is declared pure or takes writes through that name; a result declared live (`Suite::live`) is `Expiry::Always` |
| `PIPELINE` | pipeline citizenship | a mutating action with by-value inputs declares `content` (where a pipe's value and a `sink`'s body arrive); an action declaring `content` reads it |
| `NAMES` | naming convention | the description id is a kebab-case noun (`tag-suggest`, `kernel-catalog`) — the MCP projection derives an agent's tool name from it |
| `SPACE-NAME` | a name is a claim: same name, same doors | a space the module declares **self-named** claims `space_iri("<module>")` (`urn:iki:space:<module>`), on its topology root too, and two calls of its constructor claim the same name over the same doors; a space it declares **host-named** claims nothing. See [A space's name](#a-spaces-name-space-name) |
| `DECLARATIONS` | — | every declaration the module made (`live`, `cacheable`, `pure`, `namespace`, a `Fixture`, an `opt_out`, an `opt_out_check`, a declared space) reached the check that would honour it. A `live` on a `Sink` reached nothing and the report printed `declared live:` anyway |

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

## Fixtures, opt-outs, declarations

The invoking checks resolve each action with the smallest inputs its ArgSpecs
admit (`one_of[0]`, a value shaped by the XSD `class`, `urn:example:conformance`
for an entity, `x` otherwise). They run **against the kernel you pass**, so build
it as a test fixture. When the spec cannot say what a valid call is, or an action
has real side effects, say so — and what you said is printed in the report, the
fixtures included (`fixture: file source path="README.md"`):

```rust
use ikigai_conformance::{Check, Fixture, Suite};
use ikigai_core::Verb;

Suite::new()
    .fixture(Fixture::new("jsonld-expand", Verb::Source).arg("content", "{}"))
    .fixture(Fixture::new("file", Verb::Source).binding("path", "README.md"))
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
`checked:` line stars a check that ran on only some endpoints.

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

**What a walk fires, under root** — the footprint a fixture author can count on.
It is stated **per FIRING, and a firing is a request**: the verb, the target IRI,
and the arguments. Per firing: a `Source` or `Exists` is resolved once (twice when
cacheable; the second is the cache probe), each declared RDF face beyond the first
is resolved once more with `as=`, and a `Sink` or `Delete` declaring `content` is
fired **once**, by the pipeline probe — `OUTPUTS` reads that same firing rather than
making another. A mutating action without `content` is never fired under root
(`ENFORCED` runs under no grants and, when the gate is declared, never reaches the
endpoint); the report lists it as `unprobed`. Sinks land: capture what a fixture
reads before the walk and assert effects after it.

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

**Counts.** `Report.endpoints` counts distinct description ids; `Report.actions`
counts one per distinct FIRING — `urn:a11y:config` and `urn:a11y:config:{app}`
sharing one description are one endpoint and, with one `Source` each, two actions,
because they resolve to different IRIs. Two bindings that resolve to the SAME IRI
with the same arguments are one action, and `Report.collapsed` names them.
`Report.walked` is that first count BY NAME, in walk
order, so a test can assert the walk reached exactly the endpoints the module
means to bind (an endpoint that stops being bound is otherwise a report that gets
*cleaner*).

**One honest exception to "every input has a class".** An opaque-bytes input
(`urn:sniff`'s `content`: a PNG is valid) has no XSD datatype that is true of the
value; declare `xsd:string` — the type the wire carries — and say so in a comment.

The kernel's own `urn:kernel:*` operations are core's, not the module's: the walk
skips them unless `Suite::include_kernel_ops()` asks.

## What the walk did NOT do

A clean report is worth exactly what it covered, so the report says what it did
not reach as loudly as what it found.

```
0 finding(s) across 6 endpoint(s), 9 action(s)
checked: ARGSPECS REQUIRES-VERB ENFORCED AUTHORITY OUTPUTS* SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE NAMES DECLARATIONS
* OUTPUTS ran on 1 of 6 endpoint(s); waived on the rest (see `opted out:`)
probed 7 face(s) across 5 endpoint(s)
probed: cms-graph source `text/turtle`: 412 triple(s)
probed: ik-context source `application/ld+json`: 0 triple(s) — nothing was checked
probed: wc source `text/plain`: 3 byte(s)
opted out: a-face b-face c-face VOCABULARY: ik:madeUp lands in the next vocabulary release
unprobed: notes-delete delete OUTPUTS: never fired under root
```

- **`probed:`** — every face the walk actually resolved, not only the RDF ones.
  For an RDF face the count is triples, and **0 triples says `nothing was
  checked`** (both RDF checks pass vacuously over an empty graph); for a face no
  check parses it is bytes, evidence that the action was reached and served
  something. A walk that resolved nothing at all says so on one line rather than
  printing no section: a module with no graph face used to be indistinguishable
  from a walk that reached nothing.
- **`unprobed:`** — the actions a check could not observe, with the reason.
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
about a check that will honour it, and one whose target the walk never reaches is
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
| a `Fixture::binding` naming no template variable of any pattern that id is bound at | `binds \`number\`, which is not a template variable …` |
| an `opt_out` that excluded nothing | `opted out … but excluded nothing: …` |
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
  endpoint declaring none of them is held to none of them.
- **`Source` pipeline routing.** The engine routes a piped value into the sole
  unnamed required input by contract; nothing an endpoint does makes that right or
  wrong. Newline-separated list output (the `..` map convention) is a shape no
  ArgSpec states.
- **A declared golden thread is a promise a host must keep.** `CACHEABLE` checks
  the thread set names something besides the endpoint, not that anything cuts it; a module declaring
  `urn:file:` threads over a config home no host watches is clean here. The suite
  cannot see the host.
- **"Required" that is actually optional.** The minimal call supplies every
  required input, so an input marked required that the endpoint does without is
  invisible to `ARGSPECS`.
- **A face built over a bundled graph tests that graph too.** A module that folds
  `ikigai-vocab` into every result is probed OVER it; a blank node in a future
  vocabulary release fails that module's walk first.
- **A pass-through output.** An action that serves whatever its origin says has no
  `Description` spelling in core (`outputs` is a closed list), so `OUTPUTS` reports
  whatever the fixture's origin serves; such a module subtracts `Checks::OUTPUTS`
  and says why.
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
`ikigai-core`'s own `toUpper` / `reverseList` / `echo` (which predate the recipe)
and pins the exact findings: three untyped inputs, two pre-convention ids, three
cacheable pure functions nobody declared pure — and nothing else.

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

0.6.1. Depends only on published crates (`ikigai-core`, `ikigai-vocab`,
`oxrdfio`). Dual-licensed MIT / Apache-2.0.

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

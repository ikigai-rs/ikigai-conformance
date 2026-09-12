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
tag-suggest  CACHEABLE  source: cacheable with an empty golden-thread set: it will be served forever with nothing to cut it. `Suite::pure(id)` if it is a pure function of its inputs; otherwise `depends_on` the thread of the state it reads
link-remove  REQUIRES-VERB  declares requires `urn:cap:fs:write:*` but no verb: `action_specs()` iterates verbs, so this scope is silently inert — the kernel enforces nothing (add `.verb(…)`)
cms-graph  SKOLEM-RDF  source: the `text/turtle` face has 4 blank node(s) (_:b0, _:b1, _:b2, …): skolemize — mint a stable IRI per node (`urn:ikigai:endpoint:{id}:…`, `urn:event:{uid}`), never a counter
cms-graph  VOCABULARY  source: the `text/turtle` face uses `https://ikigai-rs.dev/ns#shelf`, which ikigai-vocab does not define and no well-known or registered namespace covers: an invented term with no definition (…)
sparql-construct  OUTPUTS  source: served `text/turtle` with its minimal inputs but declares only `application/sparql-results+json`: a face the manifold does not announce and the RDF checks never saw — declare it (`.output("text/turtle")`) or serve what is declared
link-remove  DECLARATIONS  declared live (`Suite::live`) but nothing held it to it: it declares no cacheable verb (it declares delete), and CACHEABLE returns early on a mutating one. The report prints `declared live: link-remove`, which reads as a check that ran
7 finding(s) across 4 endpoint(s), 4 action(s)
checked: ARGSPECS REQUIRES-VERB ENFORCED OUTPUTS SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE NAMES DECLARATIONS
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
| `REQUIRES-VERB` | declared = enforced | a `requires` with no verb — a floor `action_specs()` never yields, so the kernel enforces nothing, silently |
| `ENFORCED` | declared = enforced | under a capability holding no grants, every action with a `requires` is refused with a typed `Denied`; an action declaring nothing is not (an undeclared enforced scope makes the manifold over-offer) |
| `OUTPUTS` | faces are declared | the bare media type the action serves with its minimal inputs (`;charset=` and other parameters stripped, no `as=`) is one of its declared `outputs`. A wrong declaration hides a face from every consumer that reads outputs — `SKOLEM-RDF` and `VOCABULARY` included, which filter the declaration for RDF faces before probing; linkeddata's `sparql-construct` declared only `application/sparql-results+json` over Turtle for its whole life and the RDF checks saw nothing. What the check cannot observe (a mutating action never fired under root, a failed minimal resolution, a caller's `as=` label) is printed as `unprobed`, never as a finding |
| `SKOLEM-RDF` | skolemize; no blank nodes | every declared RDF face (`text/turtle`, `application/ld+json`, `application/rdf+xml`, N-Triples, N-Quads, TriG) resolves with the smallest inputs its ArgSpecs allow, parses, and has no blank node |
| `VOCABULARY` | faces use the shared vocabularies | the face **parses**, and every predicate and class in it is defined in `ikigai-vocab`, or under a well-known namespace (rdf, rdfs, xsd, owl, dcterms, foaf, schema, prov, ical, skos, sh) or one the module registers. Because it parses, it also reports an unresolvable, mislabeled or malformed face — under its own name when `SKOLEM-RDF` is not selected, so `VOCABULARY` alone proves a hand-written `@prefix` line is well-formed |
| `CACHEABLE` | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and carries a golden thread unless the endpoint is declared pure; a result declared live (`Suite::live`) is `Expiry::Always` |
| `PIPELINE` | pipeline citizenship | a mutating action with by-value inputs declares `content` (where a pipe's value and a `sink`'s body arrive); an action declaring `content` reads it |
| `NAMES` | naming convention | the description id is a kebab-case noun (`tag-suggest`, `kernel-catalog`) — the MCP projection derives an agent's tool name from it |
| `DECLARATIONS` | — | every declaration the module made (`live`, `cacheable`, `pure`, `namespace`, a `Fixture`, an `opt_out`, an `opt_out_check`) reached the check that would honour it. A `live` on a `Sink` reached nothing and the report printed `declared live:` anyway |

Every check is independently selectable, so a module adopts incrementally, and
every skipped check is printed as skipped:

```rust
use ikigai_conformance::{check_with, Checks};

check_with(&my_kernel(), Checks::all() - Checks::RDF).unwrap();
```

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
    .opt_out_check("review", Check::Vocabulary, "ik:quote lands in vocab 0.1.70")
    .namespace("https://example.org/cms#")   // a vocabulary this module serves
    .pure("wc")                              // a pure function: no thread expected
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

Where the waiver can be made exact, prefer that: **`ikigai_conformance::rdf` is
public**, and `rdf::parse` + `rdf::terms` + `rdf::is_defined` reproduce
`VOCABULARY` exactly. A module can pin its undefined terms as an EXACT list that
goes red in both directions — a new invented term fails, and so does the day the
missing terms land, which makes the waiver self-destructing. That is strictly
better than `Suite::namespace` for a module's own namespace: a registration waives
every term under it forever, including the next one somebody invents.

**What a walk fires, under root** — the footprint a fixture author can count on:
a `Source` or `Exists` is resolved once (twice when cacheable; the second is the
cache probe), each declared RDF face beyond the first is resolved once more with
`as=`, and a `Sink` or `Delete` declaring `content` is fired **once**, by the
pipeline probe — `OUTPUTS` reads that same firing rather than making another. A
mutating action without `content` is never fired under root (`ENFORCED` runs
under no grants and, when the gate is declared, never reaches the endpoint); the
report lists it as `unprobed`. Sinks land: capture what a fixture reads before the
walk and assert effects after it.

**Fixture bindings are per entry, not per verb.** Every action of an entry
resolves the same IRI, so the verb on a binding-only fixture is ignored (the first
fixture for the id that binds the variable wins); arguments are per `(id, verb)`.

**Counts.** `Report.endpoints` counts distinct description ids; `Report.actions`
counts one per bound entry per verb — `urn:a11y:config` and
`urn:a11y:config:{app}` sharing one description are one endpoint and, with one
`Source` each, two actions. `Report.walked` is that first count BY NAME, in walk
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
checked: ARGSPECS REQUIRES-VERB ENFORCED OUTPUTS* SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE NAMES DECLARATIONS
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
  the thread set is non-empty, not that anything cuts it; a module declaring
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
that breaks nothing, asserting the suite is clean on it. Every `DECLARATIONS`
case is falsified the same way — a `live` on a `Sink`, a fixture for an id
nothing binds, a binding naming no template variable, a waiver for a check that
could not run, a namespace covering nothing — each asserting the silence is gone,
and the `live`-on-a-`Sink` test asserts `CACHEABLE` still says nothing, which is
the whole point. `tests/builtins.rs` runs the suite against
`ikigai-core`'s own `toUpper` / `reverseList` / `echo` (which predate the recipe)
and pins the exact findings: three untyped inputs, two pre-convention ids, three
cacheable pure functions nobody declared pure — and nothing else.

## Status

0.2.0. Depends only on published crates (`ikigai-core`, `ikigai-vocab`,
`oxrdfio`). Dual-licensed MIT / Apache-2.0.

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

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
5 finding(s) across 3 endpoint(s), 3 action(s)
checked: ARGSPECS REQUIRES-VERB ENFORCED SKOLEM-RDF VOCABULARY CACHEABLE PIPELINE NAMES
```

The module recipe in the ikigai field guide is a page of prose every author must
remember. The checkable rules are now one test; the prose rule becomes one line
pointing at the check.

## The checks

| check | recipe row | what it sees |
|---|---|---|
| `ARGSPECS` | ArgSpecs from day one | at least one action per description; every input has an IRI `class`; a `default` is one of the `one_of` values when both exist; input names unique per action; every template variable (`urn:file:{path}`) is a declared `.binding()` input |
| `REQUIRES-VERB` | declared = enforced | a `requires` with no verb — a floor `action_specs()` never yields, so the kernel enforces nothing, silently |
| `ENFORCED` | declared = enforced | under a capability holding no grants, every action with a `requires` is refused with a typed `Denied`; an action declaring nothing is not (an undeclared enforced scope makes the manifold over-offer) |
| `SKOLEM-RDF` | skolemize; no blank nodes | every declared RDF face (`text/turtle`, `application/ld+json`, `application/rdf+xml`, N-Triples, N-Quads, TriG) resolves with the smallest inputs its ArgSpecs allow, parses, and has no blank node |
| `VOCABULARY` | faces use the shared vocabularies | every predicate and class in a face is defined in `ikigai-vocab`, or under a well-known namespace (rdf, rdfs, xsd, owl, dcterms, foaf, schema, prov, ical, skos, sh) or one the module registers |
| `CACHEABLE` | cacheability | a result marked cacheable is a cache hit the second time (the kernel's trace says so), byte-identical, and carries a golden thread unless the endpoint is declared pure |
| `PIPELINE` | pipeline citizenship | a mutating action with by-value inputs declares `content` (where a pipe's value and a `sink`'s body arrive); an action declaring `content` reads it |
| `NAMES` | naming convention | the description id is a kebab-case noun (`tag-suggest`, `kernel-catalog`) — the MCP projection derives an agent's tool name from it |

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
has real side effects, say so — and what you said is printed in the report:

```rust
use ikigai_conformance::{Fixture, Suite};
use ikigai_core::Verb;

Suite::new()
    .fixture(Fixture::new("jsonld-expand", Verb::Source).arg("content", "{}"))
    .fixture(Fixture::new("file", Verb::Source).binding("path", "README.md"))
    .opt_out("email-send", Some(Verb::Sink), "sends real mail")
    .namespace("https://example.org/cms#")   // a vocabulary this module serves
    .pure("wc")                              // a pure function: no thread expected
    .cacheable("catalog")                    // marks .cacheable(): hold it to that
    .run_blocking(&my_kernel())
    .into_result()
    .unwrap();
```

`Suite::cacheable` exists because the kernel hands back the **effective** expiry —
the least cacheable of a result and its dependencies — so an endpoint that marked
its result cacheable over one volatile sub-resolution comes back indistinguishable
from one that never did. That silent downgrade once turned a 20µs read into a 1s
read on every request; declared, it is a red test.

The kernel's own `urn:kernel:*` operations are core's, not the module's: the walk
skips them unless `Suite::include_kernel_ops()` asks.

## What stays prose

Honest residue — what no check here can see:

- **The converse of declared = enforced** is approximated: a `Denied` from an
  action declaring nothing is caught; an endpoint that enforces an undeclared scope
  by *succeeding differently* (a filtered view) is invisible.
- **Whether a result should be cacheable**, and whether it is pure. The probe checks
  a cacheable result behaves like one; purity and "marks cacheable" are
  declarations the module makes (`pure`, `cacheable`).
- **`Source` pipeline routing.** The engine routes a piped value into the sole
  unnamed required input by contract; nothing an endpoint does makes that right or
  wrong. Newline-separated list output (the `..` map convention) is a shape no
  ArgSpec states.
- Reading through the kernel rather than `std::fs` (a lint, not a test); where a
  fix belongs; when in doubt, don't cache.

## Proof

`tests/violations.rs` binds one endpoint per check that breaks its rule on
purpose and asserts the check sees it — and one endpoint that breaks nothing,
asserting the suite is clean on it. `tests/builtins.rs` runs the suite against
`ikigai-core`'s own `toUpper` / `reverseList` / `echo` (which predate the recipe)
and pins the exact findings: three untyped inputs, two pre-convention ids, three
cacheable pure functions nobody declared pure — and nothing else.

## Status

0.1.0. Depends only on published crates (`ikigai-core`, `ikigai-vocab`,
`oxrdfio`). Dual-licensed MIT / Apache-2.0.

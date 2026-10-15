//! The forced-recomputation witness of CACHEABLE (ledger #330).
//!
//! The cacheable-twice probe's second resolution IS the cache hit, so "byte-identical"
//! compared the first computation with a copy of itself: `ikigai-encrypt` shipped
//! `.cacheable()` on age ciphertext (a fresh ephemeral key per call) and the walk said
//! nothing. The witness cuts every thread of the first result and resolves again, so
//! the third resolution is a genuinely independent computation; then it cuts each
//! DECLARED thread alone and does the same.
//!
//! Every test selects CACHEABLE and RECOMPUTE alone: the other invoking checks resolve
//! the action too (ENFORCED under no grants, in its own cache partition), and these
//! endpoints count their computations.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use ikigai_conformance::{Check, Checks, Report, Suite};
use ikigai_core::{
    Description, EndpointSpace, Exact, FixedClock, FnEndpoint, Invocation, Kernel, ReprType,
    Representation, Time, Verb,
};

const DIFFERENT: &str = "marked cacheable but a forced recomputation returned different bytes";
const NOW: u64 = 1_790_512_496_000;
const NEXT_MINUTE: u64 = 1_790_512_500_000;

fn text(s: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), s)
}

/// An endpoint whose `n`th computation (from 1) answers `answer(n)`, shaped by `shape`.
fn counting(
    answer: impl Fn(usize) -> String + Send + Sync + 'static,
    shape: impl Fn(Representation) -> Representation + Send + Sync + 'static,
) -> FnEndpoint {
    let computed = AtomicUsize::new(0);
    FnEndpoint::new("seal", move |_inv: &Invocation<'_>| {
        let n = computed.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(shape(text(answer(n))))
    })
    .with_description(
        Description::new("seal")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

fn walk(endpoint: FnEndpoint, suite: Suite) -> Report {
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), endpoint);
    suite
        .checks(Checks::CACHEABLE | Checks::RECOMPUTE)
        .run_blocking(&Kernel::new(Arc::new(space)))
}

#[test]
fn a_pure_result_that_differs_per_computation_is_caught() {
    // A fresh nonce per computation, declared pure: before the witness the second
    // resolution was the hit and the report was clean.
    let nonce = counting(|n| format!("nonce-{n}"), Representation::cacheable);
    let report = walk(nonce, Suite::new().pure("seal"));
    let found: Vec<_> = report.of(Check::Recompute).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].detail.contains(DIFFERENT), "{report}");
    assert!(
        found[0]
            .detail
            .contains("after a cut of the thread `urn:example:seal`"),
        "names what it cut:\n{report}"
    );
    assert!(report.recomputed().is_empty(), "{report}");
}

#[test]
fn a_threaded_result_that_differs_per_computation_is_caught() {
    // The ikigai-encrypt shape: a threaded recipient supplies the golden thread, so
    // the no-thread rule is satisfied and the hit is real; the ciphertext differs per
    // sealing.
    let ciphertext = counting(
        |n| format!("age-ciphertext-{n}"),
        |r| r.cacheable().depends_on("urn:example:recipient"),
    );
    let report = walk(ciphertext, Suite::new());
    assert!(
        report
            .of(Check::Recompute)
            .any(|f| f.detail.contains(DIFFERENT)),
        "{report}"
    );
}

#[test]
fn a_deterministic_threaded_result_recomputes_identically() {
    let notes = counting(
        |_| "hello".to_string(),
        |r| r.cacheable().depends_on("urn:example:notes-store"),
    );
    let report = walk(notes, Suite::new());
    report.assert_clean();
    let seen = &report.recomputed()[0];
    assert_eq!(
        (seen.endpoint.as_str(), seen.cut, seen.declared),
        ("seal", 2, 1)
    );
    assert!(
        report.to_string().contains(
            "recomputed: seal source 24 times after a cut of 2 thread(s), and of each of 1 \
             declared one alone: identical\n"
        ),
        "{report}"
    );
}

#[test]
fn every_recomputation_is_held_to_the_first() {
    // Computations 1 to 4 agree and the fifth differs: one recomputation (the third
    // resolution) saw the same bytes and walked clean. The fourth forced one is the
    // first to differ, and the finding says the earlier ones agreed by chance.
    let late = counting(
        |n| if n < 5 { "a=1\nb=2" } else { "b=2\na=1" }.to_string(),
        Representation::cacheable,
    );
    let report = walk(late, Suite::new().pure("seal"));
    let found: Vec<_> = report.of(Check::Recompute).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].detail.contains(DIFFERENT), "{report}");
    assert!(
        found[0]
            .detail
            .contains("(recomputation 4 of 24; the 3 before it repeated the first by chance)"),
        "{report}"
    );
    assert!(report.recomputed().is_empty(), "{report}");
}

#[test]
fn each_declared_thread_is_cut_alone_and_recomputed() {
    // Computation 1 is the first resolution, 2 to 25 the 24 forced ones after the full
    // cut (the same bytes), 26 the one after a cut of the declared thread alone
    // (different): only the narrower half reaches it.
    let drifting = counting(
        |n| {
            if n <= 25 {
                "v1".to_string()
            } else {
                "v2".to_string()
            }
        },
        |r| r.cacheable().depends_on("urn:example:db"),
    );
    let report = walk(drifting, Suite::new());
    let found: Vec<_> = report.of(Check::Recompute).collect();
    assert_eq!(found.len(), 1, "{report}");
    assert!(found[0].detail.contains(DIFFERENT), "{report}");
    assert!(
        found[0]
            .detail
            .contains("after a cut of the thread `urn:example:db`"),
        "{report}"
    );
}

#[test]
fn an_expiry_at_result_is_not_held_to_a_forced_recomputation() {
    // A deadline bounds the answer in time: a recomputation may differ by design.
    let reading = counting(
        |n| format!("12:34:{n:02}"),
        |r| r.cacheable_until(Time::from_millis(NEXT_MINUTE)),
    );
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), reading);
    let clocked = Kernel::new(Arc::new(space)).with_clock(Arc::new(FixedClock::at(NOW)));
    let report = Suite::new()
        .checks(Checks::CACHEABLE | Checks::RECOMPUTE)
        .run_blocking(&clocked);
    report.assert_clean();
    assert!(report.recomputed().is_empty(), "{report}");
}

#[test]
fn an_uncached_second_resolution_is_judged_by_the_byte_comparison_alone() {
    // An uncacheable answer is never recomputed by force: there is no hit to defeat.
    let live = counting(|n| format!("tick-{n}"), |r| r);
    let report = walk(live, Suite::new());
    report.assert_clean();
    assert!(report.recomputed().is_empty(), "{report}");
}

#[test]
fn recompute_runs_only_where_selected_and_is_waivable_with_a_reason() {
    let nonce = || counting(|n| format!("nonce-{n}"), Representation::cacheable);
    // A selection that does not name it (any written before it existed) does not run it.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), nonce());
    let report = Suite::new()
        .pure("seal")
        .checks(Checks::CACHEABLE)
        .run_blocking(&Kernel::new(Arc::new(space)));
    report.assert_clean();
    // Subtracted suite-wide, for a test that pins how often a fixture is read.
    let space = EndpointSpace::new().bind(Exact::new("urn:example:seal"), nonce());
    let report = Suite::new()
        .pure("seal")
        .checks(Checks::all() - Checks::RECOMPUTE)
        .run_blocking(&Kernel::new(Arc::new(space)));
    assert!(report.of(Check::Recompute).next().is_none(), "{report}");
    // Waived per endpoint, with the reason the answer differs by design.
    let report = walk(
        nonce(),
        Suite::new().pure("seal").opt_out_check(
            "seal",
            Check::Recompute,
            "the answer stamps when it was computed",
        ),
    );
    report.assert_clean();
    assert!(
        report
            .to_string()
            .contains("opted out: seal RECOMPUTE: the answer stamps when it was computed"),
        "{report}"
    );
}

#[test]
fn recompute_stands_down_when_cacheable_already_saw_two_computations_differ() {
    // Never stored (a volatile dependency would do the same): CACHEABLE's second
    // resolution recomputed and differed, and that is its finding, made once.
    let space = EndpointSpace::new().bind(
        Exact::new("urn:example:seal"),
        counting(|n| format!("nonce-{n}"), Representation::cacheable),
    );
    let kernel = Kernel::new(Arc::new(space)).with_cache_policy(Arc::new(
        ikigai_core::Lru::with_bound(ikigai_core::CacheBound::new(0, 0)),
    ));
    let report = Suite::new()
        .pure("seal")
        .checks(Checks::CACHEABLE | Checks::RECOMPUTE)
        .run_blocking(&kernel);
    assert!(report.of(Check::Recompute).next().is_none(), "{report}");
    assert!(report.of(Check::Cacheable).next().is_some(), "{report}");
}

/// Two entries in a `HashMap`, serialized in iteration order: each computation builds
/// a new map with a fresh `RandomState`, so two computations agree on the order about
/// half the time. The shape of the jsonld, shacl and rdf-union defects.
fn hash_ordered() -> FnEndpoint {
    FnEndpoint::new("seal", |_inv: &Invocation<'_>| {
        let entries: std::collections::HashMap<&str, &str> =
            [("a", "1"), ("b", "2")].into_iter().collect();
        let body: Vec<String> = entries.iter().map(|(k, v)| format!("{k}={v}")).collect();
        Ok(text(body.join("\n")).cacheable())
    })
    .with_description(
        Description::new("seal")
            .verb(Verb::Source)
            .output("text/plain"),
    )
}

#[test]
fn a_two_entry_hash_order_is_caught_on_every_walk() {
    // One forced recomputation of a two-entry map agrees with the first about half
    // the time (ikigai-jsonld's ANONYMOUS fixture: 5 catches in 12 runs). Repeated
    // recomputations, each held to the first, make a miss negligible: fifty walks,
    // every one must say so.
    let mut missed = 0;
    for _ in 0..50 {
        let report = walk(hash_ordered(), Suite::new().pure("seal"));
        if !report
            .of(Check::Recompute)
            .any(|f| f.detail.contains(DIFFERENT))
        {
            missed += 1;
        }
    }
    assert_eq!(missed, 0, "{missed} of 50 walks missed the order defect");
}

//! `race`: the probes see a check-then-act defect, and pass the endpoint that has none.
//!
//! Each defect here is the shape ledger #2772 audits for, built small: a counter that reads
//! and then writes in two critical sections, and a "first one wins" endpoint that reads a
//! flag through one sub-request and sets it through another. Each has a correct twin, and
//! each probe must tell the two apart.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ikigai_conformance::race::{Call, Interleave, Race};
use ikigai_core::{
    ArgRef, Capability, Description, Endpoint, EndpointSpace, Error, Exact, FnEndpoint, Invocation,
    Iri, Kernel, ReprType, Representation, Request, Result, Space, Verb,
};

fn text(body: impl Into<Vec<u8>>) -> Representation {
    Representation::new(ReprType::new("text/plain"), body.into())
}

// --------------------------------------------------------------------- Race::copies

/// A counter at `urn:test:lossy` (read, pause, write: two critical sections) and one at
/// `urn:test:atomic` (one critical section), both over `count`.
fn counters(count: Arc<Mutex<u64>>) -> Kernel {
    let lossy_count = Arc::clone(&count);
    let lossy = FnEndpoint::new("lossy", move |_inv: &Invocation<'_>| {
        let seen = *lossy_count.lock().unwrap();
        // The window: another call reads the same `seen` while this one waits.
        std::thread::sleep(Duration::from_millis(2));
        *lossy_count.lock().unwrap() = seen + 1;
        Ok(text("ok"))
    });
    let atomic = FnEndpoint::new("atomic", move |_inv: &Invocation<'_>| {
        *count.lock().unwrap() += 1;
        Ok(text("ok"))
    });
    Kernel::new(Arc::new(
        EndpointSpace::new()
            .bind(Exact::new("urn:test:lossy"), lossy)
            .bind(Exact::new("urn:test:atomic"), atomic),
    ))
}

/// Run 8 copies of `iri` for 20 rounds and hold the count to the successes.
fn eight_at_once(iri: &'static str) -> ikigai_conformance::race::RaceReport {
    let count = Arc::new(Mutex::new(0u64));
    let kernel = counters(Arc::clone(&count));
    let report = Race::over(&kernel).rounds(20).copies(
        8,
        |_kernel, _round| *count.lock().unwrap(),
        |_before| Call::new(Verb::Sink, iri),
        |_kernel, before, answers| {
            let succeeded = answers.iter().filter(|a| a.is_ok()).count() as u64;
            let moved = *count.lock().unwrap() - before;
            if moved == succeeded {
                Ok(())
            } else {
                Err(format!(
                    "{succeeded} calls succeeded and the count moved {moved}"
                ))
            }
        },
    );
    report
}

#[test]
fn copies_catch_a_lost_update() {
    let report = eight_at_once("urn:test:lossy");
    assert!(
        !report.holds(),
        "eight read-then-write increments lost nothing: {report}"
    );
    // A sleep between the read and the write loses updates in essentially every round.
    assert!(
        report.violations().len() >= 15,
        "the window is 2 ms wide and was hit in only {} of 20 rounds",
        report.violations().len()
    );
    let shown = report.to_string();
    assert!(shown.contains("the invariant broke in"), "{shown}");
    assert!(
        shown.contains("calls succeeded and the count moved"),
        "the invariant's own words are in the report: {shown}"
    );
}

#[test]
fn copies_pass_an_atomic_update() {
    let report = eight_at_once("urn:test:atomic");
    report.assert_holds();
    assert_eq!(report.rounds(), 20);
    assert_eq!(
        report.to_string(),
        "the invariant held in all 20 round(s) of 8 concurrent call(s)"
    );
}

#[test]
#[should_panic(expected = "the invariant broke in")]
fn assert_holds_panics_with_the_report() {
    eight_at_once("urn:test:lossy").assert_holds();
}

/// Every call in a round is in flight at once: the barrier releases them together, so a
/// call that waits for its peers sees all of them arrive.
#[test]
fn together_releases_every_call_at_once() {
    let inside = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let (i, p) = (Arc::clone(&inside), Arc::clone(&peak));
    let wait = FnEndpoint::new("wait", move |_inv: &Invocation<'_>| {
        let now = i.fetch_add(1, Ordering::SeqCst) + 1;
        p.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(20));
        i.fetch_sub(1, Ordering::SeqCst);
        Ok(text("ok"))
    });
    let kernel = Kernel::new(Arc::new(
        EndpointSpace::new().bind(Exact::new("urn:test:wait"), wait),
    ));
    let report = Race::over(&kernel).rounds(3).together(
        |_kernel, _round| (),
        |_| {
            vec![
                Call::new(Verb::Source, "urn:test:wait").arg("n", "1"),
                Call::new(Verb::Source, "urn:test:wait").arg("n", "2"),
                Call::new(Verb::Source, "urn:test:wait").arg("n", "3"),
            ]
        },
        |_kernel, _, answers| {
            if answers.len() == 3 && answers.iter().all(|a| a.is_ok()) {
                Ok(())
            } else {
                Err(format!("{answers:?}"))
            }
        },
    );
    report.assert_holds();
    assert_eq!(peak.load(Ordering::SeqCst), 3, "the three calls overlapped");
}

/// `Race::fresh` builds a kernel per round, and answers come back in the order given.
#[test]
fn fresh_builds_a_kernel_per_round_and_keeps_the_call_order() {
    let built = AtomicU64::new(0);
    let report = Race::fresh(|| {
        built.fetch_add(1, Ordering::SeqCst);
        let echo = FnEndpoint::new("echo", |inv: &Invocation<'_>| {
            Ok(text(inv.inline_str("say")?.to_string()))
        });
        Kernel::new(Arc::new(
            EndpointSpace::new().bind(Exact::new("urn:test:echo"), echo),
        ))
    })
    .rounds(4)
    .together(
        |_kernel, round| round,
        |round| {
            ["a", "b", "c"]
                .iter()
                .map(|s| Call::new(Verb::Source, "urn:test:echo").arg("say", format!("{s}{round}")))
                .collect()
        },
        |_kernel, round, answers| {
            let said: Vec<String> = answers
                .iter()
                .map(|a| String::from_utf8_lossy(&a.as_ref().unwrap().bytes).into_owned())
                .collect();
            let want: Vec<String> = ["a", "b", "c"]
                .iter()
                .map(|s| format!("{s}{round}"))
                .collect();
            if said == want {
                Ok(())
            } else {
                Err(format!("answers out of order: {said:?}"))
            }
        },
    );
    report.assert_holds();
    assert_eq!(built.load(Ordering::SeqCst), 4);
}

// ------------------------------------------------------------------------ Interleave

/// `urn:test:flag`: Source answers `set` or `unset`, Sink sets it, and with `if-unset=true`
/// sets it only if unset, answering whether this call changed it (a compare-and-set).
fn flag_space(flag: Arc<Mutex<bool>>) -> EndpointSpace {
    let endpoint = FnEndpoint::new("flag", move |inv: &Invocation<'_>| {
        let mut set = flag.lock().unwrap();
        match inv.request.verb {
            Verb::Sink => {
                let only_if_unset = inv.inline_str("if-unset").ok() == Some("true");
                let changed = !*set;
                if only_if_unset && *set {
                    return Ok(text("unchanged"));
                }
                *set = true;
                Ok(text(if changed { "changed" } else { "unchanged" }))
            }
            _ => Ok(text(if *set { "set" } else { "unset" })),
        }
    });
    EndpointSpace::new().bind(Exact::new("urn:test:flag"), endpoint)
}

/// `urn:test:first`: "am I first?" Answers `won` or `lost`, and counts its wins.
/// `checked=false` decides from a read and then sets the flag (the defect); `checked=true`
/// asks the flag to compare-and-set and decides from that answer.
struct First {
    wins: Arc<AtomicU64>,
}

#[async_trait]
impl Endpoint for First {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let flag = || Iri::parse("urn:test:flag").expect("an IRI");
        let won = if inv.inline_str("checked").ok() == Some("true") {
            let answer = inv
                .issue(
                    Request::new(Verb::Sink, flag())
                        .with_arg("if-unset", ArgRef::Inline(b"true".to_vec())),
                )
                .await?;
            answer.bytes == b"changed"
        } else {
            let read = inv.issue(Request::new(Verb::Source, flag())).await?;
            if read.bytes == b"set" {
                false
            } else {
                inv.issue(Request::new(Verb::Sink, flag())).await?;
                true
            }
        };
        if won {
            self.wins.fetch_add(1, Ordering::SeqCst);
        }
        Ok(text(if won { "won" } else { "lost" }))
    }

    fn name(&self) -> &str {
        "first"
    }

    fn describe(&self) -> Description {
        Description::new("first").verb(Verb::Sink)
    }
}

struct Fixture {
    kernel: Arc<Kernel>,
    interleave: Interleave,
    wins: Arc<AtomicU64>,
}

fn fixture() -> Fixture {
    let wins = Arc::new(AtomicU64::new(0));
    let space = flag_space(Arc::new(Mutex::new(false))).bind(
        Exact::new("urn:test:first"),
        First {
            wins: Arc::clone(&wins),
        },
    );
    let interleave = Interleave::new();
    let kernel = Arc::new(Kernel::new(interleave.wrap(Arc::new(space))));
    interleave.attach(&kernel);
    Fixture {
        kernel,
        interleave,
        wins,
    }
}

fn issue(kernel: &Kernel, call: &Call) -> Result<String> {
    futures::executor::block_on(kernel.issue(call.request().clone(), call.capability()))
        .map(|r| String::from_utf8_lossy(&r.bytes).into_owned())
}

/// The trigger every interleaving test uses: the flag's WRITE, which in the unchecked path
/// comes after the read the decision was made from.
fn on_the_flags_write(inv: &Invocation<'_>) -> bool {
    inv.request.target.as_str() == "urn:test:flag" && inv.request.verb == Verb::Sink
}

#[test]
fn interleave_forces_the_window_a_read_then_write_leaves_open() {
    let f = fixture();
    let first = Call::new(Verb::Sink, "urn:test:first").arg("checked", "false");
    f.interleave.before(on_the_flags_write, first.clone());
    let answer = issue(&f.kernel, &first).unwrap();
    assert_eq!(f.interleave.armed(), 0, "the trigger matched");
    let interleaved = f.interleave.answers();
    assert_eq!(interleaved.len(), 1);
    let other = String::from_utf8_lossy(&interleaved[0].as_ref().unwrap().bytes).into_owned();
    // Both read `unset` before either wrote: two winners, every time.
    assert_eq!((answer.as_str(), other.as_str()), ("won", "won"));
    assert_eq!(f.wins.load(Ordering::SeqCst), 2);
}

#[test]
fn interleave_passes_a_compare_and_set() {
    let f = fixture();
    let first = Call::new(Verb::Sink, "urn:test:first").arg("checked", "true");
    f.interleave.before(on_the_flags_write, first.clone());
    let answer = issue(&f.kernel, &first).unwrap();
    assert_eq!(f.interleave.armed(), 0);
    let other = f.interleave.answers().remove(0).unwrap();
    // The interleaved call ran first and won; the outer one's compare-and-set saw it.
    assert_eq!(other.bytes, b"won");
    assert_eq!(answer, "lost");
    assert_eq!(f.wins.load(Ordering::SeqCst), 1);
}

#[test]
fn after_runs_once_the_matching_invocation_has_answered() {
    let f = fixture();
    let read = Call::new(Verb::Source, "urn:test:flag");
    f.interleave.after(on_the_flags_write, read);
    let answer = issue(
        &f.kernel,
        &Call::new(Verb::Sink, "urn:test:first").arg("checked", "false"),
    )
    .unwrap();
    assert_eq!(answer, "won");
    let seen = f.interleave.answers().remove(0).unwrap();
    assert_eq!(seen.bytes, b"set", "the call after the write saw it");
}

#[test]
fn a_trigger_that_never_matches_stays_armed() {
    let f = fixture();
    f.interleave.before(
        |inv: &Invocation<'_>| inv.request.target.as_str() == "urn:test:nowhere",
        Call::new(Verb::Source, "urn:test:flag"),
    );
    issue(
        &f.kernel,
        &Call::new(Verb::Sink, "urn:test:first").arg("checked", "true"),
    )
    .unwrap();
    assert_eq!(
        f.interleave.armed(),
        1,
        "a probe that tested nothing says so"
    );
    assert!(f.interleave.answers().is_empty());
}

#[test]
fn an_unattached_interleave_answers_an_error_rather_than_hanging() {
    let interleave = Interleave::new();
    let kernel = Kernel::new(interleave.wrap(Arc::new(flag_space(Arc::new(Mutex::new(false))))));
    interleave.before(
        |_inv: &Invocation<'_>| true,
        Call::new(Verb::Source, "urn:test:flag"),
    );
    issue(&kernel, &Call::new(Verb::Source, "urn:test:flag")).unwrap();
    let answers = interleave.answers();
    match &answers[..] {
        [Err(Error::Endpoint(message))] => assert!(message.contains("attach"), "{message}"),
        other => panic!("expected one Endpoint error naming attach, got {other:?}"),
    }
}

/// The wrapper changes nothing a walk can see: the same entries, and every endpoint
/// described as it describes itself, so the rest of this crate's suite runs unchanged
/// over a wrapped kernel.
#[test]
fn the_wrapped_space_is_described_as_the_unwrapped_one() {
    let wins = Arc::new(AtomicU64::new(0));
    let build = || -> Arc<dyn Space> {
        Arc::new(flag_space(Arc::new(Mutex::new(false))).bind(
            Exact::new("urn:test:first"),
            First {
                wins: Arc::clone(&wins),
            },
        ))
    };
    let plain = Kernel::new(build());
    let interleave = Interleave::new();
    let wrapped = Kernel::new(interleave.wrap(build()));
    let entries = |k: &Kernel| format!("{:?}", k.entries());
    assert_eq!(entries(&plain), entries(&wrapped));
    for pattern in ["urn:test:flag", "urn:test:first"] {
        assert_eq!(
            format!("{:?}", plain.describe_pattern(pattern)),
            format!("{:?}", wrapped.describe_pattern(pattern)),
            "{pattern}"
        );
    }
    let meta = |k: &Kernel| {
        futures::executor::block_on(k.issue(
            Request::new(Verb::Meta, Iri::parse("urn:test:first").unwrap()),
            &Capability::root(),
        ))
        .map(|r| r.bytes)
        .map_err(|e| e.to_string())
    };
    assert_eq!(meta(&plain), meta(&wrapped));
}

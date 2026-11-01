//! **Concurrency probes**: does a write still hold when it is not alone? (ledger #2772)
//!
//! The rest of this crate walks a kernel one request at a time, so it cannot see the
//! defect this module exists for: an endpoint that **reads, decides, and then writes with
//! a guard that does not re-check what it decided on**. Run once, such an endpoint is
//! correct. Run four times at once, all four read the same state, all four decide the same
//! thing, and all four write it: four "the claim is released" comments on one item
//! (ikigai-ledger's close, ledger #2764), a budget counter that loses increments (ledger
//! #2767). Each was found by reading the source for a decision made before the write and
//! then firing requests in parallel. This module is that second half, as a call a module's
//! own tests make, so the property a fix establishes stays established.
//!
//! Two probes, and an invariant the caller states:
//!
//! - [`Race`] releases several requests **at once**, behind a barrier, on real threads, for
//!   many rounds, and checks the invariant after each round: N copies of one request
//!   ([`Race::copies`]), or any set of different ones ([`Race::together`]). It reports how
//!   many rounds broke the invariant, because a race is a rate, not a yes or no.
//! - [`Interleave`] **forces** one ordering: it wraps a space so that another request runs
//!   immediately before (or after) the first invocation that matches a trigger. A window
//!   that unforced threads hit one round in a hundred is then hit every time, which makes it
//!   a regression test rather than a statistic.
//!
//! The invariant is the caller's, because only the module knows what "one" means: one
//! release comment, one status event, a counter equal to the number of calls that were told
//! they succeeded, a tuple taken at most once.
//!
//! ```
//! use ikigai_conformance::race::{Call, Race};
//! use ikigai_core::{Exact, EndpointSpace, FnEndpoint, Invocation, Kernel, Representation};
//! use ikigai_core::{ReprType, Verb};
//! use std::sync::{Arc, Mutex};
//!
//! // A counter whose increment is ONE critical section: the read and the write together.
//! let count = Arc::new(Mutex::new(0u64));
//! let shared = Arc::clone(&count);
//! let bump = FnEndpoint::new("bump", move |_inv: &Invocation<'_>| {
//!     let mut n = shared.lock().unwrap();
//!     *n += 1;
//!     Ok(Representation::new(ReprType::new("text/plain"), n.to_string().into_bytes()))
//! });
//! let kernel = Kernel::new(Arc::new(
//!     EndpointSpace::new().bind(Exact::new("urn:example:bump"), bump),
//! ));
//!
//! Race::over(&kernel)
//!     .rounds(20)
//!     .copies(
//!         8,
//!         |_kernel, _round| *count.lock().unwrap(),           // the count before the round
//!         |_before| Call::new(Verb::Sink, "urn:example:bump"),
//!         |_kernel, before, answers| {
//!             let succeeded = answers.iter().filter(|a| a.is_ok()).count() as u64;
//!             let after = *count.lock().unwrap();
//!             if after == before + succeeded {
//!                 Ok(())
//!             } else {
//!                 Err(format!("{succeeded} calls succeeded and the count moved {}", after - before))
//!             }
//!         },
//!     )
//!     .assert_holds();
//! ```
//!
//! # What a clean run proves, and what it does not
//!
//! A [`Race`] that holds over N rounds is evidence, not proof: the threads were released
//! together, and the scheduler still decides who reads first. Raise [`Race::rounds`] until
//! the defect you are guarding against fails reliably on the unfixed code (state the rate in
//! the test), and use [`Interleave`] for a window you can name. The probes see only what the
//! kernel answers and what the invariant reads back; they do not inspect locks.
//!
//! ⚠ **Each call is a separate top-level request on its own OS thread**, issued with
//! `futures::executor::block_on`, so an endpoint that blocks (a store write lock, a file
//! lock, a `std::sync::Mutex`) really is contended. That is the point. It also means the
//! kernel must be shareable across threads, which every `ikigai_core::Kernel` is.

use std::fmt;
use std::sync::{Arc, Barrier, Mutex, Weak};

use async_trait::async_trait;
use futures::executor::block_on;
use ikigai_core::{
    ArgRef, Body, Capability, Description, Endpoint, Error, Forward, Invocation, Iri, Kernel,
    Representation, Request, Resolution, Scope, Space, SpaceEntry, Topology, Verb,
};

/// What one call answered: the representation, or the error the kernel returned.
pub type Answer = Result<Representation, Error>;

/// One request and the capability it is issued under.
///
/// [`Call::new`] is issued under [`Capability::root`]; [`Call::under`] narrows it, which is
/// how a probe exercises a module's published grants rather than root's.
#[derive(Clone, Debug)]
pub struct Call {
    request: Request,
    capability: Capability,
}

impl Call {
    /// A request for `verb` on `iri`, under root.
    ///
    /// # Panics
    ///
    /// When `iri` does not parse: a probe is test code, and a typo should stop it.
    pub fn new(verb: Verb, iri: &str) -> Call {
        let iri = Iri::parse(iri).unwrap_or_else(|e| panic!("`{iri}` is not an IRI: {e}"));
        Call {
            request: Request::new(verb, iri),
            capability: Capability::root(),
        }
    }

    /// An existing request, under `capability`.
    pub fn from_request(request: Request, capability: Capability) -> Call {
        Call {
            request,
            capability,
        }
    }

    /// The same call with an inline argument.
    pub fn arg(mut self, name: &str, value: impl AsRef<[u8]>) -> Call {
        self.request = self
            .request
            .with_arg(name, ArgRef::Inline(value.as_ref().to_vec()));
        self
    }

    /// The same call under `capability` instead of root.
    pub fn under(mut self, capability: Capability) -> Call {
        self.capability = capability;
        self
    }

    /// The request this call issues.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// The capability this call is issued under.
    pub fn capability(&self) -> &Capability {
        &self.capability
    }

    fn issue(&self, kernel: &Kernel) -> Answer {
        block_on(kernel.issue(self.request.clone(), &self.capability))
    }
}

enum Kernels<'k> {
    Shared(&'k Kernel),
    Fresh(Box<dyn Fn() -> Kernel + Sync + 'k>),
}

/// Release several requests at once, many times, and hold each round to an invariant.
///
/// A round is: `setup` (on this thread, alone), then every call released together on its
/// own thread behind a barrier, then `invariant` over what each call answered (in the order
/// the calls were given) and whatever it reads back from the kernel. See the [module
/// documentation](self) for an example.
pub struct Race<'k> {
    kernels: Kernels<'k>,
    rounds: usize,
}

impl<'k> Race<'k> {
    /// Every round against one kernel. `setup` gives each round its own subject (a new item,
    /// a new key), so the rounds do not see each other's writes.
    pub fn over(kernel: &'k Kernel) -> Race<'k> {
        Race {
            kernels: Kernels::Shared(kernel),
            rounds: 50,
        }
    }

    /// Each round against a kernel `make` builds for it, so no state carries between rounds.
    pub fn fresh(make: impl Fn() -> Kernel + Sync + 'k) -> Race<'k> {
        Race {
            kernels: Kernels::Fresh(Box::new(make)),
            rounds: 50,
        }
    }

    /// How many rounds to run (50 unless set; at least 1).
    pub fn rounds(mut self, rounds: usize) -> Race<'k> {
        self.rounds = rounds.max(1);
        self
    }

    /// `n` copies of one call, released together, each round.
    ///
    /// `setup` runs first and returns the round's context (the subject the calls name);
    /// `call` builds the request from it; `invariant` says whether the round held.
    pub fn copies<T>(
        &self,
        n: usize,
        setup: impl Fn(&Kernel, usize) -> T,
        call: impl Fn(&T) -> Call,
        invariant: impl Fn(&Kernel, &T, &[Answer]) -> Result<(), String>,
    ) -> RaceReport {
        self.together(
            setup,
            |context| (0..n.max(1)).map(|_| call(context)).collect(),
            invariant,
        )
    }

    /// Any set of calls (two different actions on one subject, a reader beside a writer),
    /// released together, each round.
    pub fn together<T>(
        &self,
        setup: impl Fn(&Kernel, usize) -> T,
        calls: impl Fn(&T) -> Vec<Call>,
        invariant: impl Fn(&Kernel, &T, &[Answer]) -> Result<(), String>,
    ) -> RaceReport {
        let mut report = RaceReport {
            rounds: self.rounds,
            calls: 0,
            violations: Vec::new(),
        };
        for round in 0..self.rounds {
            let fresh;
            let kernel = match &self.kernels {
                Kernels::Shared(kernel) => *kernel,
                Kernels::Fresh(make) => {
                    fresh = make();
                    &fresh
                }
            };
            let context = setup(kernel, round);
            let calls = calls(&context);
            report.calls = report.calls.max(calls.len());
            let answers = release(kernel, &calls);
            if let Err(message) = invariant(kernel, &context, &answers) {
                report.violations.push(Violation { round, message });
            }
        }
        report
    }
}

/// Issue every call on its own thread, all released by one barrier, and collect the answers
/// in the order the calls were given. A panic on a call's thread is the caller's panic.
fn release(kernel: &Kernel, calls: &[Call]) -> Vec<Answer> {
    let barrier = Barrier::new(calls.len());
    std::thread::scope(|scope| {
        let handles: Vec<_> = calls
            .iter()
            .map(|call| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    call.issue(kernel)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|p| std::panic::resume_unwind(p))
            })
            .collect()
    })
}

/// One round that broke the invariant, and what the invariant said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The round, from 0.
    pub round: usize,
    /// The invariant's own message.
    pub message: String,
}

/// What a [`Race`] saw: how many rounds, and which of them broke the invariant.
#[derive(Clone, Debug)]
pub struct RaceReport {
    rounds: usize,
    calls: usize,
    violations: Vec<Violation>,
}

impl RaceReport {
    /// How many rounds ran.
    pub fn rounds(&self) -> usize {
        self.rounds
    }

    /// The rounds that broke the invariant, in order.
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// Whether every round held.
    pub fn holds(&self) -> bool {
        self.violations.is_empty()
    }

    /// Panic with the report unless every round held.
    #[track_caller]
    pub fn assert_holds(&self) {
        assert!(self.holds(), "{self}");
    }
}

impl fmt::Display for RaceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.violations.is_empty() {
            return write!(
                f,
                "the invariant held in all {} round(s) of {} concurrent call(s)",
                self.rounds, self.calls
            );
        }
        writeln!(
            f,
            "the invariant broke in {} of {} round(s) of {} concurrent call(s):",
            self.violations.len(),
            self.rounds,
            self.calls
        )?;
        for violation in self.violations.iter().take(5) {
            writeln!(f, "  round {}: {}", violation.round, violation.message)?;
        }
        if self.violations.len() > 5 {
            writeln!(f, "  and {} more", self.violations.len() - 5)?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------ interleave

/// Force one ordering of two requests: run a [`Call`] immediately before, or immediately
/// after, the first invocation that matches a trigger.
///
/// The window this aims at is the one between a decision and the write it guards. An
/// endpoint that reads through a sub-request and then writes through another is entered by
/// the kernel once per sub-request, so a trigger on the WRITE's sub-request (its target and
/// arguments, read from the [`Invocation`]) puts the other call exactly between the read
/// and the write, every time.
///
/// Wiring takes three steps, because the kernel the interleaved call is issued on is built
/// over the wrapped space:
///
/// ```
/// use ikigai_conformance::race::{Call, Interleave};
/// use ikigai_core::{EndpointSpace, Exact, FnEndpoint, Invocation, Kernel, Representation};
/// use ikigai_core::{ReprType, Space, Verb};
/// use std::sync::Arc;
///
/// let echo = FnEndpoint::new("echo", |_inv: &Invocation<'_>| {
///     Ok(Representation::new(ReprType::new("text/plain"), b"ok".to_vec()))
/// });
/// let inner: Arc<dyn Space> =
///     Arc::new(EndpointSpace::new().bind(Exact::new("urn:example:echo"), echo));
///
/// let interleave = Interleave::new();
/// let kernel = Arc::new(Kernel::new(interleave.wrap(inner)));   // 1. wrap
/// interleave.attach(&kernel);                                     // 2. attach
/// interleave.before(                                              // 3. arm
///     |inv: &Invocation<'_>| inv.request.verb == Verb::Sink,
///     Call::new(Verb::Source, "urn:example:echo"),
/// );
///
/// futures::executor::block_on(kernel.issue(
///     ikigai_core::Request::new(Verb::Sink, ikigai_core::Iri::parse("urn:example:echo").unwrap()),
///     &ikigai_core::Capability::root(),
/// ))
/// .unwrap();
/// assert_eq!(interleave.armed(), 0, "the trigger matched");
/// assert!(interleave.answers()[0].is_ok(), "and the interleaved call ran");
/// ```
///
/// Each arming fires **once**. [`armed`](Interleave::armed) says how many have not fired:
/// a trigger that never matched is a probe that tested nothing, so a test asserts it is 0.
///
/// ⚠ The interleaved call runs to completion on the matching invocation's task, before (or
/// after) that invocation, and outside it. It must not need anything the matching request
/// holds while it waits: a trigger on a sub-request is entered with no lock of the
/// sub-request's own endpoint held, but an endpoint that holds a lock of its own across its
/// sub-requests would deadlock against an interleaved call that takes the same lock.
#[derive(Clone, Default)]
pub struct Interleave {
    state: Arc<State>,
}

#[derive(Default)]
struct State {
    kernel: Mutex<Option<Weak<Kernel>>>,
    armed: Mutex<Vec<Armed>>,
    answers: Mutex<Vec<Answer>>,
}

type Trigger = Box<dyn Fn(&Invocation<'_>) -> bool + Send + Sync>;

struct Armed {
    after: bool,
    trigger: Trigger,
    call: Call,
}

impl Interleave {
    /// A new interleaving, armed with nothing.
    pub fn new() -> Interleave {
        Interleave::default()
    }

    /// `space`, with every endpoint it resolves wrapped so that an armed call can run beside
    /// it. Everything else passes through: the entries, the id and the topology of the space;
    /// the description, name, forwarding and confinement of each endpoint.
    pub fn wrap(&self, space: Arc<dyn Space>) -> Arc<dyn Space> {
        Arc::new(Wrapped {
            inner: space,
            state: Arc::clone(&self.state),
        })
    }

    /// The kernel an armed call is issued on: the one built over [`wrap`](Self::wrap)'s
    /// space. Held weakly, so the kernel's lifetime is still its owner's.
    pub fn attach(&self, kernel: &Arc<Kernel>) {
        *lock(&self.state.kernel) = Some(Arc::downgrade(kernel));
    }

    /// Run `call` immediately BEFORE the first invocation for which `trigger` holds.
    pub fn before(
        &self,
        trigger: impl Fn(&Invocation<'_>) -> bool + Send + Sync + 'static,
        call: Call,
    ) {
        self.arm(false, Box::new(trigger), call);
    }

    /// Run `call` immediately AFTER the first invocation for which `trigger` holds, whatever
    /// that invocation answered, and then hand its answer back.
    pub fn after(
        &self,
        trigger: impl Fn(&Invocation<'_>) -> bool + Send + Sync + 'static,
        call: Call,
    ) {
        self.arm(true, Box::new(trigger), call);
    }

    fn arm(&self, after: bool, trigger: Trigger, call: Call) {
        lock(&self.state.armed).push(Armed {
            after,
            trigger,
            call,
        });
    }

    /// How many armed calls have not fired yet.
    pub fn armed(&self) -> usize {
        lock(&self.state.armed).len()
    }

    /// The answers of the calls that fired, in firing order, taken (a second read is empty).
    pub fn answers(&self) -> Vec<Answer> {
        std::mem::take(&mut *lock(&self.state.answers))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // A panic inside a probe poisons the lock; what it guards is still whole.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl State {
    /// Take the first armed call on this side whose trigger matches, so it fires once.
    fn take(&self, after: bool, inv: &Invocation<'_>) -> Option<Call> {
        let mut armed = lock(&self.armed);
        let at = armed
            .iter()
            .position(|a| a.after == after && (a.trigger)(inv))?;
        Some(armed.remove(at).call)
    }

    async fn fire(&self, call: Call) {
        let kernel = lock(&self.kernel).as_ref().and_then(Weak::upgrade);
        let answer = match kernel {
            Some(kernel) => kernel.issue(call.request, &call.capability).await,
            None => Err(Error::Endpoint(
                "an interleaved call fired before `Interleave::attach` named the kernel to \
                 issue it on, or after that kernel was dropped"
                    .to_string(),
            )),
        };
        lock(&self.answers).push(answer);
    }
}

struct Wrapped {
    inner: Arc<dyn Space>,
    state: Arc<State>,
}

impl Space for Wrapped {
    fn resolve(&self, request: &Request, scope: &Scope) -> Resolution {
        let state = Arc::clone(&self.state);
        self.inner
            .resolve(request, scope)
            .map_endpoint(|endpoint| Arc::new(Beside { endpoint, state }) as Arc<dyn Endpoint>)
    }

    fn entries(&self) -> Option<Vec<SpaceEntry>> {
        self.inner.entries()
    }

    fn id(&self) -> Option<Iri> {
        self.inner.id()
    }

    fn topology(&self) -> Topology {
        self.inner.topology()
    }
}

/// An endpoint with an interleaving point on either side of it. ⚠ Every defaulted method of
/// `Endpoint` is passed through: a wrapper that forgets one (`forwards`, ledger #2537)
/// changes how the endpoint is described, and a probe must not change what it probes.
struct Beside {
    endpoint: Arc<dyn Endpoint>,
    state: Arc<State>,
}

#[async_trait]
impl Endpoint for Beside {
    async fn invoke(&self, inv: &Invocation<'_>) -> ikigai_core::Result<Representation> {
        if let Some(call) = self.state.take(false, inv) {
            self.state.fire(call).await;
        }
        let answer = self.endpoint.invoke(inv).await;
        if let Some(call) = self.state.take(true, inv) {
            self.state.fire(call).await;
        }
        answer
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> ikigai_core::Result<Representation<Body>> {
        if let Some(call) = self.state.take(false, inv) {
            self.state.fire(call).await;
        }
        let answer = self.endpoint.invoke_body(inv).await;
        if let Some(call) = self.state.take(true, inv) {
            self.state.fire(call).await;
        }
        answer
    }

    fn name(&self) -> &str {
        self.endpoint.name()
    }

    fn describe(&self) -> Description {
        self.endpoint.describe()
    }

    fn is_limiter(&self) -> bool {
        self.endpoint.is_limiter()
    }

    fn forwards(&self) -> Option<Forward> {
        self.endpoint.forwards()
    }

    fn confinement(&self) -> Option<Topology> {
        self.endpoint.confinement()
    }
}

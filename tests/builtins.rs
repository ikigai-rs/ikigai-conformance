//! The suite against `ikigai-core`'s own builtins (`toUpper`, `reverseList`,
//! `echo`) and, separately, the kernel's `urn:kernel:*` operations. These predate
//! the recipe, so the expected result is a specific set of findings — the
//! demonstration that each check sees a real endpoint, pinned so a change in
//! either core or this crate is a deliberate one.

use std::sync::Arc;

use ikigai_conformance::{check, Check, Suite};
use ikigai_core::builtins;
use ikigai_core::{EndpointSpace, Exact, Kernel, UriTemplate};

fn builtins_kernel() -> Kernel {
    let root = EndpointSpace::new()
        .bind(Exact::new("urn:example:toUpper"), builtins::to_upper())
        .bind(
            Exact::new("urn:example:reverseList"),
            builtins::reverse_list(),
        )
        .bind(
            UriTemplate::parse("urn:example:echo/{message}").unwrap(),
            builtins::echo(),
        );
    Kernel::new(Arc::new(root))
}

#[test]
fn the_builtins_report_exactly_the_findings_their_age_predicts() {
    let report = check(&builtins_kernel()).unwrap_err();
    assert_eq!(report.endpoints, 3, "{report}");
    assert_eq!(report.actions, 3, "{report}");

    // ARGSPECS: one untyped input each (`in`, `in`, `message`).
    let untyped: Vec<_> = report.of(Check::ArgSpecs).collect();
    assert_eq!(untyped.len(), 3, "{report}");
    assert!(untyped.iter().all(|f| f.detail.contains("has no class")));

    // NAMES: the two lowerCamelCase ids the crate docs name as the known exception.
    let names: Vec<_> = report
        .of(Check::Names)
        .map(|f| f.endpoint.as_str())
        .collect();
    assert_eq!(names, ["toUpper", "reverseList"], "{report}");

    // CACHEABLE: all three are pure functions nobody declared pure, so each
    // cacheable result has an empty thread set. The cache itself works (no
    // "recomputed" finding).
    let cache: Vec<_> = report.of(Check::Cacheable).collect();
    assert_eq!(cache.len(), 3, "{report}");
    assert!(cache
        .iter()
        .all(|f| f.detail.contains("empty golden-thread set")));

    // Nothing else: the builtins declare no capability and enforce none, serve
    // the `text/plain;charset=utf-8` they declare (parameters stripped, the same
    // face), serve no RDF face, and take their input by name.
    for check in [
        Check::RequiresVerb,
        Check::Enforced,
        Check::Outputs,
        Check::SkolemRdf,
        Check::Vocabulary,
        Check::Pipeline,
    ] {
        assert!(report.of(check).next().is_none(), "{check}:\n{report}");
    }
    assert_eq!(report.findings.len(), 8, "{report}");
}

#[test]
fn the_builtins_are_clean_once_declared_pure_and_typed_names_are_waived() {
    let report = Suite::new()
        .checks(ikigai_conformance::Checks::all() - Check::Names - Check::ArgSpecs)
        .pure("toUpper")
        .pure("reverseList")
        .pure("echo")
        .run_blocking(&builtins_kernel());
    assert!(report.is_clean(), "{report}");
}

#[test]
fn the_kernel_operations_reported_for_the_record() {
    // `urn:kernel:*` is core's; a module's report skips it. Included here so the
    // count is visible and pinned: these are findings against core, to be fixed
    // there (or waived deliberately), not noise every module carries.
    let report = Suite::new()
        .include_kernel_ops()
        .checks(ikigai_conformance::Checks::all() - Check::Names)
        .pure("toUpper")
        .pure("reverseList")
        .pure("echo")
        .run_blocking(&builtins_kernel());
    let kernel_findings: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.endpoint.starts_with("kernel-"))
        .collect();
    assert!(!kernel_findings.is_empty(), "{report}");
    // Every kernel op's inputs are untyped — the same ARGSPECS finding the
    // builtins carry, one per input.
    assert!(
        kernel_findings
            .iter()
            .filter(|f| f.check == Check::ArgSpecs)
            .all(|f| f.detail.contains("has no class")),
        "{report}"
    );
    // No kernel op enforces an undeclared scope, and every declared one denies.
    assert!(
        kernel_findings.iter().all(|f| f.check != Check::Enforced),
        "{report}"
    );
    // The intrinsic `urn:kernel:*` path caches but records no trace event, so the
    // cache-hit witness for a kernel op is `Kernel::is_cached`, not the trace —
    // a kernel op must never read as "recomputed" because it is merely untraced.
    assert!(
        kernel_findings
            .iter()
            .all(|f| !f.detail.contains("recomputed")),
        "{report}"
    );
    // What the cache probe DOES see on the manifold: cached `Never` with no thread —
    // a rebind leaves it stale forever (ikigai-core audit item 7).
    assert!(
        kernel_findings
            .iter()
            .any(|f| f.endpoint == "kernel-actions"
                && f.check == Check::Cacheable
                && f.detail.contains("empty golden-thread set")),
        "{report}"
    );
    // Every kernel op serves what it declares. Two cannot be resolved with minimal
    // inputs on a bare kernel (`kernel-catalog` needs a Meta renderer,
    // `kernel-validate` a proposal), and OUTPUTS lists them as unprobed rather than
    // reporting a face it never saw.
    assert!(
        kernel_findings.iter().all(|f| f.check != Check::Outputs),
        "{report}"
    );
    let unprobed: Vec<&str> = report
        .unprobed
        .iter()
        .filter(|u| u.check == Check::Outputs)
        .map(|u| u.endpoint.as_str())
        .collect();
    assert_eq!(unprobed, ["kernel-catalog", "kernel-validate"], "{report}");
    eprintln!("{report}");
}

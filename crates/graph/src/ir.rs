//! Stage 5 — Runtime IR + the transformer seam.
//!
//! Type erasure (when wired by RT-003) preserves source positions: annotations are
//! blanked in place, no downlevel emit, no sourcemap layer (CANON §9).

use std::sync::Arc;

use oxc_span::Span;

use crate::cst::Cst;
use crate::error::StripDiagnostic;
use crate::semantic::SemanticGraph;
use crate::strip::StripPolicy;

/// Type-erased, V8-ready output.
#[derive(Debug, Clone)]
pub struct RuntimeIr {
    pub code: Arc<str>,
    pub positions_preserved: bool,
}

/// Internal result of the stage-5 query: either the IR or the diagnostics that
/// explain why this file cannot be lowered.
pub(crate) struct IrOutcome {
    pub(crate) ir: Option<RuntimeIr>,
    pub(crate) diagnostics: Vec<StripDiagnostic>,
}

/// Stage-5 query body: lower a file to runtime IR — honestly.
///
/// Lowering only succeeds when the input is something we can actually hand to V8:
/// 1. a file with recovered parse/semantic errors is never lowered (broken input
///    must not masquerade as runnable code);
/// 2. the installed [`StripPolicy`] must accept it;
/// 3. until RT-003 wires the real type-strip, only clean JavaScript lowers — a
///    TypeScript input is rejected rather than returned verbatim as "runnable".
pub(crate) fn compute_runtime_ir(
    policy: &dyn StripPolicy,
    cst: &Cst,
    sem: &SemanticGraph,
    source: Arc<str>,
) -> IrOutcome {
    // 1. Never fabricate IR from broken input.
    let parse_errors = cst.errors().len();
    let semantic_errors = sem.errors().len();
    if parse_errors > 0 || semantic_errors > 0 {
        return IrOutcome {
            ir: None,
            diagnostics: vec![StripDiagnostic {
                span: Span::default(),
                message: format!(
                    "cannot lower a file with {parse_errors} parse + {semantic_errors} semantic error(s)"
                ),
                help: "fix the reported parse/semantic errors before lowering to runtime IR"
                    .to_string(),
            }],
        };
    }

    // 2. TypeScript erasability is INTRINSIC to stripping: regardless of the
    //    installed policy, a TS input that contains non-erasable syntax (enum,
    //    runtime namespace, parameter property, `import =`, `export =`) can never be
    //    whitespace-blanked to correct JS — so it is always rejected here. A
    //    permissive policy can relax *additional* rules, never this one.
    if cst.source_type().is_typescript() {
        if let Err(diagnostics) = crate::strip::ErasablePolicy.check(sem) {
            return IrOutcome {
                ir: None,
                diagnostics,
            };
        }
    }

    // 3. Consult the installed strip policy for any further rules.
    if let Err(diagnostics) = policy.check(sem) {
        return IrOutcome {
            ir: None,
            diagnostics,
        };
    }

    // === RT-003 ===
    // Erasable-only type strip (CANON §9, I-3). The policy gate above has already
    // rejected every non-erasable construct, so what remains is erasable TS (or
    // plain JS). Plain JS needs no stripping, so its verbatim source IS the runtime
    // IR (identity — no allocation). Erasable TS is stripped to whitespace in place:
    // annotations erased, positions byte-for-byte preserved, no downlevel emit and
    // no sourcemap.
    let code: Arc<str> = if cst.source_type().is_typescript() {
        Arc::from(crate::strip::strip(&source, cst))
    } else {
        source
    };
    IrOutcome {
        ir: Some(RuntimeIr {
            code,
            positions_preserved: true,
        }),
        diagnostics: Vec::new(),
    }
    // === /RT-003 ===
}

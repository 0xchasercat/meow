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

    // 2. Consult the installed strip policy.
    if let Err(diagnostics) = policy.check(sem) {
        return IrOutcome {
            ir: None,
            diagnostics,
        };
    }

    // === RT-003 ===
    // Seam for RT-003's actual `oxc_transformer` / `oxc_codegen` type-strip. Until
    // it lands, the only honest lowering is the IDENTITY passthrough for clean
    // JavaScript (JS needs no stripping, so verbatim source IS the runtime IR).
    // TypeScript has no strip yet, so it cannot be lowered — returning the verbatim
    // TS as `positions_preserved: true` would be a lie (CONSTITUTION I-3 / I-11:
    // prose must match code).
    if cst.source_type().is_typescript() {
        return IrOutcome {
            ir: None,
            diagnostics: vec![StripDiagnostic {
                span: Span::default(),
                message: "type stripping is not yet available (lands in RT-003)".to_string(),
                help: "TypeScript cannot be lowered to runtime IR until the RT-003 strip lands; \
                       only clean JavaScript can be lowered today"
                    .to_string(),
            }],
        };
    }

    // Clean JavaScript, no errors, policy satisfied: identity passthrough.
    IrOutcome {
        ir: Some(RuntimeIr {
            code: source,
            positions_preserved: true,
        }),
        diagnostics: Vec::new(),
    }
    // === /RT-003 ===
}

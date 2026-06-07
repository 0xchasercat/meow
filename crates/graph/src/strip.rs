//! Stage-5 strip policy seam.
//!
//! The erasable-only type-strip *policy* (which TS constructs are banned + the exact
//! fix messages) is owned by RT-003 (I-3). This file is RT-003's edit region: it
//! supplies only the trait and a permissive default seam.

use crate::error::StripDiagnostic;
use crate::semantic::SemanticGraph;

/// Gate consulted before lowering to runtime IR. RT-003 installs the real
/// erasable-only implementation (rejecting enum / namespace / param-props /
/// `import =`); a rejection names the construct and the fix.
pub trait StripPolicy: Send + Sync {
    fn check(&self, sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>>;
}

// === RT-003 ===
/// Default seam: accepts everything. This is an explicit placeholder, NOT a shipped
/// erasable-only guarantee — RT-003 replaces it with the real policy.
#[derive(Default)]
pub struct PermissivePolicy;

impl StripPolicy for PermissivePolicy {
    fn check(&self, _sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>> {
        Ok(())
    }
}
// === /RT-003 ===

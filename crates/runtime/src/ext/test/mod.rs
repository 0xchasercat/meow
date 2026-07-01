//! `meow:test` op layer (TEST-001).
//!
//! Provides a single op for the JS test runner to store JSON results,
//! which Rust reads back after script evaluation.

use deno_core::{op2, OpState};

/// Stored test results as JSON. Set by JS runner, read by Rust after eval.
#[derive(Clone, Default)]
pub struct TestResults(pub std::rc::Rc<std::cell::RefCell<Option<String>>>);

#[op2(fast)]
pub fn op_test_store_results(state: &mut OpState, #[string] json: &str) {
    if let Some(results) = state.try_borrow_mut::<TestResults>() {
        *results.0.borrow_mut() = Some(json.to_owned());
    }
}

deno_core::extension!(
    meow_test,
    ops = [op_test_store_results],
    state = |state: &mut OpState| {
        state.put::<TestResults>(TestResults::default());
    },
);

pub fn test_extension() -> deno_core::Extension {
    meow_test::init()
}

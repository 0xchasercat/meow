//! `meow:ui` op layer (UI-001).

use deno_core::OpState;
use meow_ui::Ui;

use super::write_output;

fn ui_for_output(state: &OpState) -> Ui {
    if state.try_borrow::<super::PrintSink>().is_some() {
        Ui::captured()
    } else {
        Ui::auto()
    }
}

fn emit_ui_line(state: &mut OpState, body: &str, is_err: bool) -> Result<(), std::io::Error> {
    let ui = ui_for_output(state);
    let mut line = if is_err {
        ui.hiss_line(body)
    } else {
        ui.purr_line(body)
    };
    line.push('\n');
    write_output(state, &line, is_err)
}

#[deno_core::op2(fast)]
pub fn op_ui_purr(state: &mut OpState, #[string] body: &str) -> Result<(), std::io::Error> {
    emit_ui_line(state, body, false)
}

#[deno_core::op2(fast)]
pub fn op_ui_hiss(state: &mut OpState, #[string] body: &str) -> Result<(), std::io::Error> {
    emit_ui_line(state, body, true)
}

#[deno_core::op2(fast)]
pub fn op_ui_pounce(state: &mut OpState, #[string] body: &str) -> Result<(), std::io::Error> {
    let ui = ui_for_output(state);
    let mut line = ui.pounce_line(body);
    line.push('\n');
    write_output(state, &line, false)
}

deno_core::extension!(
    meow_ui_runtime,
    ops = [op_ui_purr, op_ui_hiss, op_ui_pounce],
);

pub fn ui_extension() -> deno_core::Extension {
    meow_ui_runtime::init()
}

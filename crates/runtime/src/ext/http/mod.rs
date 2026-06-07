//! `meow:http` op layer (RT-005).

pub mod ops;

use std::rc::Rc;

use deno_core::OpState;

use crate::io::{AllowAll, CapabilityCheck};

deno_core::extension!(
    meow_http,
    ops = [
        ops::op_http_serve,
        ops::op_http_next,
        ops::op_http_respond,
        ops::op_http_shutdown,
    ],
    state = |state: &mut OpState| {
        if state.try_borrow::<Rc<dyn CapabilityCheck>>().is_none() {
            state.put::<Rc<dyn CapabilityCheck>>(Rc::new(AllowAll));
        }
    },
);

pub fn http_extension() -> deno_core::Extension {
    meow_http::init()
}

/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) mod extraction;
pub(crate) mod fields;
pub(crate) mod logic_transforms;
pub(crate) mod method_transforms;
pub(crate) mod move_ops;
pub(crate) mod oop;
pub(crate) mod parameters;
pub(crate) mod schema_codemod;

pub(crate) use extraction::*;
pub(crate) use fields::*;
pub(crate) use logic_transforms::*;
pub(crate) use method_transforms::*;
pub(crate) use move_ops::*;
pub(crate) use oop::*;
pub(crate) use parameters::*;
pub(crate) use schema_codemod::*;

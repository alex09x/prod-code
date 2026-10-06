/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[derive(Debug, Clone)]
pub struct PolyglotDecl {
    pub fn_name: String,
    pub open_paren: usize,
    pub close_paren: usize,
    pub body_open: usize,
    pub body_close: usize,
    pub receiver: Option<String>,
    pub params: Vec<crate::parameter_object::Param>,
    pub ret_span: Option<(usize, usize)>,
    pub is_async: bool,
    pub async_keyword_span: Option<(usize, usize)>,
    pub visibility_span: Option<(usize, usize)>,
}

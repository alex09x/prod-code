/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) fn inner_bracket_type<'a>(ty: &'a str, wrapper: &str) -> Option<&'a str> {
    let t = ty.trim();
    if !t.starts_with(wrapper) {
        return None;
    }
    let open = t.find('<')?;
    let close = t.rfind('>')?;
    Some(t[open + 1..close].trim())
}

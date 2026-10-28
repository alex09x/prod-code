/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod builtin_macros;
pub mod classify;
pub mod hazards;

pub use builtin_macros::mark_builtin_stringify_args;
pub use classify::*;
pub use hazards::*;

pub async fn effect_hazards_with_builtin_stringify(
    remote: std::net::SocketAddr,
    root: &std::path::Path,
    name: &str,
    declared: &[crate::signature::types::Declared],
    facts: &[crate::signature::types::ParamFacts],
    has_receiver: bool,
    args: &[Option<usize>],
    calls: &mut [crate::signature::types::CallSite],
) -> Vec<String> {
    mark_builtin_stringify_args(remote, root, calls, args, declared.len(), has_receiver).await;
    hazards::effect_hazards(name, declared, facts, has_receiver, args, calls)
}

#[allow(clippy::too_many_arguments)]
pub async fn ensure_reorder_is_safe(
    remote: std::net::SocketAddr,
    root: &std::path::Path,
    file: &std::path::Path,
    source: &str,
    open: usize,
    close: usize,
    name: &str,
    declared: &[crate::signature::types::Declared],
    has_receiver: bool,
    args: &[Option<usize>],
    references: &[crate::signature::types::Reference],
) -> anyhow::Result<()> {
    let mut calls = crate::signature::call_sites::call_sites(root, references)?;
    let facts = hazards::param_facts(remote, root, file, source, open, close, declared).await;
    let hazards = effect_hazards_with_builtin_stringify(
        remote,
        root,
        name,
        declared,
        &facts,
        has_receiver,
        args,
        &mut calls,
    )
    .await;
    anyhow::ensure!(
        hazards.is_empty(),
        "the new parameter list would change what the program does, or it cannot be shown \
         that it does not; nothing was written, and `force` does not override this:\n  {}\n\
         bind such an argument to a local of the parameter's own type before the call \
         (`let v: T = …;`) and pass the local, keep owned parameters and references in their \
         order, or remove them in a step of their own",
        hazards.join("\n  ")
    );
    Ok(())
}

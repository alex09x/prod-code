# Rust toolchains on a build node

A build node runs a project's commands with the toolchain its `rust-toolchain.toml` names.
rustup installs what is missing the first time a command needs it. This page covers the one
way that goes wrong and stays wrong until someone repairs the node.

## `detected conflict` while installing a component

`code_test`, `code_check`, `code_lint` or `prod-code exec -- cargo …` ends before building:

```
info: downloading component llvm-tools
info: rolling back changes
error: failed to install component: 'llvm-tools-preview-x86_64-unknown-linux-gnu', detected conflict: 'lib/rustlib/x86_64-unknown-linux-gnu/lib/libLLVM-22-rust-1.98.1-stable.so'
```

The same project works on another node, and keeps failing on this one.

### What it means

The message reads as if two components of the project could not be installed together, for
example `rustc-dev` and `llvm-tools-preview`. They can. rustup stops because the toolchain
directory already holds a file the component is about to install, and no installed component
owns that file: rustup has no record of where it came from, so it will not overwrite it.

Such a file is left behind when rustup loses track of an install. Two installs into the same
toolchain at the same moment do that. Starting from a toolchain with neither component:

```
$ rustup component add --toolchain 1.98.1 llvm-tools-preview &
$ rustup component add --toolchain 1.98.1 rustc-dev &
$ wait        # both exit 0
```

Both sets of files are now in the toolchain, but `lib/rustlib/multirust-config.toml` lists
only the component whose process wrote it last. rustup takes the other one as not installed,
and the next command that asks for it fails with `detected conflict` on its first file. An
install that is killed half way leaves the same kind of state.

On a node this happens when a toolchain is new there and several commands that need it, with
different component lists, start together.

### Check

`scripts/rustup-unowned-files.sh` lists, for every toolchain on the machine, the files no
installed component owns. It only reads. Run it on the node:

```
$ ssh <node> 'bash -s' < scripts/rustup-unowned-files.sh
1.97.1-x86_64-unknown-linux-gnu: 0 file(s) no installed component owns
1.98.1-x86_64-unknown-linux-gnu: 2 file(s) no installed component owns
    lib/rustlib/x86_64-unknown-linux-gnu/lib/libLLVM-22-rust-1.98.1-stable.so
    lib/rustlib/x86_64-unknown-linux-gnu/lib/libLLVM.so.22.1-rust-1.98.1-stable
```

The file the error names is in the list: that confirms the cause. Files a tool wrote into a
toolchain on its own, such as a `Cargo.lock` under `lib/rustlib/src` or a `.DS_Store`, are
listed too and do no harm, because no component ships a file of that name.

A file that an installed component does own is a different problem: the project asks for two
components that ship the same file, and no repair on the node changes that.

### Repair

On the node, for the toolchain and the component the error names:

1. Move the listed files that belong to the failing component out of the toolchain
   directory. Moving keeps them until the install has succeeded.
2. Install the component: `rustup component add --toolchain 1.98.1 llvm-tools-preview`.
3. Run the script again: the toolchain reports `0 file(s)`, and the component is in
   `lib/rustlib/components`. Delete the files moved in step 1.

If the install in step 2 names another file, that file is a leftover of the same lost install:
move it out as well and repeat.

Removing and installing the whole toolchain (`rustup toolchain uninstall`, then
`rustup toolchain install` with the components the projects need) is the blunt form of the
same repair, and takes every other component of that toolchain with it for a few minutes.

### Avoid it

Install a toolchain a node has not seen yet once, with every component the projects on that
node ask for, before commands that need it run side by side:

```
$ rustup toolchain install 1.98.1 --component rustc-dev --component llvm-tools-preview --component rust-src
```

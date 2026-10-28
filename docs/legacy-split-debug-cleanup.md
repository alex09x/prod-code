# One-time cleanup of legacy split-debug files

The dev/test profile change in v0.3.18 stopped producing `.dwo` files. It does not remove files left by an older profile with `debug = 1`, `split-debuginfo = "unpacked"`, and incremental compilation. Clean those files from the owning checkout's target directory after the new profile is in use.

## Safety and scope

- Run this on the node that owns the checkout and its build cache. Confirm no Cargo/rustc build, agent task, or other process is using that checkout's target directory. Do not run this while a build is active.
- Set `target_dir` to that checkout's `target` directory, or to its configured `CARGO_TARGET_DIR`. Do not point it at a shared Cargo home or a parent containing multiple workspaces.
- The procedure touches only regular `.dwo` files below `debug/deps` and `debug/incremental`. It leaves other build outputs intact. Do not substitute `cargo clean`.

## Inventory first

From the checkout root, resolve the target directory and list only the legacy files:

```sh
target_dir="${CARGO_TARGET_DIR:-$PWD/target}"
printf 'Target directory: %s\n' "$target_dir"

for directory in "$target_dir/debug/deps" "$target_dir/debug/incremental"; do
    if [ -d "$directory" ]; then
        find "$directory" -type f -name '*.dwo' -print
    fi
done
```

Review the printed paths and confirm that they belong to this checkout. To count them before deletion:

```sh
for directory in "$target_dir/debug/deps" "$target_dir/debug/incremental"; do
    if [ -d "$directory" ]; then
        find "$directory" -type f -name '*.dwo' -print
    fi
done | wc -l
```

## Delete and verify

After the inventory is reviewed and the target is still idle, run:

```sh
for directory in "$target_dir/debug/deps" "$target_dir/debug/incremental"; do
    if [ -d "$directory" ]; then
        find "$directory" -type f -name '*.dwo' -print -delete
    fi
done
```

The command prints each file as it removes it. Repeat the inventory loop above; it should print no paths. This is safe to repeat. If any new `.dwo` appears, stop and check which build is still using the old profile before running the cleanup again.

## Reproduce the old-profile case

`docs/fixtures/legacy-dwo` is a small Cargo project using the old profile. On a Linux build node, create an isolated cache and build it:

```sh
CARGO_TARGET_DIR=/tmp/prod-code-legacy-dwo-fixture \
    cargo build --manifest-path docs/fixtures/legacy-dwo/Cargo.toml
```

Set `target_dir=/tmp/prod-code-legacy-dwo-fixture` and run the inventory/deletion/verification steps above. The fixture exercises both locations without touching the repository's normal `target/` cache. Remove the temporary target directory after verification.

Validation performed on a Linux build node: the fixture produced 16 `.dwo` files (8 under `debug/deps`, 8 under `debug/incremental`); the scoped deletion removed all 16, and a repeat inventory returned no files. A separate earlier read-only check of the current checkout found no `.dwo` files, so this fixture validates the procedure without claiming that stale artifacts remained in production caches.

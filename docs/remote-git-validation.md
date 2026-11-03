# Git metadata in remote commands

The remote workspace is a synchronized file mirror. It does not include the local
checkout's `.git` directory, Git index or commit history. Commands such as
`git shortlog -sne HEAD`, `git log` and `git status` may therefore fail remotely
even though they succeed in the original checkout.

Run read-only Git history or index validation locally in that original checkout.
Continue running builds, linters and tests on the gateway. `prod-code exec` does
not copy private Git configuration, credentials or repository history to make a
Git command succeed, and does not silently rerun a command on the workstation.

For direct Git history/index commands, the gateway prints this guidance when the
mirrored working directory has no `.git` marker. The requested command still runs
remotely with its original arguments, output and exit status. The notice does not
parse arbitrary shell scripts or wrappers. Version checks, `git diff --no-index`
and commands that explicitly select a repository with `-C`, `--git-dir`,
`--work-tree` or request environment variables are left alone.

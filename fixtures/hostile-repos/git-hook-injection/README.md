# Hostile Repository Fixture: Git Hook and Config Injections

Threat vectors where a repository's own metadata makes host-side git execute a
command of the repository's choosing.

There are no checked-in fixture files here, and there cannot be: a hostile
`.git` directory is not something one repository can carry inside another. The
vectors are built at test time instead, by
`crates/git-workspaces/tests/host_git_hardening.rs`.

## Test Vectors

Confirmed to execute under plain `git status` on git 2.55. The test asserts each
one fires before asserting the hardening blocks it, so a git build that lost the
vector cannot make the test pass by accident.

1. **`core.fsmonitor`**: any value is run through a shell whenever the index is
   refreshed.
2. **`post-index-change` hook**: run whenever status writes a refreshed index
   back. No committed file is involved; `.git/hooks/` is enough.
3. **`filter.<driver>.clean`**: run when an entry is stat dirty and git has to
   re-hash the working tree file. A file whose mtime moved while its size did
   not is sufficient, which is what a build that rewrites files produces.

## Enforcement, as implemented

Host-side git is built only by `vitna_git_workspaces::host_git::command`, which
applies:

- `-c core.fsmonitor=false`
- `-c core.hooksPath=<empty directory owned by this process>`, re-checked empty
  on every call
- `-c filter.<driver>.{clean,smudge,process}=` and `required=false` for every
  filter driver declared in `local` or `worktree` scope
- `--no-optional-locks`, which also keeps status from reaching a hook at all
- `--no-pager`, `-c core.pager=cat`, `-c diff.external=`
- `GIT_CONFIG_NOSYSTEM=1`, and `GIT_EXTERNAL_DIFF` / `GIT_PAGER` cleared

`crates/git-workspaces/tests/no_bare_git_invocations.rs` fails if any crate
builds a `Command::new("git")` of its own, which is the shape the original two
callers had.

## Corrections to earlier versions of this file

- It claimed `vitna-git-broker` overrode `core.hooksPath=/dev/null` for all git
  operations. Nothing did. `vitna-git-broker` does not invoke git at all; it
  works on file contents in memory. The two callers that did invoke git,
  `tools/src/git_status.rs` and `git-workspaces/src/workspace.rs`, passed no
  overrides whatsoever.
- It claimed aliases were neutralized. Aliases cannot override a builtin
  subcommand, so `alias.status` never runs for `git status`, verified against
  git 2.55. Every host-side invocation names a builtin. There is nothing to
  neutralize, and claiming otherwise implied a control that did not exist.
- It claimed unsafe protocols (`ext::`, `file://`) were denied. **They are
  not.** No clone, fetch, or submodule operation exists in the codebase yet, so
  there is currently nothing to deny. A protocol allowlist has to land with the
  first code that fetches.

## Known gaps

- Filter drivers declared inside a submodule's own gitdir
  (`.git/modules/<name>/config`) are not enumerated. `git_status` passes
  `--ignore-submodules=dirty` so status does not scan submodule working trees,
  which is what would reach them.
- `diff.<driver>.textconv` is selected by attributes rather than declared as a
  driver list, so it is not enumerable the way filter drivers are. No
  diff-producing subcommand is invoked today; one that is added must pass
  `--no-ext-diff --no-textconv`.
- `GIT_CONFIG_NOSYSTEM=1` also hides a system-scope git-lfs install, so an LFS
  working tree may report spurious modifications through these invocations.

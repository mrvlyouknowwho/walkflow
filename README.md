# walkflow

**Step through your GitHub Actions workflow locally — before you push.**

You know the loop: edit a `.yml`, commit, push, wait for the runner, watch it fail on step 6, tweak, push again. `walkflow` breaks that loop. It runs your workflow's steps **on your machine, one at a time**, and pauses between them so you can look around, fix things, and continue — no commit, no push, no waiting.

```
┌─ step 4/7: run migrations
│    $ ./scripts/migrate.sh
│  [enter] run · [s]hell · [k] skip · [q] quit >
```

Hit `s` and you're dropped into a shell **in the workspace, with every environment variable the previous steps exported** — the exact state step 4 would see. Poke around. `exit`. Run the step. If it fails:

```
│  step failed. [r]etry · [s]hell · [c]ontinue · [q]uit >
```

Retry lets you edit the command in `$EDITOR` and run it again immediately. The inner loop that used to be "push and pray" is now seconds long.

## A session

```console
$ walkflow
walkflow — .github/workflows/ci.yml · job 'build'

▶ job build — 6 step(s)

┌─ step 1/6: checkout
│  uses: actions/checkout@v4 — not executable in host mode, skipping.

┌─ step 2/6: install deps
│    $ npm ci
│  [enter] run · [s]hell · [k] skip · [q] quit >           # ⏎
│  running in /home/me/app
└─ ok

┌─ step 3/6: run tests
│    $ npm test
│  [enter] run · [s]hell · [k] skip · [q] quit >           # ⏎
│  running in /home/me/app
FAIL src/auth.test.ts  ✗ token refresh
│  step failed. [r]etry · [s]hell · [c]ontinue · [q]uit >  # s
│  entering shell (/bin/zsh); `exit` to return to walkflow
$ echo $NODE_ENV        # inspect the exact env step 3 saw
test
$ vim src/auth.ts       # fix it right here
$ exit
│  back in walkflow
│  step failed. [r]etry · [s]hell · [c]ontinue · [q]uit >  # r
└─ ok
```

No commit. No push. No 4-minute runner wait to find out.

## Why not just use `act`?

[`act`](https://github.com/nektos/act) is great for *replaying* a whole workflow in Docker. But it runs top-to-bottom and stops — there's no pausing between steps, no dropping into the live state, no edit-and-continue. That's [a years-open feature request](https://github.com/nektos/act/issues/1050). `walkflow` is built around exactly that: the interactive step-through inner loop, not the full replay.

|                              | `act` | `walkflow` |
|------------------------------|:-----:|:----------:|
| Run workflow locally         | ✅    | ✅         |
| Pause **between** steps       | ❌    | ✅         |
| Drop into a shell with live step state | ❌ | ✅  |
| Edit a failed step and retry in place | ❌ | ✅   |
| Faithful `$GITHUB_ENV` / `$GITHUB_PATH` threading | ✅ | ✅ |

## Install

```bash
cargo install --git https://github.com/mrvlyouknowwho/walkflow
```

Prebuilt binaries: see [Releases](https://github.com/mrvlyouknowwho/walkflow/releases).

## Use

From your repo root:

```bash
walkflow                       # finds .github/workflows/<the one file>
walkflow ci.yml --job build    # pick a file and job
walkflow --list                # show jobs and steps, then exit
walkflow --from 4               # auto-run steps 1–3, go interactive from step 4
walkflow --from "run tests"     # ...or pick the step by name/substring
walkflow -y                     # run everything, no pausing (great for a fast local sanity check)
```

Keys while walking:

| key       | when        | does |
|-----------|-------------|------|
| `enter`   | before a step | run it |
| `s`       | anytime     | shell in the workspace with the current accumulated env |
| `k`       | before a step | skip it |
| `r`       | after a failure | retry (offers `$EDITOR` first) |
| `c`       | after a failure | continue to the next step anyway |
| `q`       | anytime     | quit |

## What it faithfully reproduces

- Environment layering: workflow `env:` → job `env:` → step `env:`.
- **`$GITHUB_ENV`** exports (both `KEY=value` and `KEY<<EOF` heredoc form) threaded into later steps.
- **`$GITHUB_PATH`** additions prepended to `PATH` for later steps.
- The default step shell (`bash --noprofile --norc -eo pipefail`), or your `shell:` override.
- `$GITHUB_WORKSPACE`, `$CI`, `$GITHUB_ACTIONS`, per-step `working-directory`.

## What it doesn't (yet)

- **`uses:` steps** (marketplace actions like `actions/checkout`, `actions/setup-node`) are skipped with a note — most are no-ops on your local checkout. Running them with full Docker environment parity is the headline **Pro** feature on the roadmap.
- **`${{ expressions }}`** are not evaluated — steps run with the literal env. Condition (`if:`) and matrix expansion are shown, not enforced.

These are honest limitations, not silent ones — `walkflow` tells you when it hits one.

## Roadmap

- **Docker runner** — run steps in the real `ubuntu-latest` image for full parity (Pro).
- `uses:` execution via a bundled action executor.
- Expression + `if:` evaluation.
- `--only <step>` to run a single step in isolation.

## License

MIT © walkflow contributors

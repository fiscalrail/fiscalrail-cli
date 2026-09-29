# FiscalRail CLI

`fiscalrail` is a Rust client for the current FiscalRail public API. It covers all
43 operations in `config/fiscal_rail/api.oas.yml` as of 2026-09-28. Commands for
proposed API endpoints in `docs/interface-design.md` will be added when those
endpoints ship.

## Install

Download the `v0.5.0` archive for your machine:

| Platform | Archive |
| --- | --- |
| macOS, Apple Silicon | [aarch64-apple-darwin](https://github.com/fiscalrail/fiscalrail-cli/releases/download/v0.5.0/fiscalrail-v0.5.0-aarch64-apple-darwin.tar.gz) |
| macOS, Intel | [x86_64-apple-darwin](https://github.com/fiscalrail/fiscalrail-cli/releases/download/v0.5.0/fiscalrail-v0.5.0-x86_64-apple-darwin.tar.gz) |
| Linux, arm64 | [aarch64-unknown-linux-musl](https://github.com/fiscalrail/fiscalrail-cli/releases/download/v0.5.0/fiscalrail-v0.5.0-aarch64-unknown-linux-musl.tar.gz) |
| Linux, x86-64 | [x86_64-unknown-linux-musl](https://github.com/fiscalrail/fiscalrail-cli/releases/download/v0.5.0/fiscalrail-v0.5.0-x86_64-unknown-linux-musl.tar.gz) |

For example, on an Apple Silicon Mac (change `target` for another platform):

```sh
version=0.5.0
target=aarch64-apple-darwin
archive="fiscalrail-v${version}-${target}.tar.gz"
base="https://github.com/fiscalrail/fiscalrail-cli/releases/download/v${version}"
tmp=$(mktemp -d)
curl -fsSL "$base/$archive" -o "$tmp/$archive"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
cd "$tmp"
grep "  $archive$" SHA256SUMS | shasum -a 256 -c -
tar -xzf "$archive"
mkdir -p "$HOME/.local/bin"
install -m 755 fiscalrail "$HOME/.local/bin/fiscalrail"
```

On Linux, use `sha256sum -c -` in place of `shasum -a 256 -c -`. Ensure
`~/.local/bin` is on your `PATH`, then run `fiscalrail --help`.

With Rust installed, you can build the same tagged version instead:

```sh
cargo install --git https://github.com/fiscalrail/fiscalrail-cli.git --tag v0.5.0 --locked fiscalrail-cli
```

The executable is `fiscalrail`. There is no separate `fr` dialect.

### Release binaries

Tagged releases publish archives for macOS (Apple Silicon and Intel) and Linux
(arm64 and x86-64, statically linked with musl) on the
[GitHub Releases page](https://github.com/fiscalrail/fiscalrail-cli/releases).
Each archive contains `fiscalrail`, this README, and the license. macOS binaries
are currently unsigned.

The release workflow runs when a `v<version>` tag matching `Cargo.toml` is
pushed from a commit on `main`. It verifies formatting, lint, and tests before
building all four targets and publishing the release. The workflow can also be
run manually from Actions to test the builds without publishing a release.

## Credentials

For scripts and CI, set `FISCALRAIL_API_KEY`. It overrides a selected profile.
For local use, add a profile. The command prompts securely on a terminal, or
reads the key from stdin when `--key-stdin` is set:

```sh
fiscalrail profiles add test
fiscalrail profiles list
fiscalrail profiles use test
fiscalrail profiles delete test --yes
```

Profiles select exactly one account and environment. `--profile NAME` overrides
`FISCALRAIL_PROFILE`, which overrides the configured default. The selected key
never falls back to another key after an authentication failure. Profile names
and the default live in `$XDG_CONFIG_HOME/fiscalrail/config.toml`, or
`~/.config/fiscalrail/config.toml`. Keys are **plaintext** in a separate
`credentials` file with mode `0600`; the directory has mode `0700`. Both files
are replaced atomically. `profiles list` reports the effective credential source
without printing the key. Public tax regime catalog reads work without a key.

## Commands

```text
account get|update
account invoicing get|update
account balance get
account tax-regime get
customers list|get ID|create|update ID|delete ID
tax-ids get ID
tax-regimes list|get ID
invoices list|get ID|issue|amend ID
invoices pdf get ID|render ID|download ID
invoice-series list|get ID|create|update ID|delete ID
payment-instructions list|get ID|create|update ID|delete ID
api-keys list|get ID|create|delete ID
events list|get ID
event-destinations list|get ID|create|update ID|delete ID|enable ID|disable ID
profiles list|add NAME|use NAME|delete NAME
```

`--data @file.json` and `--data @-` send the API's JSON payload directly. For
example:

```sh
fiscalrail customers create --data @customer.json
fiscalrail invoices issue --data @invoice.json --idempotency-key 8e97db4a-5c06-439e-a09a-d473449e45de
fiscalrail invoices pdf render in_example
fiscalrail invoices pdf download in_example --output invoice.pdf
```

PDF download only retrieves an existing render. Use `render` explicitly first
if necessary. Downloads require `--output PATH` or `--output -`, refuse terminal
binary output, and never overwrite a file. Delete commands ask for confirmation
in a terminal and require `--yes` when unattended.

List commands fetch one page. Use `--limit`, `--starting-after`, or
`--ending-before` where the API supports them. Filters include `--q` and
`--country` for customers; `--q`, `--customer`, `--issue-date-from`, and
`--issue-date-to` for invoices; and `--types` for events. `--types` is a
comma-separated list of up to 20 exact event types. The API's `has_more` value
is retained in JSON output.

Terminal output defaults to formatted JSON; piped output and `--json` use compact,
API-shaped JSON. Errors go to stderr, and `--json` makes them structured. Exit
codes: `0` success, `1` local or network error, `2` usage error, `3` API
validation, `4` authorization, `5` missing resource, `6` conflict, `7` rate
limit, `8` server error. The CLI does not automatically retry POST requests. For
an uncertain invoice issue or amendment, reuse the same idempotency key.

The default base URL is `https://api.fiscalrail.com/v1`. `--api-url` or
`FISCALRAIL_API_URL` can point to a development API.

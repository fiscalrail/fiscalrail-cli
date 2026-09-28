# FiscalRail CLI

`fiscalrail` is a Rust client for the current FiscalRail public API. It covers all
43 operations in `config/fiscal_rail/api.oas.yml` as of 2026-09-28. Commands for
proposed API endpoints in `docs/interface-design.md` will be added when those
endpoints ship.

## Install

With Rust installed:

```sh
cargo install --path .
```

The executable is `fiscalrail`. There is no separate `fr` dialect.

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

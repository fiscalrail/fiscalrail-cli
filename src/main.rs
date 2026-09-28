use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

const DEFAULT_URL: &str = "https://api.fiscalrail.com/v1";

#[derive(Debug)]
struct CliError {
    code: i32,
    message: String,
    request_id: Option<String>,
    status: Option<u16>,
    body: Option<Value>,
}
impl CliError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
            request_id: None,
            status: None,
            body: None,
        }
    }
    fn local(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
            request_id: None,
            status: None,
            body: None,
        }
    }
}
type Result<T> = std::result::Result<T, CliError>;

#[derive(Default, Debug)]
struct Options {
    profile: Option<String>,
    api_url: Option<String>,
    json: bool,
    yes: bool,
    data: Option<String>,
    output: Option<String>,
    idempotency_key: Option<String>,
    key_stdin: bool,
    limit: Option<String>,
    starting_after: Option<String>,
    ending_before: Option<String>,
    filters: BTreeMap<String, String>,
}

fn parse_args(raw: &[String]) -> Result<(Options, Vec<String>)> {
    let mut opts = Options::default();
    let mut words = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let arg = &raw[i];
        if arg == "--" {
            words.extend_from_slice(&raw[i + 1..]);
            break;
        }
        if arg == "--help" || arg == "-h" {
            words.push("help".into());
            i += 1;
            continue;
        }
        if arg == "--json" {
            opts.json = true;
            i += 1;
            continue;
        }
        if arg == "--yes" {
            opts.yes = true;
            i += 1;
            continue;
        }
        if arg == "--key-stdin" {
            opts.key_stdin = true;
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            let (flag, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
            let value = if let Some(v) = inline {
                v.to_string()
            } else {
                i += 1;
                raw.get(i)
                    .cloned()
                    .ok_or_else(|| CliError::usage(format!("{flag} needs a value")))?
            };
            if value.is_empty() {
                return Err(CliError::usage(format!("{flag} cannot be empty")));
            }
            let slot = match flag {
                "--profile" => &mut opts.profile,
                "--api-url" => &mut opts.api_url,
                "--data" => &mut opts.data,
                "--output" => &mut opts.output,
                "--idempotency-key" => &mut opts.idempotency_key,
                "--limit" => &mut opts.limit,
                "--starting-after" => &mut opts.starting_after,
                "--ending-before" => &mut opts.ending_before,
                "--q" | "--country" | "--customer" | "--issue-date-from" | "--issue-date-to"
                | "--types" => {
                    let name = flag.trim_start_matches("--").replace('-', "_");
                    if opts.filters.insert(name, value).is_some() {
                        return Err(CliError::usage(format!("{flag} repeated")));
                    }
                    i += 1;
                    continue;
                }
                _ => return Err(CliError::usage(format!("unknown option {flag}"))),
            };
            if slot.replace(value).is_some() {
                return Err(CliError::usage(format!("{flag} repeated")));
            }
        } else {
            words.push(arg.clone());
        }
        i += 1;
    }
    if opts.starting_after.is_some() && opts.ending_before.is_some() {
        return Err(CliError::usage("use only one cursor option"));
    }
    if opts.key_stdin && opts.data.as_deref() == Some("@-") {
        return Err(CliError::usage(
            "stdin cannot contain both a key and JSON data",
        ));
    }
    Ok((opts, words))
}

#[derive(Clone, Debug)]
struct Operation {
    method: &'static str,
    path: String,
    list: bool,
    body: bool,
    destructive: bool,
    public: bool,
    binary: bool,
    idempotent: bool,
    filter_names: &'static [&'static str],
}
impl Operation {
    fn new(method: &'static str, path: String) -> Self {
        Self {
            method,
            path,
            list: false,
            body: false,
            destructive: false,
            public: false,
            binary: false,
            idempotent: false,
            filter_names: &[],
        }
    }
    fn list(mut self, filters: &'static [&'static str]) -> Self {
        self.list = true;
        self.filter_names = filters;
        self
    }
    fn body(mut self) -> Self {
        self.body = true;
        self
    }
    fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
    fn public(mut self) -> Self {
        self.public = true;
        self
    }
    fn binary(mut self) -> Self {
        self.binary = true;
        self
    }
    fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }
}

fn segment(s: &str) -> Result<String> {
    if s.is_empty()
        || s == "."
        || s == ".."
        || s.contains('/')
        || s.contains('%')
        || s.contains('?')
        || s.contains('#')
        || s.chars().any(char::is_control)
    {
        return Err(CliError::usage("invalid resource ID"));
    }
    Ok(s.to_string())
}
fn member(noun: &str, id: &str) -> Result<String> {
    Ok(format!("/{noun}/{}", segment(id)?))
}

fn operation(w: &[String]) -> Result<Operation> {
    let a: Vec<&str> = w.iter().map(String::as_str).collect();
    let op = match a.as_slice() {
        ["account", "get"] => Operation::new("GET", "/account".into()),
        ["account", "update"] => Operation::new("PATCH", "/account".into()).body(),
        ["account", "invoicing", "get"] => Operation::new("GET", "/account/invoicing".into()),
        ["account", "invoicing", "update"] => {
            Operation::new("PATCH", "/account/invoicing".into()).body()
        }
        ["account", "balance", "get"] => Operation::new("GET", "/account/balance".into()),
        ["account", "tax-regime", "get"] => Operation::new("GET", "/account/tax-regime".into()),
        ["tax-ids", "get", id] => Operation::new("GET", member("tax-ids", id)?),
        ["tax-regimes", "list"] => Operation::new("GET", "/tax-regimes".into())
            .list(&[])
            .public(),
        ["tax-regimes", "get", id] => Operation::new("GET", member("tax-regimes", id)?).public(),
        ["invoices", "issue"] => Operation::new("POST", "/invoices".into())
            .body()
            .idempotent(),
        ["invoices", "amend", id] => {
            Operation::new("POST", format!("{}/amendments", member("invoices", id)?))
                .body()
                .idempotent()
        }
        ["invoices", "pdf", "get", id] => {
            Operation::new("GET", format!("{}/pdf", member("invoices", id)?))
        }
        ["invoices", "pdf", "render", id] => {
            Operation::new("POST", format!("{}/pdf", member("invoices", id)?))
        }
        ["invoices", "pdf", "download", id] => {
            Operation::new("GET", format!("{}/pdf", member("invoices", id)?)).binary()
        }
        [noun, "list"] => match *noun {
            "customers" => Operation::new("GET", format!("/{noun}")).list(&["q", "country"]),
            "invoices" => Operation::new("GET", format!("/{noun}")).list(&[
                "q",
                "customer",
                "issue_date_from",
                "issue_date_to",
            ]),
            "events" => Operation::new("GET", format!("/{noun}")).list(&["types"]),
            "invoice-series" | "payment-instructions" | "api-keys" | "event-destinations" => {
                Operation::new("GET", format!("/{noun}")).list(&[])
            }
            _ => return Err(CliError::usage("unknown command")),
        },
        [noun, "get", id] => match *noun {
            "customers"
            | "invoices"
            | "events"
            | "invoice-series"
            | "payment-instructions"
            | "api-keys"
            | "event-destinations" => Operation::new("GET", member(noun, id)?),
            _ => return Err(CliError::usage("unknown command")),
        },
        [noun, "create"] => match *noun {
            "customers"
            | "invoice-series"
            | "payment-instructions"
            | "api-keys"
            | "event-destinations" => Operation::new("POST", format!("/{noun}")).body(),
            _ => return Err(CliError::usage("unknown command")),
        },
        [noun, "update", id] => match *noun {
            "customers" | "invoice-series" | "payment-instructions" | "event-destinations" => {
                Operation::new("PATCH", member(noun, id)?).body()
            }
            _ => return Err(CliError::usage("unknown command")),
        },
        [noun, "delete", id] => match *noun {
            "customers"
            | "invoice-series"
            | "payment-instructions"
            | "api-keys"
            | "event-destinations" => Operation::new("DELETE", member(noun, id)?).destructive(),
            _ => return Err(CliError::usage("unknown command")),
        },
        ["event-destinations", action @ ("enable" | "disable"), id] => Operation::new(
            "POST",
            format!("{}/{action}", member("event-destinations", id)?),
        ),
        _ => return Err(CliError::usage("unknown command; run fiscalrail --help")),
    };
    Ok(op)
}

#[derive(Default, Serialize, Deserialize)]
struct Credentials {
    profiles: BTreeMap<String, String>,
}
#[derive(Default)]
struct Config {
    profiles: Vec<String>,
    default: Option<String>,
}

fn config_dir() -> Result<PathBuf> {
    if let Ok(x) = env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("fiscalrail"));
        }
    }
    let home = env::var_os("HOME").ok_or_else(|| CliError::local("HOME is unset"))?;
    Ok(PathBuf::from(home).join(".config/fiscalrail"))
}
fn read_config(dir: &Path) -> Result<Config> {
    let text = match fs::read_to_string(dir.join("config.toml")) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(CliError::local(e.to_string())),
    };
    let mut c = Config::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "default" => {
                c.default = Some(
                    serde_json::from_str(v.trim())
                        .map_err(|_| CliError::local("invalid config.toml default"))?,
                )
            }
            "profiles" => {
                c.profiles = serde_json::from_str(v.trim())
                    .map_err(|_| CliError::local("invalid config.toml profiles"))?
            }
            _ => {}
        }
    }
    Ok(c)
}
fn read_credentials(dir: &Path) -> Result<Credentials> {
    match fs::read(dir.join("credentials")) {
        Ok(b) => {
            serde_json::from_slice(&b).map_err(|_| CliError::local("invalid credentials file"))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Credentials::default()),
        Err(e) => Err(CliError::local(e.to_string())),
    }
}
fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::local("missing config directory"))?;
    fs::create_dir_all(parent).map_err(|e| CliError::local(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|e| CliError::local(e.to_string()))?;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| CliError::local(e.to_string()))?
        .as_nanos();
    let tmp = parent.join(format!(".fiscalrail-{}-{stamp}", process::id()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| CliError::local(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|e| CliError::local(e.to_string()))?;
    }
    let result = (|| -> io::Result<()> {
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| CliError::local(e.to_string()))
}
fn save_config(dir: &Path, c: &Config) -> Result<()> {
    let mut s = format!(
        "profiles = {}\n",
        serde_json::to_string(&c.profiles).map_err(|e| CliError::local(e.to_string()))?
    );
    if let Some(d) = &c.default {
        s.push_str(&format!(
            "default = {}\n",
            serde_json::to_string(d).map_err(|e| CliError::local(e.to_string()))?
        ));
    }
    atomic_write(&dir.join("config.toml"), s.as_bytes(), 0o600)
}
fn save_credentials(dir: &Path, c: &Credentials) -> Result<()> {
    let data = serde_json::to_vec(c).map_err(|e| CliError::local(e.to_string()))?;
    atomic_write(&dir.join("credentials"), &data, 0o600)
}

fn profiles_command(w: &[String], opts: &Options, dir: &Path) -> Result<()> {
    let mut config = read_config(dir)?;
    let mut creds = read_credentials(dir)?;
    let a: Vec<&str> = w.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["profiles", "list"] => {
            let selected = opts
                .profile
                .clone()
                .or_else(|| env::var("FISCALRAIL_PROFILE").ok())
                .or(config.default.clone());
            let source = if env::var("FISCALRAIL_API_KEY").is_ok() {
                "environment"
            } else if selected
                .as_ref()
                .is_some_and(|p| creds.profiles.contains_key(p))
            {
                "profile"
            } else {
                "none"
            };
            let value = json!({"profiles": config.profiles, "default": config.default, "selected": selected, "credential_source": source});
            print_value(&value, opts.json || !io::stdout().is_terminal());
        }
        ["profiles", "add", name] => {
            if !valid_profile(name) {
                return Err(CliError::usage(
                    "profile name must use 1-64 letters, digits, dashes or underscores",
                ));
            }
            if config.profiles.iter().any(|p| p == name) {
                return Err(CliError::usage("profile already exists"));
            }
            let key = if opts.key_stdin {
                let mut s = String::new();
                io::stdin()
                    .read_to_string(&mut s)
                    .map_err(|e| CliError::local(e.to_string()))?;
                s.trim_end_matches(['\r', '\n']).to_string()
            } else if io::stdin().is_terminal() {
                rpassword::prompt_password("API key: ")
                    .map_err(|e| CliError::local(e.to_string()))?
            } else {
                return Err(CliError::usage("use --key-stdin in unattended use"));
            };
            if !key.starts_with("ak_") || key.chars().any(char::is_whitespace) {
                return Err(CliError::usage("expected an ak_ or ak_test_ API key"));
            }
            creds.profiles.insert(name.to_string(), key);
            config.profiles.push(name.to_string());
            if config.default.is_none() {
                config.default = Some(name.to_string());
            }
            save_credentials(dir, &creds)?;
            save_config(dir, &config)?;
            eprintln!(
                "Profile {name} added. Stored keys are plaintext in a mode 0600 credentials file."
            );
        }
        ["profiles", "use", name] => {
            if !config.profiles.iter().any(|p| p == name) {
                return Err(CliError::usage("unknown profile"));
            }
            config.default = Some(name.to_string());
            save_config(dir, &config)?;
            eprintln!("Default profile: {name}");
        }
        ["profiles", "delete", name] => {
            if !config.profiles.iter().any(|p| p == name) {
                return Err(CliError::usage("unknown profile"));
            }
            confirm(opts, &format!("Delete local profile {name}?"))?;
            config.profiles.retain(|p| p != name);
            creds.profiles.remove(*name);
            if config.default.as_deref() == Some(name) {
                config.default = config.profiles.first().cloned();
            }
            save_credentials(dir, &creds)?;
            save_config(dir, &config)?;
            eprintln!("Profile {name} deleted");
        }
        _ => {
            return Err(CliError::usage(
                "use profiles list|add NAME|use NAME|delete NAME",
            ))
        }
    }
    Ok(())
}

fn confirm(opts: &Options, message: &str) -> Result<()> {
    if opts.yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::usage(
            "destructive command requires --yes in unattended use",
        ));
    }
    eprint!("{message} Type yes to continue: ");
    io::stderr()
        .flush()
        .map_err(|e| CliError::local(e.to_string()))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|e| CliError::local(e.to_string()))?;
    if answer.trim() != "yes" {
        return Err(CliError::local("cancelled"));
    }
    Ok(())
}

fn print_value(v: &Value, json_mode: bool) {
    if json_mode {
        println!("{}", serde_json::to_string(v).unwrap_or_default());
    } else if let Some(items) = v.get("data").and_then(Value::as_array) {
        for item in items {
            println!("{}", serde_json::to_string_pretty(item).unwrap_or_default());
        }
        eprintln!("has_more: {}", v.get("has_more").unwrap_or(&Value::Null));
    } else {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
    }
}

fn data_body(opts: &Options) -> Result<Vec<u8>> {
    let source = opts
        .data
        .as_deref()
        .ok_or_else(|| CliError::usage("--data @file.json or --data @- is required"))?;
    if !source.starts_with('@') {
        return Err(CliError::usage("--data must be @file.json or @-"));
    }
    let mut b = Vec::new();
    if source == "@-" {
        io::stdin()
            .read_to_end(&mut b)
            .map_err(|e| CliError::local(e.to_string()))?;
    } else {
        b = fs::read(&source[1..])
            .map_err(|e| CliError::local(format!("cannot read JSON data: {e}")))?;
    }
    let v: Value = serde_json::from_slice(&b)
        .map_err(|e| CliError::usage(format!("invalid JSON data: {e}")))?;
    if !v.is_object() {
        return Err(CliError::usage("JSON data must be an object"));
    }
    Ok(b)
}

fn credential(opts: &Options, dir: &Path, public: bool) -> Result<(Option<String>, String)> {
    if let Ok(k) = env::var("FISCALRAIL_API_KEY") {
        if k.is_empty() {
            return Err(CliError::usage("FISCALRAIL_API_KEY is empty"));
        }
        return Ok((Some(k), "environment".into()));
    }
    let c = read_config(dir)?;
    let selected = opts
        .profile
        .clone()
        .or_else(|| env::var("FISCALRAIL_PROFILE").ok())
        .or(c.default);
    if let Some(p) = selected {
        if let Some(k) = read_credentials(dir)?.profiles.get(&p) {
            return Ok((Some(k.clone()), format!("profile {p}")));
        }
        if !public {
            return Err(CliError::usage(format!("profile {p} has no credential")));
        }
    }
    if public {
        Ok((None, "public catalog".into()))
    } else {
        Err(CliError::usage(
            "no API key; set FISCALRAIL_API_KEY or add a profile",
        ))
    }
}

fn run_api(op: Operation, opts: &Options, dir: &Path) -> Result<()> {
    if opts.data.is_some() && !op.body {
        return Err(CliError::usage("--data is not supported for this command"));
    }
    if opts.key_stdin {
        return Err(CliError::usage("option is not supported for this command"));
    }
    if opts.idempotency_key.is_some() && !op.idempotent {
        return Err(CliError::usage(
            "--idempotency-key is only supported for invoice issue/amend",
        ));
    }
    if opts.output.is_some() && !op.binary {
        return Err(CliError::usage(
            "--output is only supported for binary downloads",
        ));
    }
    if op.binary && opts.output.is_none() {
        return Err(CliError::usage(
            "binary download requires --output PATH or --output -",
        ));
    }
    if op.binary && opts.output.as_deref() == Some("-") && io::stdout().is_terminal() {
        return Err(CliError::usage("refusing to write PDF bytes to a terminal"));
    }
    if op.binary
        && opts
            .output
            .as_deref()
            .is_some_and(|p| p != "-" && Path::new(p).exists())
    {
        return Err(CliError::usage("output file already exists"));
    }
    if !op.list
        && (opts.limit.is_some()
            || opts.starting_after.is_some()
            || opts.ending_before.is_some()
            || !opts.filters.is_empty())
    {
        return Err(CliError::usage("list filters require a list command"));
    }
    for f in opts.filters.keys() {
        if !op.filter_names.contains(&f.as_str()) {
            return Err(CliError::usage(format!(
                "--{} is not supported for this list",
                f.replace('_', "-")
            )));
        }
    }
    if let Some(l) = &opts.limit {
        if !matches!(l.parse::<u16>(), Ok(1..=100)) {
            return Err(CliError::usage("--limit must be 1-100"));
        }
    }
    if op.path == "/tax-regimes"
        && (opts.limit.is_some() || opts.starting_after.is_some() || opts.ending_before.is_some())
    {
        return Err(CliError::usage(
            "tax-regimes list does not accept pagination",
        ));
    }
    if let Some(types) = opts.filters.get("types") {
        if types.split(',').count() > 20 || types.split(',').any(str::is_empty) {
            return Err(CliError::usage(
                "--types accepts 1-20 comma-separated types",
            ));
        }
    }
    if let Some(k) = &opts.idempotency_key {
        if k.len() > 255 || k.chars().any(char::is_control) {
            return Err(CliError::usage("invalid idempotency key"));
        }
    }
    let body = if op.body {
        Some(data_body(opts)?)
    } else {
        None
    };
    if op.destructive {
        let (key, source) = credential(opts, dir, false)?;
        let environment = if key.as_deref().is_some_and(|k| k.starts_with("ak_test_")) {
            "Test"
        } else {
            "Live"
        };
        confirm(
            opts,
            &format!(
                "{environment} account ({source}): {} {}?",
                op.method, op.path
            ),
        )?;
    }
    let (key, source) = credential(opts, dir, op.public)?;
    eprintln!("Credential source: {source}");
    let base = opts
        .api_url
        .clone()
        .or_else(|| env::var("FISCALRAIL_API_URL").ok())
        .unwrap_or_else(|| DEFAULT_URL.into());
    let base_url =
        Url::parse(&base).map_err(|e| CliError::usage(format!("invalid API URL: {e}")))?;
    if !matches!(base_url.scheme(), "https" | "http")
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        return Err(CliError::usage(
            "API URL must be an HTTP(S) base URL without query or fragment",
        ));
    }
    let mut url = Url::parse(&format!("{}{}", base.trim_end_matches('/'), op.path))
        .map_err(|e| CliError::usage(format!("invalid API URL: {e}")))?;
    if op.list {
        let mut q = url.query_pairs_mut();
        if let Some(x) = &opts.limit {
            q.append_pair("limit", x);
        }
        if let Some(x) = &opts.starting_after {
            q.append_pair("starting_after", x);
        }
        if let Some(x) = &opts.ending_before {
            q.append_pair("ending_before", x);
        }
        for (k, v) in &opts.filters {
            q.append_pair(k, v);
        }
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| CliError::local(e.to_string()))?;
    let method = op
        .method
        .parse()
        .map_err(|_| CliError::local("invalid HTTP method"))?;
    let mut request = client.request(method, url).header(
        ACCEPT,
        if op.binary {
            "application/pdf"
        } else {
            "application/json"
        },
    );
    if let Some(k) = key {
        request = request.header(AUTHORIZATION, format!("Bearer {k}"));
    }
    if let Some(k) = &opts.idempotency_key {
        request = request.header("Idempotency-Key", k);
    }
    if let Some(b) = body {
        request = request.header(CONTENT_TYPE, "application/json").body(b);
    }
    let response = request.send().map_err(|e| {
        let hint = if op.idempotent && opts.idempotency_key.is_some() {
            "retry with the same --idempotency-key"
        } else if op.method == "POST" {
            "inspect the resource state before retrying"
        } else {
            "retry when connectivity is restored"
        };
        CliError::local(format!("request failed: {e}; {hint}"))
    })?;
    let status = response.status();
    let request_id = response
        .headers()
        .get("Request-Id")
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    if !status.is_success() {
        let bytes = response
            .bytes()
            .map_err(|e| CliError::local(e.to_string()))?;
        let body = serde_json::from_slice::<Value>(&bytes).ok();
        let message = body
            .as_ref()
            .and_then(|b| b.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or("API request failed")
            .to_string();
        let code = match status.as_u16() {
            400 | 422 => 3,
            401 | 403 => 4,
            404 => 5,
            409 => 6,
            429 => 7,
            500..=599 => 8,
            _ => 1,
        };
        return Err(CliError {
            code,
            message,
            request_id,
            status: Some(status.as_u16()),
            body,
        });
    }
    if op.binary {
        let output = opts.output.as_deref().ok_or_else(|| {
            CliError::usage("binary download requires --output PATH or --output -")
        })?;
        let bytes = response
            .bytes()
            .map_err(|e| CliError::local(e.to_string()))?;
        if output == "-" {
            if io::stdout().is_terminal() {
                return Err(CliError::usage("refusing to write PDF bytes to a terminal"));
            }
            io::stdout()
                .write_all(&bytes)
                .map_err(|e| CliError::local(e.to_string()))?;
        } else {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)
                .map_err(|e| {
                    CliError::local(format!("cannot create output without overwriting: {e}"))
                })?;
            f.write_all(&bytes)
                .map_err(|e| CliError::local(e.to_string()))?;
            eprintln!("Saved {output}");
        }
    } else if status.as_u16() == 204 {
        print_value(
            &json!({"deleted": true}),
            opts.json || !io::stdout().is_terminal(),
        );
    } else {
        let v: Value = response
            .json()
            .map_err(|e| CliError::local(format!("invalid API response: {e}")))?;
        print_value(&v, opts.json || !io::stdout().is_terminal());
    }
    Ok(())
}

fn help() {
    println!("fiscalrail [options] <resource> [nested-resource] <verb> [IDs]\n\nResources:\n  account get|update; account invoicing get|update; account balance get; account tax-regime get\n  customers list|get|create|update|delete\n  tax-ids get; tax-regimes list|get\n  invoices list|get|issue|amend; invoices pdf get|render|download\n  invoice-series, payment-instructions, event-destinations: list|get|create|update|delete\n  api-keys: list|get|create|delete; events: list|get\n  event-destinations enable|disable\n  profiles list|add|use|delete\n\nOptions: --profile NAME --api-url URL --data @FILE|@- --json --yes\n  --limit N --starting-after ID --ending-before ID --q TEXT --country CODE\n  --customer ID --issue-date-from DATE --issue-date-to DATE --types CSV\n  --idempotency-key KEY --output PATH|- --key-stdin\n\nExit codes: 0 success, 1 local/network error, 2 usage, 3 validation, 4 auth,\n  5 missing resource, 6 conflict, 7 rate limit, 8 server error.");
}
fn main() {
    let raw: Vec<String> = env::args().skip(1).collect();
    let json_requested = raw.iter().any(|x| x == "--json");
    let result = (|| -> Result<()> {
        let (opts, words) = parse_args(&raw)?;
        if words.is_empty() || words.iter().any(|x| x == "help") {
            help();
            return Ok(());
        }
        let dir = config_dir()?;
        if words.first().is_some_and(|x| x == "profiles") {
            profiles_command(&words, &opts, &dir)
        } else {
            run_api(operation(&words)?, &opts, &dir)
        }
    })();
    if let Err(e) = result {
        if json_requested {
            eprintln!(
                "{}",
                json!({"error": {"message": e.message, "status": e.status, "request_id": e.request_id, "response": e.body}})
            );
        } else {
            eprintln!(
                "Error: {}{}",
                e.message,
                e.request_id
                    .map(|x| format!(" (Request-Id: {x})"))
                    .unwrap_or_default()
            );
        }
        process::exit(e.code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_current_operations_have_distinct_commands() {
        let commands = [
            "account get",
            "account update",
            "account invoicing get",
            "account invoicing update",
            "account balance get",
            "account tax-regime get",
            "api-keys list",
            "api-keys create",
            "api-keys get key",
            "api-keys delete key",
            "events list",
            "events get event",
            "event-destinations list",
            "event-destinations create",
            "event-destinations get dest",
            "event-destinations update dest",
            "event-destinations delete dest",
            "event-destinations enable dest",
            "event-destinations disable dest",
            "invoice-series list",
            "invoice-series create",
            "invoice-series get series",
            "invoice-series update series",
            "invoice-series delete series",
            "payment-instructions list",
            "payment-instructions create",
            "payment-instructions get pi",
            "payment-instructions update pi",
            "payment-instructions delete pi",
            "tax-regimes list",
            "tax-regimes get regime",
            "tax-ids get taxid",
            "customers list",
            "customers create",
            "customers get customer",
            "customers update customer",
            "customers delete customer",
            "invoices list",
            "invoices issue",
            "invoices get invoice",
            "invoices amend invoice",
            "invoices pdf get invoice",
            "invoices pdf render invoice",
        ];
        assert_eq!(commands.len(), 43);
        let mut routes = std::collections::BTreeSet::new();
        for command in commands {
            let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
            let op = operation(&words).unwrap_or_else(|e| panic!("{command}: {}", e.message));
            assert!(
                routes.insert((op.method, op.path)),
                "duplicate route: {command}"
            );
        }
        let download: Vec<String> = "invoices pdf download invoice"
            .split_whitespace()
            .map(str::to_string)
            .collect();
        assert!(operation(&download).unwrap().binary);
    }

    #[test]
    fn rejects_proposed_and_unsafe_commands() {
        for command in [
            "account delete",
            "account invitations list",
            "invoices create",
            "invoices delete inv",
            "account invoicing logo upload file.png",
        ] {
            let words: Vec<String> = command.split_whitespace().map(str::to_string).collect();
            assert!(operation(&words).is_err(), "{command}");
        }
        assert!(segment("../other").is_err());
        assert!(segment("a%2fb").is_err());
    }

    #[test]
    fn parses_cursors_and_rejects_conflict() {
        let input = ["invoices", "list", "--starting-after", "in_1", "--json"];
        let (opts, words) = parse_args(&input.map(str::to_string)).unwrap();
        assert_eq!(words, ["invoices", "list"]);
        assert_eq!(opts.starting_after.as_deref(), Some("in_1"));
        assert!(opts.json);
        let invalid = ["--starting-after", "a", "--ending-before", "b"];
        assert!(parse_args(&invalid.map(str::to_string)).is_err());
    }

    #[test]
    fn sends_scoped_request_and_preserves_api_list_response() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 8192];
            let n = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..n]);
            assert!(
                request
                    .starts_with("GET /v1/customers?limit=2&starting_after=cus_1&q=Acme HTTP/1.1"),
                "{request}"
            );
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer ak_test_example"));
            let body = r#"{"object":"list","has_more":true,"data":[]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let dir = env::temp_dir().join(format!(
            "fiscalrail-cli-test-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = Config::default();
        config.profiles.push("test".into());
        config.default = Some("test".into());
        save_config(&dir, &config).unwrap();
        let mut creds = Credentials::default();
        creds
            .profiles
            .insert("test".into(), "ak_test_example".into());
        save_credentials(&dir, &creds).unwrap();
        let opts = Options {
            api_url: Some(format!("http://{address}/v1")),
            limit: Some("2".into()),
            starting_after: Some("cus_1".into()),
            filters: BTreeMap::from([("q".into(), "Acme".into())]),
            ..Options::default()
        };
        run_api(
            operation(&["customers".into(), "list".into()]).unwrap(),
            &opts,
            &dir,
        )
        .unwrap();
        server.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}

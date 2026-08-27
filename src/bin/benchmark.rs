#[path = "../test_support.rs"]
mod support;

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, process};

const DEFAULT_SIZES: &[usize] = &[5, 50, 500];
const DEFAULT_ACTIONS: &[&str] = &[
    "cycle",
    "focus-attention",
    "cycle-attention",
    "pane.focused",
    "pane.closed",
];

#[derive(Debug)]
struct Config {
    sizes: Vec<usize>,
    actions: Vec<String>,
    iterations: usize,
    warmup: usize,
    output_dir: PathBuf,
}

#[derive(Serialize)]
struct Sample {
    fixture: usize,
    action: String,
    implementation: &'static str,
    iteration: usize,
    elapsed_ms: f64,
}

struct BenchmarkResult {
    fixture: usize,
    action: String,
    samples: Vec<f64>,
}

struct BenchmarkSpec<'a> {
    binary: &'a Path,
    server: &'a support::MockHerdrServer,
    golden_state: &'a Path,
    action: &'a str,
    size: usize,
    iterations: usize,
    warmup: usize,
    current_pane: Option<&'a str>,
    event_json: Option<&'a Value>,
    temp_root: &'a Path,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before unix epoch")?
            .as_nanos();
        let path = PathBuf::from("/tmp").join(format!("herdr-benchmark-{}-{nonce}", process::id()));
        fs::create_dir(&path).with_context(|| format!("failed to create {}", path.display()))?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let config = parse_args(env::args().skip(1))?;
    let binary = env::var_os("HERDR_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/herdr-mru-cycle"));
    if !binary.exists() {
        bail!(
            "Rust binary not found: {}; run `make build` first",
            binary.display()
        );
    }

    report_progress(&format!(
        "starting implementation=Rust sizes={} actions={} iterations={} warmup={}",
        join_usizes(&config.sizes),
        config.actions.join(","),
        config.iterations,
        config.warmup
    ));

    fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("failed to create {}", config.output_dir.display()))?;
    let temp = TempDir::new()?;
    let mut results = Vec::new();
    let mut all_samples = Vec::new();

    for &size in &config.sizes {
        if size == 0 {
            bail!("fixture sizes must be greater than zero");
        }
        let panes = support::generate_panes(size);
        let socket_path = temp.path().join(format!("herdr-{size}.sock"));
        let server =
            support::MockHerdrServer::new(socket_path, panes.clone(), Some("pane-0000".into()))?;
        let golden_base = temp.path().join(format!("fixture-{size}"));
        fs::create_dir(&golden_base)?;

        for action in &config.actions {
            report_progress(&format!("fixture={size} action={action} setup"));
            let golden_state = golden_base.join(action);
            build_golden_state(&binary, &server, &golden_state, action, &panes, size)?;
            let (current_pane, event_json) = action_context(action, &panes, size)?;
            let samples = benchmark_action(BenchmarkSpec {
                binary: &binary,
                server: &server,
                golden_state: &golden_state,
                action,
                size,
                iterations: config.iterations,
                warmup: config.warmup,
                current_pane: current_pane.as_deref(),
                event_json: event_json.as_ref(),
                temp_root: temp.path(),
            })?;
            all_samples.extend(
                samples
                    .iter()
                    .enumerate()
                    .map(|(iteration, elapsed_ms)| Sample {
                        fixture: size,
                        action: action.clone(),
                        implementation: "Rust",
                        iteration,
                        elapsed_ms: *elapsed_ms,
                    }),
            );
            results.push(BenchmarkResult {
                fixture: size,
                action: action.clone(),
                samples,
            });
            report_progress(&format!("fixture={size} action={action} complete"));
        }
    }

    write_outputs(&config, &results, &all_samples)?;
    println!("{}", report(&config, &results));
    Ok(())
}

fn parse_args<I>(args: I) -> Result<Config>
where
    I: Iterator<Item = String>,
{
    let mut sizes = DEFAULT_SIZES.to_vec();
    let mut actions = DEFAULT_ACTIONS
        .iter()
        .map(|action| (*action).into())
        .collect();
    let mut iterations = 100;
    let mut warmup = 5;
    let default_output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("benchmarks/output");
    let mut output_dir = default_output;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--sizes" => sizes = parse_numbers(&mut args, "sizes")?,
            "--actions" => actions = parse_strings(&mut args, "actions")?,
            "--iterations" => iterations = parse_number(&mut args, "iterations")?,
            "--warmup" => warmup = parse_number(&mut args, "warmup")?,
            "--output-dir" => {
                output_dir = PathBuf::from(args.next().context("--output-dir needs a path")?);
            }
            "--help" | "-h" => {
                println!("usage: benchmark [--sizes N ...] [--actions ACTION ...] [--iterations N] [--warmup N] [--output-dir PATH]");
                process::exit(0);
            }
            _ => bail!("unknown argument: {arg}"),
        }
    }

    if sizes.is_empty() || actions.is_empty() {
        bail!("sizes and actions must not be empty");
    }
    Ok(Config {
        sizes,
        actions,
        iterations,
        warmup,
        output_dir,
    })
}

fn parse_numbers<I>(args: &mut std::iter::Peekable<I>, name: &str) -> Result<Vec<usize>>
where
    I: Iterator<Item = String>,
{
    parse_values(args, name, |value| {
        value
            .parse()
            .with_context(|| format!("invalid {name} value: {value}"))
    })
}

fn parse_strings<I>(args: &mut std::iter::Peekable<I>, name: &str) -> Result<Vec<String>>
where
    I: Iterator<Item = String>,
{
    parse_values(args, name, Ok)
}

fn parse_values<I, T, F>(args: &mut std::iter::Peekable<I>, name: &str, parse: F) -> Result<Vec<T>>
where
    I: Iterator<Item = String>,
    F: Fn(String) -> Result<T>,
{
    let mut values = Vec::new();
    while let Some(value) = args.peek() {
        if value.starts_with('-') {
            break;
        }
        values.push(parse(args.next().unwrap())?);
    }
    if values.is_empty() {
        bail!("--{name} needs at least one value");
    }
    Ok(values)
}

fn parse_number<I>(args: &mut std::iter::Peekable<I>, name: &str) -> Result<usize>
where
    I: Iterator<Item = String>,
{
    let value = args
        .next()
        .with_context(|| format!("--{name} needs a value"))?;
    value
        .parse()
        .with_context(|| format!("invalid {name} value: {value}"))
}

fn build_golden_state(
    binary: &Path,
    server: &support::MockHerdrServer,
    state_dir: &Path,
    action: &str,
    panes: &[Value],
    size: usize,
) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    if let Some(indices) = setup_indices(action, size) {
        for index in indices {
            let pane_id = panes[index].get("pane_id").and_then(Value::as_str).unwrap();
            let env = support::event_env(
                support::base_env(server.socket_path(), state_dir),
                "pane.focused",
                &json!({"pane_id": pane_id}),
            );
            let output = support::run_plugin(binary, &env)?;
            if !output.status.success() {
                bail!(
                    "setup pane.focused for {pane_id} failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
    Ok(())
}

fn setup_indices(action: &str, size: usize) -> Option<Vec<usize>> {
    match action {
        "cycle" => Some((0..size).collect()),
        "pane.focused" => Some(vec![usize::from(size > 1)]),
        "pane.closed" => Some((0..size.min(3)).collect()),
        _ => None,
    }
}

fn action_context(
    action: &str,
    panes: &[Value],
    size: usize,
) -> Result<(Option<String>, Option<Value>)> {
    match action {
        "cycle" => Ok((Some(pane_id(&panes[size - 1])?), None)),
        "focus-attention" | "cycle-attention" => Ok((Some(pane_id(&panes[0])?), None)),
        "pane.focused" => {
            let id = pane_id(&panes[0])?;
            Ok((None, Some(json!({"pane_id": id}))))
        }
        "pane.closed" => {
            let id = pane_id(&panes[size.min(3) - 1])?;
            Ok((None, Some(json!({"pane_id": id}))))
        }
        _ => bail!("unknown action: {action}"),
    }
}

fn pane_id(pane: &Value) -> Result<String> {
    pane.get("pane_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("fixture pane has no pane_id")
}

fn benchmark_action(spec: BenchmarkSpec<'_>) -> Result<Vec<f64>> {
    let mut samples = Vec::with_capacity(spec.iterations);
    for iteration in 0..(spec.warmup + spec.iterations) {
        let state_dir = spec
            .temp_root
            .join(format!("iter-{}-{iteration}", spec.size));
        fs::create_dir(&state_dir)?;
        copy_state(spec.golden_state, &state_dir)?;
        let base = support::base_env(spec.server.socket_path(), &state_dir);
        let env = if let Some(event_json) = spec.event_json {
            support::event_env(base, spec.action, event_json)
        } else {
            support::action_env(base, spec.action, spec.current_pane)
        };
        let start = std::time::Instant::now();
        let output = support::run_plugin(spec.binary, &env)?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        fs::remove_dir_all(&state_dir)?;
        spec.server.clear_focus_calls();
        if !output.status.success() {
            bail!(
                "Rust {} failed (fixture {}): {}",
                spec.action,
                spec.size,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if iteration >= spec.warmup {
            samples.push(elapsed_ms);
        }
    }
    Ok(samples)
}

fn copy_state(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), destination.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn write_outputs(config: &Config, results: &[BenchmarkResult], samples: &[Sample]) -> Result<()> {
    fs::write(
        config.output_dir.join("samples.json"),
        serde_json::to_vec_pretty(samples)?,
    )?;
    let mut csv = String::from("fixture,action,implementation,iteration,elapsed_ms\n");
    for sample in samples {
        csv.push_str(&format!(
            "{},{},{},{},{:.6}\n",
            sample.fixture,
            sample.action,
            sample.implementation,
            sample.iteration,
            sample.elapsed_ms
        ));
    }
    fs::write(config.output_dir.join("samples.csv"), csv)?;
    fs::write(config.output_dir.join("report.md"), report(config, results))?;
    Ok(())
}

fn report(config: &Config, results: &[BenchmarkResult]) -> String {
    let mut output = format!(
        "# herdr-pane-switcher Benchmark Report\n\n- Implementation: Rust\n- Iterations per action/fixture: {}\n- Warmup iterations: {}\n\n## Results by fixture and action\n\n| Fixture | Action | Impl | p50 (ms) | p95 (ms) | p99 (ms) |\n|---|---|---|---:|---:|---:|\n",
        config.iterations, config.warmup
    );
    for result in results {
        let mut values = result.samples.clone();
        values.sort_by(f64::total_cmp);
        output.push_str(&format!(
            "| {} | {} | Rust | {:.3} | {:.3} | {:.3} |\n",
            result.fixture,
            result.action,
            percentile(&values, 0.50),
            percentile(&values, 0.95),
            percentile(&values, 0.99),
        ));
    }
    output.push('\n');
    output
}

fn percentile(sorted_values: &[f64], p: f64) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let position = (sorted_values.len() - 1) as f64 * p;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        return sorted_values[lower];
    }
    sorted_values[lower] * (upper as f64 - position)
        + sorted_values[upper] * (position - lower as f64)
}

fn join_usizes(values: &[usize]) -> String {
    values
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn report_progress(message: &str) {
    eprintln!("[benchmark] {message}");
}

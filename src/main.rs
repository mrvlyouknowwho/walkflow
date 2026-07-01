mod runner;
mod workflow;

use anyhow::{anyhow, Result};
use clap::Parser;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use workflow::Workflow;

/// Step through your GitHub Actions workflow locally — run each step, pause,
/// inspect the state in a shell, edit, and continue, before you ever push.
#[derive(Parser, Debug)]
#[command(name = "walkflow", version, about)]
struct Cli {
    /// Path to a workflow file. If omitted, walkflow looks in .github/workflows/.
    workflow: Option<PathBuf>,

    /// Which job to run (required if the workflow has more than one).
    #[arg(short, long)]
    job: Option<String>,

    /// Working directory to treat as the repo/workspace root.
    #[arg(short = 'C', long)]
    workdir: Option<PathBuf>,

    /// Run every step without pausing (non-interactive).
    #[arg(short = 'y', long)]
    yes: bool,

    /// List the jobs and steps, then exit.
    #[arg(short, long)]
    list: bool,

    /// Jump to a step (number or name): earlier steps run automatically to build
    /// up state, then walkflow goes interactive from this one.
    #[arg(short, long)]
    from: Option<String>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("\x1b[31mwalkflow: {e:#}\x1b[0m");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    let workspace = match &cli.workdir {
        Some(w) => w.clone(),
        None => std::env::current_dir()?,
    };

    let wf_path = match &cli.workflow {
        Some(p) => p.clone(),
        None => discover_workflow(&workspace)?,
    };

    let wf = Workflow::load(&wf_path)?;

    if cli.list {
        println!("workflow: {}", wf.name.as_deref().unwrap_or("(unnamed)"));
        for (jid, job) in &wf.jobs {
            println!("  job {}:", jid);
            for (i, step) in job.steps.iter().enumerate() {
                println!("    {}. {}", i + 1, step.label(i));
            }
        }
        return Ok(());
    }

    let (job_id, job) = wf.select_job(cli.job.as_deref())?;

    let from = match &cli.from {
        Some(sel) => resolve_step(&job.steps, sel)?,
        None => 0,
    };

    // Seed the runner env with workflow-level env; job-level env is layered in
    // by the runner itself.
    let base_env: IndexMap<String, String> = wf.env.clone();

    println!(
        "walkflow — {} · job '{}'{}",
        wf_path.display(),
        job_id,
        if cli.yes { " · non-interactive" } else { "" }
    );

    let mut r = runner::Runner::new(workspace, base_env, cli.yes);
    let report = r.run_job(job_id, job, from)?;

    if report.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

/// Resolve a `--from` selector (a 1-based step number or a step name/substring)
/// to a 0-based step index.
fn resolve_step(steps: &[workflow::Step], sel: &str) -> Result<usize> {
    if let Ok(n) = sel.parse::<usize>() {
        if n >= 1 && n <= steps.len() {
            return Ok(n - 1);
        }
        return Err(anyhow!("--from {n} is out of range (1..={})", steps.len()));
    }
    if let Some(i) = steps.iter().position(|s| s.name.as_deref() == Some(sel)) {
        return Ok(i);
    }
    let needle = sel.to_lowercase();
    if let Some(i) = steps
        .iter()
        .enumerate()
        .position(|(i, s)| s.label(i).to_lowercase().contains(&needle))
    {
        return Ok(i);
    }
    Err(anyhow!("--from '{sel}' matched no step by number, name, or label"))
}

/// Find a workflow when the user didn't name one: scan .github/workflows for a
/// single yaml file, otherwise ask them to pick.
fn discover_workflow(workspace: &Path) -> Result<PathBuf> {
    let dir = workspace.join(".github").join("workflows");
    if !dir.is_dir() {
        return Err(anyhow!(
            "no workflow given and {} does not exist",
            dir.display()
        ));
    }
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("yml") | Some("yaml") => found.push(path),
            _ => {}
        }
    }
    found.sort();
    match found.len() {
        0 => Err(anyhow!("no .yml/.yaml workflows in {}", dir.display())),
        1 => Ok(found.into_iter().next().unwrap()),
        _ => Err(anyhow!(
            "multiple workflows found; pass one explicitly:\n{}",
            found
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

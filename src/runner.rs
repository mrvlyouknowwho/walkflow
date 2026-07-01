use crate::workflow::{Job, Step};
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Outcome of walking a job.
pub struct RunReport {
    pub ran: usize,
    pub skipped: usize,
    pub failed: usize,
}

pub struct Runner {
    workspace: PathBuf,
    /// Environment accumulated across steps (seeded from workflow/job env and
    /// grown by whatever steps write to $GITHUB_ENV, exactly like a real runner).
    env: IndexMap<String, String>,
    /// Directories steps have prepended via $GITHUB_PATH.
    path_additions: Vec<String>,
    interactive: bool,
    counter: u64,
}

enum PreAction {
    Run,
    Skip,
    Quit,
}

enum FailAction {
    Retry,
    Continue,
    Quit,
}

impl Runner {
    pub fn new(workspace: PathBuf, base_env: IndexMap<String, String>, assume_yes: bool) -> Self {
        Runner {
            workspace,
            env: base_env,
            path_additions: Vec::new(),
            interactive: !assume_yes && std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            counter: 0,
        }
    }

    pub fn run_job(&mut self, job_id: &str, job: &Job) -> Result<RunReport> {
        let job_wd = job
            .working_directory
            .as_ref()
            .map(|w| self.workspace.join(w))
            .unwrap_or_else(|| self.workspace.clone());

        for (k, v) in &job.env {
            self.env.insert(k.clone(), v.clone());
        }

        let mut report = RunReport { ran: 0, skipped: 0, failed: 0 };
        println!("\n▶ job \x1b[1m{}\x1b[0m — {} step(s)\n", job.name.as_deref().unwrap_or(job_id), job.steps.len());

        for (i, step) in job.steps.iter().enumerate() {
            let label = step.label(i);
            println!("\x1b[36m┌─ step {}/{}: {}\x1b[0m", i + 1, job.steps.len(), label);

            if step.run.is_none() {
                // A `uses:` (or empty) step — walkflow's host runner can't execute
                // marketplace actions. Make that explicit rather than silently lying.
                if let Some(u) = &step.uses {
                    println!("\x1b[33m│  uses: {u} — not executable in host mode, skipping.\x1b[0m");
                    println!("\x1b[33m│  (checkout/setup actions are usually no-ops locally; Docker runner is on the roadmap.)\x1b[0m\n");
                }
                report.skipped += 1;
                continue;
            }

            match self.prompt_pre(step)? {
                PreAction::Skip => {
                    println!("\x1b[33m└─ skipped\x1b[0m\n");
                    report.skipped += 1;
                    continue;
                }
                PreAction::Quit => {
                    println!("\x1b[33m└─ quit\x1b[0m\n");
                    break;
                }
                PreAction::Run => {}
            }

            let mut command = step.run.clone().unwrap();
            loop {
                let status = self.exec_run(step, &command, &job_wd)?;
                if status {
                    println!("\x1b[32m└─ ok\x1b[0m\n");
                    report.ran += 1;
                    break;
                }
                report.failed += 1;
                match self.prompt_fail()? {
                    FailAction::Retry => {
                        // Allow editing before retry when interactive.
                        if self.interactive {
                            if let Some(edited) = self.maybe_edit(&command)? {
                                command = edited;
                            }
                        }
                        report.failed -= 1; // this attempt superseded by the retry
                        continue;
                    }
                    FailAction::Continue => {
                        println!("\x1b[33m└─ continuing despite failure\x1b[0m\n");
                        break;
                    }
                    FailAction::Quit => {
                        println!("\x1b[33m└─ quit\x1b[0m\n");
                        return Ok(report);
                    }
                }
            }
        }

        println!(
            "── done: \x1b[32m{} ran\x1b[0m, {} skipped, \x1b[31m{} failed\x1b[0m",
            report.ran, report.skipped, report.failed
        );
        Ok(report)
    }

    /// Execute one `run:` step, threading env through $GITHUB_ENV / $GITHUB_PATH
    /// the way a real Actions runner does. Returns true on success (exit 0).
    fn exec_run(&mut self, step: &Step, command: &str, job_wd: &Path) -> Result<bool> {
        self.counter += 1;
        let tmp = std::env::temp_dir();
        let pid = std::process::id();
        let tag = format!("walkflow-{pid}-{}", self.counter);
        let github_env = tmp.join(format!("{tag}.env"));
        let github_path = tmp.join(format!("{tag}.path"));
        let github_output = tmp.join(format!("{tag}.output"));
        for f in [&github_env, &github_path, &github_output] {
            std::fs::write(f, "").ok();
        }

        let wd = step
            .working_directory
            .as_ref()
            .map(|w| self.workspace.join(w))
            .unwrap_or_else(|| job_wd.to_path_buf());

        // Build the effective environment: inherited process env, then the
        // accumulated workflow/job/GITHUB_ENV state, then this step's env.
        let mut env: IndexMap<String, String> = std::env::vars().collect();
        for (k, v) in &self.env {
            env.insert(k.clone(), v.clone());
        }
        for (k, v) in &step.env {
            env.insert(k.clone(), v.clone());
        }
        if !self.path_additions.is_empty() {
            let existing = env.get("PATH").cloned().unwrap_or_default();
            let joined = format!("{}:{}", self.path_additions.join(":"), existing);
            env.insert("PATH".into(), joined);
        }
        env.insert("CI".into(), "true".into());
        env.insert("GITHUB_ACTIONS".into(), "true".into());
        env.insert("WALKFLOW".into(), "true".into());
        env.insert("GITHUB_WORKSPACE".into(), self.workspace.display().to_string());
        env.insert("GITHUB_ENV".into(), github_env.display().to_string());
        env.insert("GITHUB_PATH".into(), github_path.display().to_string());
        env.insert("GITHUB_OUTPUT".into(), github_output.display().to_string());

        let (program, args) = shell_invocation(step.shell.as_deref(), command);

        println!("\x1b[36m│  running in {}\x1b[0m", wd.display());
        let status = Command::new(&program)
            .args(&args)
            .current_dir(&wd)
            .env_clear()
            .envs(&env)
            .status()
            .with_context(|| format!("spawning shell '{program}' for step"))?;

        // Absorb whatever the step exported, so later steps see it.
        if let Ok(content) = std::fs::read_to_string(&github_env) {
            merge_github_env(&content, &mut self.env);
        }
        if let Ok(content) = std::fs::read_to_string(&github_path) {
            for line in content.lines() {
                let d = line.trim();
                if !d.is_empty() {
                    self.path_additions.push(d.to_string());
                }
            }
        }
        for f in [&github_env, &github_path, &github_output] {
            std::fs::remove_file(f).ok();
        }

        Ok(status.success())
    }

    fn prompt_pre(&self, step: &Step) -> Result<PreAction> {
        if let Some(cond) = &step.cond {
            println!("\x1b[90m│  if: {cond} (walkflow does not evaluate conditions; your call)\x1b[0m");
        }
        if let Some(run) = &step.run {
            for line in run.lines() {
                println!("\x1b[90m│    $ {line}\x1b[0m");
            }
        }
        if !self.interactive {
            return Ok(PreAction::Run);
        }
        loop {
            let ans = prompt("│  [enter] run · [s]hell · [k] skip · [q] quit > ")?;
            match ans.trim() {
                "" | "r" | "c" => return Ok(PreAction::Run),
                "s" => self.open_shell()?,
                "k" => return Ok(PreAction::Skip),
                "q" => return Ok(PreAction::Quit),
                other => println!("│  ? '{other}' — pick enter/s/k/q"),
            }
        }
    }

    fn prompt_fail(&self) -> Result<FailAction> {
        if !self.interactive {
            return Ok(FailAction::Quit);
        }
        loop {
            let ans = prompt("\x1b[31m│  step failed.\x1b[0m [r]etry · [s]hell · [c]ontinue · [q]uit > ")?;
            match ans.trim() {
                "r" | "" => return Ok(FailAction::Retry),
                "s" => self.open_shell()?,
                "c" => return Ok(FailAction::Continue),
                "q" => return Ok(FailAction::Quit),
                other => println!("│  ? '{other}' — pick r/s/c/q"),
            }
        }
    }

    /// Drop the user into an interactive shell in the workspace with the current
    /// accumulated environment — the core "inspect the state right now" feature.
    fn open_shell(&self) -> Result<()> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let mut env: IndexMap<String, String> = std::env::vars().collect();
        for (k, v) in &self.env {
            env.insert(k.clone(), v.clone());
        }
        if !self.path_additions.is_empty() {
            let existing = env.get("PATH").cloned().unwrap_or_default();
            env.insert("PATH".into(), format!("{}:{}", self.path_additions.join(":"), existing));
        }
        println!("\x1b[90m│  entering shell ({shell}); `exit` to return to walkflow\x1b[0m");
        Command::new(&shell)
            .current_dir(&self.workspace)
            .env_clear()
            .envs(&env)
            .status()
            .with_context(|| format!("spawning interactive shell {shell}"))?;
        println!("\x1b[90m│  back in walkflow\x1b[0m");
        Ok(())
    }

    /// Offer to edit the command before a retry. Uses $EDITOR; returns the new
    /// command if it changed.
    fn maybe_edit(&self, command: &str) -> Result<Option<String>> {
        let ans = prompt("│  [e] edit command before retry, or [enter] retry as-is > ")?;
        if ans.trim() != "e" {
            return Ok(None);
        }
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
        let tmp = std::env::temp_dir().join(format!("walkflow-edit-{}.sh", std::process::id()));
        std::fs::write(&tmp, command)?;
        Command::new(&editor)
            .arg(&tmp)
            .status()
            .with_context(|| format!("opening editor {editor}"))?;
        let edited = std::fs::read_to_string(&tmp)?;
        std::fs::remove_file(&tmp).ok();
        let edited = edited.trim_end_matches('\n').to_string();
        if edited == command {
            Ok(None)
        } else {
            Ok(Some(edited))
        }
    }
}

/// Map an Actions `shell:` value to a program + args, defaulting to bash with
/// the same strict flags Actions uses, falling back to sh.
fn shell_invocation(shell: Option<&str>, command: &str) -> (String, Vec<String>) {
    match shell.map(|s| s.trim()) {
        Some("sh") => ("sh".into(), vec!["-e".into(), "-c".into(), command.into()]),
        Some("bash") | None => (
            "bash".into(),
            vec!["--noprofile".into(), "--norc".into(), "-eo".into(), "pipefail".into(), "-c".into(), command.into()],
        ),
        Some(other) => {
            // pwsh/python/etc: pass through as `<prog> -c` best-effort.
            (other.into(), vec!["-c".into(), command.into()])
        }
    }
}

/// Merge the KEY=VALUE and heredoc (KEY<<DELIM ... DELIM) lines a step wrote to
/// $GITHUB_ENV into the accumulated env map.
fn merge_github_env(content: &str, env: &mut IndexMap<String, String>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((key, delim)) = line.split_once("<<") {
            let key = key.trim().to_string();
            let delim = delim.trim();
            let mut value = String::new();
            i += 1;
            while i < lines.len() && lines[i] != delim {
                if !value.is_empty() {
                    value.push('\n');
                }
                value.push_str(lines[i]);
                i += 1;
            }
            if !key.is_empty() {
                env.insert(key, value);
            }
        } else if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty() {
                env.insert(key.to_string(), val.to_string());
            }
        }
        i += 1;
    }
}

fn prompt(msg: &str) -> Result<String> {
    print!("{msg}");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    let n = std::io::stdin().read_line(&mut line)?;
    if n == 0 {
        // EOF on stdin — behave as "continue/quit" without looping forever.
        return Ok("q".into());
    }
    Ok(line)
}

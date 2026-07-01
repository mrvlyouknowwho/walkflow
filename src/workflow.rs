use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::Path;

/// A parsed GitHub Actions workflow file. We intentionally parse a permissive
/// subset: the fields walkflow's local step-runner actually uses. Unknown keys
/// are ignored so real-world workflows load without choking.
#[derive(Debug, Deserialize)]
pub struct Workflow {
    pub name: Option<String>,
    #[serde(default)]
    pub env: IndexMap<String, String>,
    pub jobs: IndexMap<String, Job>,
}

#[derive(Debug, Deserialize)]
pub struct Job {
    pub name: Option<String>,
    #[serde(default)]
    pub env: IndexMap<String, String>,
    #[serde(default, rename = "working-directory")]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct Step {
    pub name: Option<String>,
    pub run: Option<String>,
    pub uses: Option<String>,
    pub shell: Option<String>,
    #[serde(default)]
    pub env: IndexMap<String, String>,
    #[serde(default, rename = "working-directory")]
    pub working_directory: Option<String>,
    #[serde(default, rename = "if")]
    pub cond: Option<String>,
}

impl Step {
    /// A human label for the step, mirroring how the Actions UI names steps.
    pub fn label(&self, index: usize) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        if let Some(u) = &self.uses {
            return format!("uses {u}");
        }
        if let Some(r) = &self.run {
            let first = r.lines().next().unwrap_or("").trim();
            return format!("run: {first}");
        }
        format!("step {}", index + 1)
    }
}

impl Workflow {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading workflow file {}", path.display()))?;
        let wf: Workflow = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing workflow YAML {}", path.display()))?;
        if wf.jobs.is_empty() {
            return Err(anyhow!("workflow {} has no jobs", path.display()));
        }
        Ok(wf)
    }

    /// Pick the job to run. If `wanted` is given it must exist; otherwise, if
    /// there is exactly one job take it, else require the caller to choose.
    pub fn select_job<'a>(&'a self, wanted: Option<&str>) -> Result<(&'a str, &'a Job)> {
        match wanted {
            Some(w) => self
                .jobs
                .get_key_value(w)
                .map(|(k, v)| (k.as_str(), v))
                .ok_or_else(|| {
                    anyhow!(
                        "job '{}' not found. Available jobs: {}",
                        w,
                        self.job_names().join(", ")
                    )
                }),
            None => {
                if self.jobs.len() == 1 {
                    let (k, v) = self.jobs.iter().next().unwrap();
                    Ok((k.as_str(), v))
                } else {
                    Err(anyhow!(
                        "workflow has {} jobs; pick one with --job. Available: {}",
                        self.jobs.len(),
                        self.job_names().join(", ")
                    ))
                }
            }
        }
    }

    pub fn job_names(&self) -> Vec<String> {
        self.jobs.keys().cloned().collect()
    }
}

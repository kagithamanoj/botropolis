//! The CEO orchestrator of Botropolis.
//!
//! The CEO takes a user request, decomposes it into subtasks, routes
//! each subtask to the right department using keyword matching against
//! the agent registry, collects the agents' results, and synthesizes
//! a CompanyReport. Mirrors `botropolis/core/orchestrator.py`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::agent::{Agent, RunOpts};
use crate::evidence::run_checks;
use crate::judge::{report_text, run_judge};
use crate::ledger::Ledger;
use crate::memory::MemoryStore;
use crate::registry::AgentRegistry;
use crate::runstate::RunStore;
use crate::schemas::new_run_id;
use crate::schemas::{
    AgentResult, CheckResult, CompanyReport, Context, Contract, Task, TeamReport, TeamRound,
};

/// Keywords that suggest a department should be involved. Ordered by
/// department so routing is deterministic.
pub const DEPARTMENT_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "research",
        &[
            "research", "find", "search", "look up", "explain", "report", "news", "trend", "fact",
            "compare", "survey", "overview",
        ],
    ),
    (
        "health",
        &[
            "health",
            "symptom",
            "sleep",
            "fitness",
            "diet",
            "exercise",
            "wellness",
            "medical",
            "doctor",
            "workout",
            "nutrition",
        ],
    ),
    (
        "finance",
        &[
            "finance",
            "stock",
            "market",
            "invest",
            "budget",
            "money",
            "equity",
            "portfolio",
            "price",
            "valuation",
            "spending",
        ],
    ),
    (
        "code",
        &[
            "code",
            "python",
            "function",
            "script",
            "bug",
            "debug",
            "program",
            "api",
            "deploy",
            "ci/cd",
            "pipeline",
            "refactor",
            "review the code",
        ],
    ),
    (
        "data",
        &[
            "data",
            "dataset",
            "pipeline",
            "etl",
            "model training",
            "train a model",
            "machine learning",
            "ml ",
            "feature",
        ],
    ),
    (
        "legal",
        &[
            "legal",
            "law",
            "contract",
            "compliance",
            "regulation",
            "clause",
            "liability",
            "terms",
        ],
    ),
    (
        "marketing",
        &[
            "marketing",
            "copy",
            "seo",
            "blog",
            "headline",
            "brand",
            "campaign",
            "landing page",
        ],
    ),
    (
        "ops",
        &[
            "schedule", "calendar", "meeting", "email", "inbox", "reminder", "organize", "plan my",
        ],
    ),
    (
        "security",
        &[
            "security",
            "vulnerability",
            "vuln",
            "secure",
            "breach",
            "pentest",
            "audit",
            "cve",
            "threat",
        ],
    ),
    (
        "support",
        &[
            "support",
            "help me with",
            "customer",
            "ticket",
            "complaint",
            "troubleshoot",
        ],
    ),
];

/// Preferred agent per department for whole-request routing.
pub const LEAD_AGENT: &[(&str, &str)] = &[
    ("research", "scout"),
    ("health", "triage"),
    ("finance", "markets"),
    ("code", "coder"),
    ("data", "pipelines"),
    ("legal", "paralegal"),
    ("marketing", "copywriter"),
    ("ops", "scheduler"),
    ("security", "auditor"),
    ("support", "support"),
];

/// Hard ceiling on team collaboration rounds. Each round runs every
/// agent, so costs grow with agents * rounds; three is enough for
/// draft, critique, revise without runaway bills.
pub const MAX_TEAM_ROUNDS: u32 = 3;

/// How much of each teammate's prior output to include in the next prompt.
pub const PRIOR_OUTPUT_CHARS: usize = 800;

/// Chat history attached to tasks: last N turns, each capped, so
/// follow-up questions carry context without blowing up the prompt.
pub const HISTORY_TURNS: usize = 40;
pub const HISTORY_TURN_CHARS: usize = 2000;
pub const HISTORY_CHARS: usize = 48000;

/// Long-term memory attached to tasks: the top few facts matching the
/// request, packed small.
pub const MEMORY_RECALL_LIMIT: usize = 5;
pub const MEMORY_CHARS: usize = 2000;

/// One earlier turn of a chat, for conversational context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// Pack recent chat turns into a Context. Empty Context when none.
pub fn history_context(history: Option<&[ChatTurn]>) -> Context {
    let Some(history) = history else {
        return Context::new();
    };
    if history.is_empty() {
        return Context::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let start = history.len().saturating_sub(HISTORY_TURNS);
    for turn in &history[start..] {
        let content: String = turn
            .content
            .trim()
            .chars()
            .take(HISTORY_TURN_CHARS)
            .collect();
        if !content.is_empty() {
            lines.push(format!("{}: {}", turn.role, content));
        }
    }
    let mut total: usize = lines.iter().map(|l| l.len() + 1).sum();
    while !lines.is_empty() && total > HISTORY_CHARS {
        total -= lines[0].len() + 1;
        lines.remove(0);
    }
    if lines.is_empty() {
        return Context::new();
    }
    let mut context = Context::new();
    context.chat_history = lines.join("\n");
    context
}

/// Recall the facts matching a request, packed as context lines.
/// Empty string when there is no store or nothing matches.
pub fn memory_context(store: Option<&MemoryStore>, request: &str) -> String {
    let Some(store) = store else {
        return String::new();
    };
    let found = match store.recall(request, MEMORY_RECALL_LIMIT) {
        Ok(found) => found,
        Err(_) => return String::new(),
    };
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0usize;
    for memory in found {
        let agent = if memory.agent.is_empty() {
            "unknown".to_string()
        } else {
            memory.agent.clone()
        };
        let line = format!("- [{agent}] {}", memory.text);
        if total + line.len() + 1 > MEMORY_CHARS {
            break;
        }
        total += line.len() + 1;
        lines.push(line);
    }
    lines.join("\n")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Orchestrates the company: plans work, assigns agents, reports back.
#[derive(Debug, Clone)]
pub struct CEO {
    pub registry: Arc<AgentRegistry>,
    pub ledger: Ledger,
    pub memory: Option<MemoryStore>,
    /// Where resumable run state is written, when set.
    pub run_store: Option<RunStore>,
}

impl CEO {
    /// The CEO has a name. Agents are addressed by first name everywhere.
    pub const NAME: &'static str = "Manoj";

    /// Build a CEO. The registry defaults to loading the specs on
    /// disk; the ledger defaults to in-memory (hermetic, no disk side
    /// effects). Pass a file ledger to persist the trail.
    pub fn new(
        registry: Option<Arc<AgentRegistry>>,
        ledger: Option<Ledger>,
        memory: Option<MemoryStore>,
    ) -> Result<Self, String> {
        let registry = match registry {
            Some(registry) => registry,
            None => Arc::new(AgentRegistry::load().map_err(|e| e.to_string())?),
        };
        Ok(CEO {
            registry,
            ledger: ledger.unwrap_or_else(Ledger::in_memory),
            memory,
            run_store: None,
        })
    }

    /// Build a CEO for one tenant, with that tenant's isolated
    /// memory, ledger, and run state. The default tenant keeps the
    /// old paths: the shared memory store, an in-memory ledger, and
    /// the shared run state directory.
    pub fn for_tenant(
        registry: Option<Arc<AgentRegistry>>,
        tenant_id: &str,
    ) -> Result<Self, String> {
        let registry = match registry {
            Some(registry) => registry,
            None => Arc::new(AgentRegistry::load().map_err(|e| e.to_string())?),
        };
        if tenant_id == crate::tenancy::DEFAULT_TENANT {
            let mut ceo = CEO::new(
                Some(Arc::clone(&registry)),
                None,
                Some(crate::memory::default_memory_store()),
            )?;
            ceo.run_store = Some(RunStore::new(None));
            return Ok(ceo);
        }
        let memory = crate::memory::MemoryStore::open(crate::paths::tenant_memory_path(tenant_id))?;
        let ledger = Ledger::open(crate::paths::tenant_ledger_path(tenant_id))?;
        let mut ceo = CEO::new(Some(registry), Some(ledger), Some(memory))?;
        ceo.run_store = Some(RunStore::new(Some(crate::paths::tenant_runs_dir(
            tenant_id,
        ))));
        Ok(ceo)
    }

    fn record(&self, run_id: &str, kind: &str, payload: Value) {
        let _ = self.ledger.record(run_id, kind, Self::NAME, payload);
    }

    /// Run a contract's machine checks, then the judge, and return
    /// the evidence, the judge outcome, and the report status.
    ///
    /// The builder/judge split: agents claim the work is done, the
    /// harness checks before the report is final. All checks passing
    /// keeps status "done"; any failure makes it "unverified". No
    /// contract, nothing changes. When the machine checks already
    /// failed, the judge does not run.
    fn verify(
        &self,
        run_id: &str,
        contract: Option<&Contract>,
        summary: &str,
        outputs: &[(String, String)],
    ) -> (Vec<CheckResult>, HashMap<String, Value>, String) {
        let Some(contract) = contract else {
            return (Vec::new(), HashMap::new(), "done".to_string());
        };
        let mut evidence: Vec<CheckResult> = Vec::new();
        if !contract.checks.is_empty() {
            evidence = run_checks(&contract.checks);
            let passed = evidence.iter().filter(|item| item.passed).count();
            let status = if passed == evidence.len() {
                "done".to_string()
            } else {
                "unverified".to_string()
            };
            self.record(
                run_id,
                "evidence_checked",
                json!({"passed": passed, "total": evidence.len(), "status": status}),
            );
            if status != "done" {
                let judge = HashMap::from([
                    ("verdict".to_string(), json!("skipped")),
                    ("reason".to_string(), json!("machine checks failed")),
                ]);
                self.record(
                    run_id,
                    "judge_skipped",
                    json!({"reason": "machine checks failed"}),
                );
                return (evidence, judge, status);
            }
        }
        let outcome = run_judge(&self.registry, contract, &report_text(summary, outputs));
        let mut judge = HashMap::new();
        judge.insert("verdict".to_string(), json!(outcome.verdict));
        judge.insert("reason".to_string(), json!(outcome.reason));
        match outcome.verdict.as_str() {
            "fail" => {
                evidence.push(CheckResult {
                    command: "judge".to_string(),
                    expected_exit_code: 0,
                    exit_code: Some(1),
                    passed: false,
                    output_tail: outcome.reason.clone(),
                });
                self.record(run_id, "judge_failed", json!({"reason": outcome.reason}));
                (evidence, judge, "unverified".to_string())
            }
            "pass" => {
                self.record(run_id, "judge_passed", json!({}));
                (evidence, judge, "done".to_string())
            }
            _ => {
                self.record(run_id, "judge_skipped", json!({"reason": outcome.reason}));
                (evidence, judge, "done".to_string())
            }
        }
    }

    /// Score departments by keyword hits, highest first.
    pub fn score_departments(&self, request: &str) -> Vec<(String, usize)> {
        let lowered = request.to_lowercase();
        let departments = self.registry.departments();
        let mut scores: Vec<(String, usize)> = Vec::new();
        for (dept, keywords) in DEPARTMENT_KEYWORDS {
            if !departments.iter().any(|d| d == dept) {
                continue;
            }
            let hits = keywords.iter().filter(|kw| lowered.contains(**kw)).count();
            if hits > 0 {
                scores.push((dept.to_string(), hits));
            }
        }
        scores.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scores
    }

    /// Decompose a request into one task per relevant department.
    ///
    /// Routing sees the recent history too, but only when the request
    /// itself names no department: the current words always win, and
    /// history rescues keyword-free follow-ups.
    pub fn plan(
        &self,
        request: &str,
        history: Option<&[ChatTurn]>,
        contract: Option<Contract>,
        needs_approval: &[String],
        approved: bool,
    ) -> Vec<Task> {
        let mut scored = self.score_departments(request);
        if scored.is_empty() {
            if let Some(history) = history {
                let hist_text = history
                    .iter()
                    .rev()
                    .take(HISTORY_TURNS)
                    .rev()
                    .map(|t| t.content.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                scored = self.score_departments(&format!("{request} {hist_text}"));
            }
        }
        let mut departments: Vec<String> =
            scored.into_iter().take(3).map(|(dept, _)| dept).collect();
        if departments.is_empty() {
            let available = self.registry.departments();
            if available.iter().any(|d| d == "research") {
                departments = vec!["research".to_string()];
            } else if let Some(first) = available.into_iter().next() {
                departments = vec![first];
            }
        }
        let mut context = history_context(history);
        let recalled = memory_context(self.memory.as_ref(), request);
        if !recalled.is_empty() {
            context.memories = recalled;
        }
        let multi = departments.len() > 1;
        let mut out = Vec::new();
        for dept in departments {
            let description = if multi {
                format!(
                    "{request} (focus: {})",
                    request.chars().take(80).collect::<String>()
                )
            } else {
                request.to_string()
            };
            let mut task = Task::new(description);
            task.department = Some(dept);
            task.context = context.clone();
            task.contract = contract.clone();
            task.needs_approval = needs_approval.to_vec();
            task.approved = approved;
            out.push(task);
        }
        out
    }

    /// Pick the best agent in the task's department.
    pub fn assign(&self, task: &Task) -> Result<Arc<Agent>, String> {
        let dept = task.department.clone().unwrap_or_default();
        let agents = self.registry.by_department(&dept);
        if agents.is_empty() {
            return Err(format!("No agents in department: {dept}"));
        }
        if let Some(name) = &task.agent_name {
            return self.registry.get(name).map_err(|e| e.to_string());
        }
        let lead = LEAD_AGENT
            .iter()
            .find(|(d, _)| *d == dept.to_lowercase())
            .map(|(_, lead)| *lead);
        if let Some(lead) = lead {
            if let Ok(agent) = self.registry.get(lead) {
                return Ok(agent);
            }
        }
        Ok(agents.into_iter().next().unwrap())
    }

    /// Run the full company workflow for one user request.
    pub fn handle(
        &self,
        request: &str,
        history: Option<&[ChatTurn]>,
        contract: Option<Contract>,
        needs_approval: &[String],
        approved: bool,
    ) -> Result<CompanyReport, String> {
        let run_id = new_run_id();
        let started = Instant::now();
        let mut start_payload = json!({"request": request});
        if let Some(contract) = &contract {
            start_payload["contract"] = serde_json::to_value(contract).unwrap_or(Value::Null);
        }
        if !needs_approval.is_empty() {
            start_payload["needs_approval"] = json!(needs_approval);
            start_payload["approved"] = json!(approved);
        }
        self.record(&run_id, "run_started", start_payload);
        let tasks = self.plan(request, history, contract.clone(), needs_approval, approved);
        let mut results: Vec<AgentResult> = Vec::new();
        let mut departments: Vec<String> = Vec::new();
        let mut error: Option<String> = None;
        for mut task in tasks {
            let agent = match self.assign(&task) {
                Ok(agent) => agent,
                Err(e) => {
                    error = Some(e);
                    break;
                }
            };
            task.agent_name = Some(agent.name.clone());
            let store = self.run_store.clone();
            let result = agent.run(
                task,
                RunOpts {
                    ledger: Some(self.ledger.clone()),
                    run_id: Some(run_id.clone()),
                    store,
                    ..Default::default()
                },
            );
            if !departments.contains(&agent.department) {
                departments.push(agent.department.clone());
            }
            results.push(result);
        }
        if let Some(e) = error {
            self.record(&run_id, "run_failed", json!({"error": e}));
            return Err(e);
        }
        let summary = Self::synthesize(request, &results);
        let outputs: Vec<(String, String)> = results
            .iter()
            .map(|r| (r.agent_name.clone(), r.output.clone()))
            .collect();
        let (evidence, judge, status) = self.verify(&run_id, contract.as_ref(), &summary, &outputs);
        let report = CompanyReport {
            request: request.to_string(),
            departments_involved: departments.clone(),
            results,
            summary,
            contract,
            evidence,
            judge,
            status,
            elapsed_seconds: started.elapsed().as_secs_f64(),
            created_at: now_secs(),
        };
        self.record(
            &run_id,
            "run_finished",
            json!({
                "departments": departments,
                "results": report.results.len(),
                "successful": report.successful(),
                "elapsed_seconds": report.elapsed_seconds,
            }),
        );
        Ok(report)
    }

    /// Run named agents as a team in collaboration rounds.
    ///
    /// Each agent's prompt includes the original request plus the
    /// outputs of every teammate that ran before it, so later turns
    /// can build on, critique, or revise earlier work. Rounds are
    /// capped at MAX_TEAM_ROUNDS. Unknown agent names raise an error;
    /// an empty team raises an error.
    pub fn team(
        &self,
        request: &str,
        agent_names: &[String],
        rounds: u32,
        contract: Option<Contract>,
        needs_approval: &[String],
        approved: bool,
    ) -> Result<TeamReport, String> {
        if agent_names.is_empty() {
            return Err("team() needs at least one agent".to_string());
        }
        let mut deduped: Vec<String> = Vec::new();
        for name in agent_names {
            if !deduped.contains(name) {
                deduped.push(name.clone());
            }
        }
        let mut agents: Vec<Arc<Agent>> = Vec::new();
        for name in &deduped {
            match self.registry.get(name) {
                Ok(agent) => agents.push(agent),
                Err(_) => return Err(format!("Unknown agent: {name}")),
            }
        }
        let rounds = rounds.clamp(1, MAX_TEAM_ROUNDS);
        let run_id = new_run_id();
        let started = Instant::now();
        let mut start_payload = json!({
            "request": request,
            "team": agents.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
        });
        if let Some(contract) = &contract {
            start_payload["contract"] = serde_json::to_value(contract).unwrap_or(Value::Null);
        }
        self.record(&run_id, "run_started", start_payload);
        let mut team_rounds: Vec<TeamRound> = Vec::new();
        let mut prior: Vec<(u32, String, String)> = Vec::new();
        for round_num in 1..=rounds {
            for agent in &agents {
                let mut task = Task::new(format!(
                    "Team request (round {round_num} of {rounds}): {request}"
                ));
                task.department = Some(agent.department.clone());
                task.agent_name = Some(agent.name.clone());
                task.context = self.team_context(&prior);
                task.contract = contract.clone();
                task.needs_approval = needs_approval.to_vec();
                task.approved = approved;
                let store = self.run_store.clone();
                let result = agent.run(
                    task,
                    RunOpts {
                        ledger: Some(self.ledger.clone()),
                        run_id: Some(run_id.clone()),
                        store,
                        ..Default::default()
                    },
                );
                prior.push((round_num, agent.name.clone(), result.output.clone()));
                team_rounds.push(TeamRound {
                    round_number: round_num,
                    agent_name: agent.name.clone(),
                    department: agent.department.clone(),
                    result,
                });
            }
        }
        let synthesis = Self::synthesize_team(request, &agents, &team_rounds);
        let outputs: Vec<(String, String)> = team_rounds
            .iter()
            .map(|r| (r.agent_name.clone(), r.result.output.clone()))
            .collect();
        let (evidence, judge, status) =
            self.verify(&run_id, contract.as_ref(), &synthesis, &outputs);
        let report = TeamReport {
            request: request.to_string(),
            agents: agents.iter().map(|a| a.name.clone()).collect(),
            rounds: team_rounds,
            synthesis,
            contract,
            evidence,
            judge,
            status,
            elapsed_seconds: started.elapsed().as_secs_f64(),
            created_at: now_secs(),
        };
        self.record(
            &run_id,
            "run_finished",
            json!({
                "agents": report.agents,
                "rounds": report.rounds.len(),
                "successful": report.successful(),
                "elapsed_seconds": report.elapsed_seconds,
            }),
        );
        Ok(report)
    }

    fn team_context(&self, prior: &[(u32, String, String)]) -> Context {
        if prior.is_empty() {
            return Context::new();
        }
        let mut chunks = Vec::new();
        for (round_num, agent_name, output) in prior {
            let snippet: String = output.trim().chars().take(PRIOR_OUTPUT_CHARS).collect();
            chunks.push(format!("[round {round_num} - {agent_name}]\n{snippet}"));
        }
        let mut context = Context::new();
        context.teammates_so_far = chunks.join("\n\n");
        context
    }

    /// Summarize the final state of a team session.
    fn synthesize_team(request: &str, agents: &[Arc<Agent>], team_rounds: &[TeamRound]) -> String {
        let num_rounds = team_rounds
            .iter()
            .map(|r| r.round_number)
            .max()
            .unwrap_or(0);
        let mut lines = vec![
            format!("Team request: {request}"),
            format!(
                "Team: {} ({} runs over {} round{})",
                agents
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                team_rounds.len(),
                num_rounds,
                if num_rounds != 1 { "s" } else { "" }
            ),
            String::new(),
            "Final state per agent:".to_string(),
        ];
        let mut final_rounds: HashMap<&str, &TeamRound> = HashMap::new();
        for team_round in team_rounds {
            final_rounds.insert(team_round.agent_name.as_str(), team_round);
        }
        for (name, team_round) in &final_rounds {
            let status = if team_round.result.success {
                "done"
            } else {
                "failed"
            };
            let output = team_round.result.output.trim();
            let first_line = output.lines().next().unwrap_or("(no output)");
            lines.push(format!("[{name}] {status}: {first_line}"));
        }
        if !team_rounds.is_empty()
            && team_rounds.iter().all(|r| {
                r.result
                    .metadata
                    .get("stub")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
        {
            lines.push(String::new());
            lines.push(
                "All runs used the offline stub: reasoning shape only, no live model calls."
                    .to_string(),
            );
        }
        lines.join("\n")
    }

    /// Combine agent outputs into an executive summary.
    fn synthesize(request: &str, results: &[AgentResult]) -> String {
        let mut lines = vec![
            format!("Request: {request}"),
            format!(
                "Departments consulted: {}",
                results
                    .iter()
                    .map(|r| r.department.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            String::new(),
        ];
        for result in results {
            let status = if result.success { "done" } else { "failed" };
            let output = result.output.trim();
            let first_line = output.lines().next().unwrap_or("(no output)");
            lines.push(format!(
                "[{} / {}] {status}: {first_line}",
                result.department, result.agent_name
            ));
        }
        lines.push(String::new());
        lines.push(
            "Full per-agent outputs are included in the report results. Consult a human specialist before acting on health, legal, or financial guidance."
                .to_string(),
        );
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ScriptedClient;

    fn test_ceo() -> CEO {
        CEO::new(None, None, None).unwrap()
    }

    #[test]
    fn research_question_goes_to_research() {
        let ceo = test_ceo();
        let tasks = ceo.plan("research the history of the abacus", None, None, &[], false);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].department.as_deref(), Some("research"));
    }

    #[test]
    fn plan_attaches_chat_history() {
        let ceo = test_ceo();
        let history = vec![
            ChatTurn {
                role: "user".to_string(),
                content: "Tell me about abacuses.".to_string(),
            },
            ChatTurn {
                role: "assistant".to_string(),
                content: "They are old counting tools.".to_string(),
            },
        ];
        let tasks = ceo.plan(
            "and how long does it last?",
            Some(&history),
            None,
            &[],
            false,
        );
        assert!(!tasks[0].context.chat_history.is_empty());
        assert!(tasks[0].context.chat_history.contains("abacuses"));
    }

    #[test]
    fn history_context_caps_turns() {
        let history: Vec<ChatTurn> = (0..50)
            .map(|i| ChatTurn {
                role: "user".to_string(),
                content: format!("turn {i}"),
            })
            .collect();
        let context = history_context(Some(&history));
        assert!(!context.chat_history.contains("turn 0"));
        assert!(context.chat_history.contains("turn 49"));
    }

    #[test]
    fn assign_prefers_lead_agent() {
        let ceo = test_ceo();
        let mut task = Task::new("research the abacus");
        task.department = Some("research".to_string());
        let agent = ceo.assign(&task).unwrap();
        assert_eq!(agent.name, "scout");
    }

    #[test]
    fn tenant_ceos_keep_memory_apart() {
        let _guard = crate::test_support::lock_env();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BOTROPOLIS_DATA_DIR", dir.path());
        let registry = Arc::new(AgentRegistry::load().unwrap());
        let ceo_a = CEO::for_tenant(Some(Arc::clone(&registry)), "team-a").unwrap();
        let ceo_b = CEO::for_tenant(Some(Arc::clone(&registry)), "team-b").unwrap();
        let store_a = ceo_a.memory.as_ref().unwrap();
        store_a
            .save("Team A keeps its launch date secret", "scout", 5, &[])
            .unwrap();
        assert_eq!(store_a.count().unwrap(), 1);
        let store_b = ceo_b.memory.as_ref().unwrap();
        assert_eq!(store_b.count().unwrap(), 0);
        assert!(store_b.recall("launch date secret", 5).unwrap().is_empty());
        assert_eq!(store_a.recall("launch date secret", 5).unwrap().len(), 1);
        assert!(dir
            .path()
            .join("tenants")
            .join("team-a")
            .join("memory.db")
            .is_file());
        assert!(dir
            .path()
            .join("tenants")
            .join("team-b")
            .join("ledger.db")
            .is_file());
        assert_eq!(
            ceo_a.run_store.as_ref().unwrap().root,
            dir.path().join("tenants").join("team-a").join("runs")
        );
        std::env::remove_var("BOTROPOLIS_DATA_DIR");
    }

    #[test]
    fn team_rejects_unknown_agent() {
        let ceo = test_ceo();
        let err = ceo
            .team(
                "do a thing",
                &["NoSuchAgent".to_string()],
                1,
                None,
                &[],
                false,
            )
            .unwrap_err();
        assert!(err.contains("Unknown agent"));
    }

    #[test]
    fn team_rejects_empty_team() {
        let ceo = test_ceo();
        assert!(ceo.team("do a thing", &[], 1, None, &[], false).is_err());
    }

    #[test]
    fn handle_with_scripted_client_runs() {
        // Swap every agent's client for a scripted one that answers
        // with a JSON summary, then handle a research question.
        let registry = Arc::new(AgentRegistry::load().unwrap());
        for agent in registry.list_all() {
            agent.set_client(Arc::new(ScriptedClient::new(vec![
                "{\"summary\": \"The abacus is old.\", \"details\": \"\", \"key_points\": []}"
                    .to_string(),
            ])));
        }
        let ceo = CEO::new(Some(registry), None, None).unwrap();
        let report = ceo
            .handle("research the abacus", None, None, &[], false)
            .unwrap();
        assert!(report.successful());
        assert_eq!(report.departments_involved, vec!["research"]);
        assert!(report.summary.contains("research"));
    }
}

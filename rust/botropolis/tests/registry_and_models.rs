//! Registry loading and model client tests for the Rust port.

use botropolis::agent::Agent;
use botropolis::models::{find_model_entry, infer_provider, load_model_registry, ModelClient, RealClient, ScriptedClient};
use botropolis::registry::AgentRegistry;
use botropolis::schemas::Task;
use std::sync::Arc;

fn expected_counts() -> Vec<(&'static str, usize)> {
    vec![
        ("research", 3),
        ("health", 3),
        ("finance", 2),
        ("code", 3),
        ("data", 2),
        ("legal", 1),
        ("marketing", 2),
        ("ops", 2),
        ("security", 1),
        ("support", 1),
    ]
}

#[test]
fn all_twenty_specs_load() {
    let registry = AgentRegistry::load().unwrap();
    assert_eq!(registry.len(), 20);
}

#[test]
fn departments_match() {
    let registry = AgentRegistry::load().unwrap();
    let departments = registry.departments();
    let expected: Vec<String> = vec![
        "code", "data", "finance", "health", "legal", "marketing", "ops", "research",
        "security", "support",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(departments, expected);
}

#[test]
fn department_headcounts() {
    let registry = AgentRegistry::load().unwrap();
    for (dept, count) in expected_counts() {
        assert_eq!(registry.by_department(dept).len(), count, "{dept}");
    }
}

#[test]
fn required_spec_fields_present() {
    let registry = AgentRegistry::load().unwrap();
    for agent in registry.list_all() {
        assert!(!agent.name.is_empty());
        assert!(!agent.title.is_empty());
        assert!(!agent.department.is_empty());
        assert!(!agent.specialty.is_empty());
        assert!(!agent.model.is_empty());
        assert!(agent.system_prompt.split_whitespace().count() > 30);
    }
}

#[test]
fn duplicate_agent_name_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let dept = dir.path().join("research");
    std::fs::create_dir(&dept).unwrap();
    let spec = "name: scout\ntitle: Scout\ndepartment: research\nspecialty: testing\nmodel: stub\nsystem_prompt: You are a test agent.\n";
    std::fs::write(dept.join("one.yaml"), spec).unwrap();
    std::fs::write(dept.join("two.yaml"), spec).unwrap();
    let err = AgentRegistry::load_from(dir.path(), None).unwrap_err();
    assert!(err.to_string().contains("Duplicate agent name"));
}

#[test]
fn stub_client_without_keys() {
    // No provider keys are set in the test environment, so every
    // known model id resolves to the stub.
    let client = RealClient::default();
    assert_eq!(client.provider_for("gpt-4o"), "stub");
    assert_eq!(client.provider_for("claude-sonnet-4-5"), "stub");
    assert_eq!(client.provider_for("gemini-2.5-pro"), "stub");
    assert_eq!(client.provider_for("unknown-model"), "stub");
    assert_eq!(client.provider_for("stub"), "stub");
}

#[test]
fn provider_inference_matches_python() {
    assert_eq!(infer_provider("gpt-4o"), "openai");
    assert_eq!(infer_provider("o1-mini"), "openai");
    assert_eq!(infer_provider("claude-sonnet-4-5"), "anthropic");
    assert_eq!(infer_provider("gemini-2.5-pro"), "google");
    assert_eq!(infer_provider("qwen3"), "stub");
}

#[test]
fn model_registry_file_loads() {
    let registry = load_model_registry(None);
    assert!(!registry.base_models.is_empty());
    let entry = find_model_entry("gpt-4o", &registry).expect("gpt-4o in registry");
    assert_eq!(entry.provider, "openai");
    let claude = find_model_entry("claude-sonnet-4-5", &registry).expect("claude in registry");
    assert_eq!(claude.provider, "anthropic");
    assert!(find_model_entry("no-such-model", &registry).is_none());
}

#[test]
fn stub_chat_labels_itself_and_restates_the_task() {
    let client = RealClient::default();
    let messages = vec![botropolis::models::ChatMessage::new(
        "user",
        "What is 6 times 7?",
    )];
    let first = client.chat("stub", &messages).unwrap();
    let second = client.chat("stub", &messages).unwrap();
    assert_eq!(first, second, "stub is deterministic");
    assert!(first.contains("[offline stub"));
    assert!(first.contains("What is 6 times 7?"));
    assert!(first.contains("Model requested: stub"));
}

#[test]
fn single_shot_agent_run_with_stub() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.yaml");
    std::fs::write(
        &path,
        "name: TestAgent\ntitle: Tester\ndepartment: research\nspecialty: testing\nmodel: stub\nsystem_prompt: You are a test agent.\ntoolkit: []\n",
    )
    .unwrap();
    let agent = Agent::load(&path, None).unwrap();
    let result = agent.run(Task::new("Explain the tides."), Default::default());
    assert!(result.success);
    assert!(result.output.contains("Explain the tides."));
    assert_eq!(result.confidence, 0.5);
    assert_eq!(
        result.metadata.get("provider"),
        Some(&serde_json::Value::String("stub".to_string()))
    );
    assert!(result.structured.get("summary").is_some());
}

#[test]
fn scripted_client_plays_replies_in_order() {
    let client: Arc<dyn ModelClient> =
        Arc::new(ScriptedClient::new(vec!["first".to_string(), "second".to_string()]));
    let messages = vec![botropolis::models::ChatMessage::new("user", "hi")];
    assert_eq!(client.chat("stub", &messages).unwrap(), "first");
    assert_eq!(client.chat("stub", &messages).unwrap(), "second");
    assert_eq!(client.chat("stub", &messages).unwrap(), "second");
    assert_eq!(client.provider_for("stub"), "scripted");
}

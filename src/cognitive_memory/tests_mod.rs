use super::*;
use super::*;

fn test_mem() -> NativeCognitiveMemory {
    NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
}

#[test]
fn open_in_memory_creates_schema() {
    let mem = test_mem();
    let stats = mem.get_statistics().unwrap();
    assert_eq!(stats.total(), 0);
}

#[test]
fn store_and_search_fact() {
    let mem = test_mem();
    let id = mem
        .store_fact("rust", "systems language", 0.9, &[], "test")
        .unwrap();
    assert!(id.starts_with("sem_"));

    let facts = mem.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert!((facts[0].confidence - 0.9).abs() < f64::EPSILON);
}

#[test]
fn search_facts_respects_min_confidence() {
    let mem = test_mem();
    mem.store_fact("low", "low confidence", 0.1, &[], "test")
        .unwrap();
    mem.store_fact("high", "high confidence", 0.9, &[], "test")
        .unwrap();

    let results = mem.search_facts("confidence", 10, 0.5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].concept, "high");
}

#[test]
fn record_and_prune_sensory() {
    let mem = test_mem();
    mem.record_sensory("test", "data", 0).unwrap(); // expires immediately
    let pruned = mem.prune_expired_sensory().unwrap();
    assert!(pruned >= 1);
}

#[test]
fn push_get_clear_working() {
    let mem = test_mem();
    mem.push_working("goal", "build it", "task-1", 1.0).unwrap();
    mem.push_working("context", "extra", "task-1", 0.5).unwrap();

    let slots = mem.get_working("task-1").unwrap();
    assert_eq!(slots.len(), 2);

    let cleared = mem.clear_working("task-1").unwrap();
    assert_eq!(cleared, 2);
    assert!(mem.get_working("task-1").unwrap().is_empty());
}

#[test]
fn store_episode_and_consolidate() {
    let mem = test_mem();
    for i in 0..5 {
        mem.store_episode(&format!("event {i}"), "test", None)
            .unwrap();
    }
    let consolidated = mem.consolidate_episodes(5).unwrap();
    assert!(consolidated.is_some());
    let stats = mem.get_statistics().unwrap();
    // 5 original (now compressed=1) + 1 summary = 6
    assert_eq!(stats.episodic_count, 6);
}

#[test]
fn consolidate_episodes_returns_none_when_insufficient() {
    let mem = test_mem();
    mem.store_episode("only one", "test", None).unwrap();
    assert!(mem.consolidate_episodes(5).unwrap().is_none());
}

#[test]
fn store_and_recall_procedure() {
    let mem = test_mem();
    let steps = vec!["compile".to_string(), "test".to_string()];
    mem.store_procedure("build", &steps, &[]).unwrap();

    let procs = mem.recall_procedure("build", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "build");
    assert_eq!(procs[0].steps, steps);
}

#[test]
fn store_prospective_and_check_triggers() {
    let mem = test_mem();
    mem.store_prospective("watch errors", "error", "alert", 5)
        .unwrap();
    let triggered = mem.check_triggers("an error occurred").unwrap();
    assert_eq!(triggered.len(), 1);
    assert_eq!(triggered[0].description, "watch errors");
}

#[test]
fn check_triggers_ignores_non_matching() {
    let mem = test_mem();
    mem.store_prospective("watch errors", "error", "alert", 5)
        .unwrap();
    let triggered = mem.check_triggers("all good").unwrap();
    assert!(triggered.is_empty());
}

#[test]
fn get_statistics_counts_all_types() {
    let mem = test_mem();
    mem.record_sensory("vis", "img", 300).unwrap();
    mem.push_working("ctx", "data", "t1", 1.0).unwrap();
    mem.store_episode("event", "src", None).unwrap();
    mem.store_fact("f", "fact", 0.5, &[], "").unwrap();
    mem.store_procedure("p", &[], &[]).unwrap();
    mem.store_prospective("desc", "trigger", "action", 1)
        .unwrap();
    let stats = mem.get_statistics().unwrap();
    assert_eq!(stats.total(), 6);
}

#[test]
fn cypher_injection_escaped() {
    let mem = test_mem();
    let result = mem.store_fact("test'DROP", "con'tent", 0.5, &[], "src");
    assert!(result.is_ok(), "single quotes should be escaped");
}

#[test]
fn escape_cypher_handles_special_chars() {
    assert_eq!(escape_cypher("a'b"), "a\\'b");
    assert_eq!(escape_cypher("a\\b"), "a\\\\b");
    assert_eq!(escape_cypher("line\nbreak"), "line\\nbreak");
    assert_eq!(escape_cypher("tab\there"), "tab\\there");
    assert_eq!(escape_cypher("null\0byte"), "null\\0byte");
    assert_eq!(escape_cypher("cr\rreturn"), "cr\\rreturn");
}

#[test]
fn newline_in_content_does_not_break_query() {
    let mem = test_mem();
    let result = mem.store_fact("key", "line1\nline2\ttab", 0.5, &[], "src");
    assert!(result.is_ok(), "newlines and tabs should be safely escaped");
    let facts = mem.search_facts("key", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
}

#[test]
fn disk_persist_facts_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        mem.store_fact("rust", "systems language", 0.95, &[], "test")
            .unwrap();
    } // drop closes the DB

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    let facts = mem2.search_facts("rust", 10, 0.0).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].concept, "rust");
    assert_eq!(facts[0].content, "systems language");
    assert!((facts[0].confidence - 0.95).abs() < f64::EPSILON);
}

#[test]
fn disk_persist_procedures_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        let steps = vec![
            "compile".to_string(),
            "test".to_string(),
            "deploy".to_string(),
        ];
        mem.store_procedure("release", &steps, &[]).unwrap();
    }

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    let procs = mem2.recall_procedure("release", 5).unwrap();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0].name, "release");
    assert_eq!(
        procs[0].steps,
        vec![
            "compile".to_string(),
            "test".to_string(),
            "deploy".to_string()
        ]
    );
}

#[test]
fn disk_persist_episodes_and_consolidation_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_path_buf();

    {
        let mem = NativeCognitiveMemory::open(&path).unwrap();
        for i in 0..5 {
            mem.store_episode(&format!("event {i}"), "test", None)
                .unwrap();
        }
        let consolidated = mem.consolidate_episodes(5).unwrap();
        assert!(consolidated.is_some());
    }

    let mem2 = NativeCognitiveMemory::open(&path).unwrap();
    // Query for the consolidated episode (compressed=1 with source_label='consolidation')
    let rows = mem2
        .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
        .unwrap();
    assert_eq!(rows.len(), 1, "consolidated episode should survive reopen");
    let content = super::as_str(&rows[0][0]).unwrap();
    assert!(
        content.starts_with("[consolidated 5"),
        "consolidated content should start with marker, got: {content}"
    );
}

#[test]
fn consolidate_episodes_deduplicates() {
    let mem = test_mem();
    // Store duplicate episodes
    mem.store_episode("duplicate event", "test", None).unwrap();
    mem.store_episode("duplicate event", "test", None).unwrap();
    mem.store_episode("  duplicate event  ", "test", None)
        .unwrap();
    mem.store_episode("unique event", "test", None).unwrap();

    let consolidated = mem.consolidate_episodes(10).unwrap();
    assert!(consolidated.is_some());

    let rows = mem
        .query("MATCH (e:Episode) WHERE e.compressed = 1 AND e.source_label = 'consolidation' RETURN e.content")
        .unwrap();
    assert_eq!(rows.len(), 1);
    let content = super::as_str(&rows[0][0]).unwrap();
    // 4 original → 2 unique
    assert!(
        content.contains("4→2"),
        "should show dedup ratio, got: {content}"
    );
}

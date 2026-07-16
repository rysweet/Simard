//! Prep scheduling: turn a set of dishes (with prep/cook durations and
//! prerequisite dependencies) into a backward-planned timeline anchored so that
//! every dish is ready by the event's service time.
//!
//! The model assumes parallel stations (each dish can be worked independently),
//! so a task's only timing constraint is that any recipe it `depends_on` must
//! *finish* before it *starts*. This yields a latest-start ("just in time")
//! schedule and a single `kitchen_start` — the earliest any task must begin.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::book::KitchenBook;
use super::types::{ClockTime, GastronomeError, GastronomeResult};

/// One scheduled prep task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepTask {
    /// Recipe id.
    pub recipe: String,
    /// Recipe name.
    pub name: String,
    /// When to start (latest start that still hits every deadline).
    pub start: String,
    /// When the task finishes.
    pub end: String,
    /// Total minutes (prep + cook).
    pub duration_minutes: u32,
    /// Raw latest-start minutes since midnight (may be negative for the day
    /// before). Exposed for deterministic sorting/testing.
    pub start_minutes: i32,
}

/// A complete prep schedule for an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepSchedule {
    /// The service deadline everything is anchored to.
    pub service_time: String,
    /// The earliest a task must begin (the kitchen "call time").
    pub kitchen_start: String,
    /// Tasks ordered by start time (earliest first), then recipe id.
    pub tasks: Vec<PrepTask>,
}

/// Build a backward-planned schedule for `recipe_ids` (transitive prerequisites
/// are pulled in automatically) so all dishes are ready by `service_time`.
///
/// # Errors
/// Returns [`GastronomeError::ScheduleCycle`] if the dependency graph has a
/// cycle, or [`GastronomeError::UnknownRecipe`] if a referenced recipe (or
/// dependency) is missing from the book.
pub fn schedule(
    book: &KitchenBook,
    recipe_ids: &[String],
    service_time: ClockTime,
) -> GastronomeResult<PrepSchedule> {
    // 1. Expand to the full set of recipes to prepare (menu + transitive deps).
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = recipe_ids.to_vec();
    while let Some(id) = stack.pop() {
        if !nodes.insert(id.clone()) {
            continue;
        }
        let recipe = book.recipe(&id)?;
        for dep in &recipe.depends_on {
            stack.push(dep.clone());
        }
    }

    // 2. Kahn's algorithm: topological order with deps *before* dependents.
    //    Edge d -> r means "d must finish before r starts".
    let mut successors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut indegree: BTreeMap<String, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    for id in &nodes {
        let recipe = book.recipe(id)?;
        for dep in &recipe.depends_on {
            successors.entry(dep.clone()).or_default().push(id.clone());
            *indegree.get_mut(id).expect("node in set") += 1;
        }
    }
    let mut queue: VecDeque<String> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    // BTreeMap iteration is sorted, so the queue seed is deterministic.
    let mut topo: Vec<String> = Vec::with_capacity(nodes.len());
    while let Some(n) = queue.pop_front() {
        topo.push(n.clone());
        if let Some(succs) = successors.get(&n) {
            for s in succs {
                let d = indegree.get_mut(s).expect("successor tracked");
                *d -= 1;
                if *d == 0 {
                    queue.push_back(s.clone());
                }
            }
        }
    }
    if topo.len() != nodes.len() {
        // Some node never reached indegree 0 → it sits on a cycle.
        let culprit = nodes
            .iter()
            .find(|n| !topo.contains(*n))
            .cloned()
            .unwrap_or_default();
        return Err(GastronomeError::ScheduleCycle { recipe: culprit });
    }

    // 3. Backward pass: latest_finish starts at the service deadline for every
    //    dish; processing successors first tightens each prerequisite's finish.
    let mut latest_finish: BTreeMap<String, i32> = nodes
        .iter()
        .map(|n| (n.clone(), service_time.minutes()))
        .collect();
    for id in topo.iter().rev() {
        let recipe = book.recipe(id)?;
        let start =
            latest_finish[id] - i32::try_from(recipe.duration_minutes()).unwrap_or(i32::MAX);
        for dep in &recipe.depends_on {
            let slot = latest_finish.get_mut(dep).expect("dep tracked");
            if start < *slot {
                *slot = start;
            }
        }
    }

    // 4. Materialise tasks.
    let mut tasks: Vec<PrepTask> = Vec::with_capacity(nodes.len());
    for id in &nodes {
        let recipe = book.recipe(id)?;
        let end_min = latest_finish[id];
        let duration = recipe.duration_minutes();
        let start_min = end_min - i32::try_from(duration).unwrap_or(i32::MAX);
        tasks.push(PrepTask {
            recipe: recipe.id.clone(),
            name: recipe.name.clone(),
            start: ClockTime::from_minutes(start_min).to_string(),
            end: ClockTime::from_minutes(end_min).to_string(),
            duration_minutes: duration,
            start_minutes: start_min,
        });
    }
    tasks.sort_by(|a, b| {
        a.start_minutes
            .cmp(&b.start_minutes)
            .then_with(|| a.recipe.cmp(&b.recipe))
    });

    let kitchen_start_min = tasks
        .iter()
        .map(|t| t.start_minutes)
        .min()
        .unwrap_or_else(|| service_time.minutes());

    Ok(PrepSchedule {
        service_time: service_time.to_string(),
        kitchen_start: ClockTime::from_minutes(kitchen_start_min).to_string(),
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_finishes_before_dependent_starts() {
        let book = KitchenBook::demo();
        let svc = ClockTime::parse("18:30").unwrap();
        let sched = schedule(&book, &["focaccia".into()], svc).unwrap();
        // poolish is pulled in transitively via focaccia.depends_on.
        let poolish = sched.tasks.iter().find(|t| t.recipe == "poolish").unwrap();
        let focaccia = sched.tasks.iter().find(|t| t.recipe == "focaccia").unwrap();
        // focaccia must finish by 18:30; its duration is 180 min → start 15:30.
        assert_eq!(focaccia.end, "18:30");
        // poolish (730 min) must finish before focaccia starts (15:30).
        let focaccia_start = focaccia.start_minutes;
        let poolish_end_min = focaccia_start; // tightened to dependent's start
        let poolish_start = poolish.start_minutes;
        assert!(
            poolish_start
                + i32::try_from(book.recipe("poolish").unwrap().duration_minutes()).unwrap()
                <= poolish_end_min + 1
        );
        assert!(poolish_start < focaccia_start);
    }

    #[test]
    fn all_terminal_dishes_ready_by_service() {
        let book = KitchenBook::demo();
        let svc = ClockTime::parse("18:30").unwrap();
        let ids: Vec<String> = vec!["roast_chicken".into(), "green_beans".into()];
        let sched = schedule(&book, &ids, svc).unwrap();
        for t in &sched.tasks {
            assert!(t.start_minutes + i32::try_from(t.duration_minutes).unwrap() <= svc.minutes());
        }
    }

    #[test]
    fn kitchen_start_is_earliest_task() {
        let book = KitchenBook::demo();
        let svc = ClockTime::parse("18:30").unwrap();
        let sched = schedule(&book, &["focaccia".into()], svc).unwrap();
        let min_start = sched.tasks.iter().map(|t| t.start_minutes).min().unwrap();
        assert_eq!(
            sched.kitchen_start,
            ClockTime::from_minutes(min_start).to_string()
        );
    }

    #[test]
    fn tasks_sorted_by_start_time() {
        let book = KitchenBook::demo();
        let svc = ClockTime::parse("18:30").unwrap();
        let ids: Vec<String> = book
            .brief
            .clone()
            .unwrap()
            .courses
            .iter()
            .map(|c| c.recipe.clone())
            .collect();
        let sched = schedule(&book, &ids, svc).unwrap();
        let starts: Vec<i32> = sched.tasks.iter().map(|t| t.start_minutes).collect();
        let mut sorted = starts.clone();
        sorted.sort_unstable();
        assert_eq!(starts, sorted);
    }

    #[test]
    fn cycle_is_detected() {
        use super::super::types::Recipe;
        let recipes = vec![
            Recipe {
                id: "a".into(),
                name: "A".into(),
                servings: 1.0,
                prep_minutes: 5,
                cook_minutes: 0,
                depends_on: vec!["b".into()],
                ingredients: vec![],
            },
            Recipe {
                id: "b".into(),
                name: "B".into(),
                servings: 1.0,
                prep_minutes: 5,
                cook_minutes: 0,
                depends_on: vec!["a".into()],
                ingredients: vec![],
            },
        ];
        let book = KitchenBook::new(vec![], recipes, None).unwrap();
        let svc = ClockTime::parse("12:00").unwrap();
        let err = schedule(&book, &["a".into()], svc).unwrap_err();
        assert!(matches!(err, GastronomeError::ScheduleCycle { .. }));
    }

    #[test]
    fn long_bake_reaching_previous_day_renders_offset() {
        // A dish longer than the time-of-day forces a previous-day start.
        use super::super::types::Recipe;
        let recipes = vec![Recipe {
            id: "brisket".into(),
            name: "Smoked brisket".into(),
            servings: 10.0,
            prep_minutes: 60,
            cook_minutes: 900, // 15h
            depends_on: vec![],
            ingredients: vec![],
        }];
        let book = KitchenBook::new(vec![], recipes, None).unwrap();
        let svc = ClockTime::parse("12:00").unwrap();
        let sched = schedule(&book, &["brisket".into()], svc).unwrap();
        // 12:00 minus 16h = -4:00 → previous day.
        assert!(sched.kitchen_start.contains("-1d"));
    }
}

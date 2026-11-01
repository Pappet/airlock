//! File overlap and dependency stages of open tasks (dashboard views, Spec 04
//! §3.5–3.6; claim-time warnings, Spec 01 §2.1).
//!
//! Two open tasks overlap when an entry of one `touch` list covers an entry of
//! the other: the same path, or a directory and a path beneath it
//! (`src/render/` covers `src/render/mesh.rs`). A task without `touch` data has
//! unknown overlap and is reported as such, never as clean.

use crate::core::{Task, TaskState, TaskStatus};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Overlap and stage analysis of all open (not done) tasks.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Analysis {
    /// Open tasks, sorted by id
    pub tasks: Vec<TaskAnalysis>,
    /// Paths touched by two or more open tasks, most shared first
    pub files: Vec<FileUse>,
    /// Open tasks without `touch` data
    pub no_touch: Vec<String>,
}

impl Analysis {
    /// Overlaps of task `id` with occupying tasks: shared files whose work is
    /// not merged yet (claim-time warning, Spec 01 §2.1).
    pub fn conflicts(&self, id: &str) -> Vec<Overlap> {
        self.tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.overlaps.iter().filter(|o| o.occupied).cloned().collect())
            .unwrap_or_default()
    }
}

/// Overlaps in one line: "src/a.rs, src/b.rs with E-1; src/c.rs with E-3".
pub fn describe(overlaps: &[Overlap]) -> String {
    overlaps
        .iter()
        .map(|o| format!("{} with {}", o.paths.join(", "), o.id))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Overlap and stage of one open task.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TaskAnalysis {
    pub id: String,
    pub status: TaskStatus,
    /// 1 for tasks without open prerequisites, else 1 + the deepest open prerequisite
    pub stage: u32,
    /// The task has no `touch` data, so its overlap is unknown
    pub unknown: bool,
    /// Other open tasks sharing at least one path (by coverage), sorted by id
    pub overlaps: Vec<Overlap>,
}

/// Another open task that touches some of the same paths.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Overlap {
    pub id: String,
    /// The other task is in progress or in review, so the paths are occupied
    pub occupied: bool,
    /// Shared paths, sorted: for a directory and a path beneath it, the more
    /// specific one
    pub paths: Vec<String>,
}

/// A path touched by several open tasks.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileUse {
    pub path: String,
    /// Open tasks touching the path, sorted by id
    pub tasks: Vec<String>,
    /// The subset of `tasks` that is in progress or in review
    pub occupied_by: Vec<String>,
}

/// Normalizes a `touch` path for comparison: `\` becomes `/`, leading `./` and
/// repeated `/` are removed. A trailing `/` is kept; it only marks intent, since
/// `src/render` and `src/render/` cover the same paths (see [`covers`]).
pub fn normalize_path(path: &str) -> String {
    let mut path = path.trim().replace('\\', "/");
    while path.contains("//") {
        path = path.replace("//", "/");
    }
    while let Some(rest) = path.strip_prefix("./") {
        path = rest.to_string();
    }
    path
}

/// `path` without a trailing `/`.
fn stem(path: &str) -> &str {
    path.strip_suffix('/').unwrap_or(path)
}

/// Whether `touch` entry `a` covers path `b` (both normalized): `a` and `b` name
/// the same path, or `b` lies beneath directory `a`. `src/rend` does not cover
/// `src/render/a.rs`; a file covers only itself, as it has no children.
pub fn covers(a: &str, b: &str) -> bool {
    let (a, b) = (stem(a), stem(b));
    a == b || (b.len() > a.len() && b.starts_with(a) && b.as_bytes()[a.len()] == b'/')
}

/// The path to report when `a` covers `b`: the more specific `b`, or for the
/// same path written with and without a trailing `/`, the directory form.
fn covered(a: &str, b: &str) -> String {
    if stem(a) == stem(b) && (a.ends_with('/') || b.ends_with('/')) {
        format!("{}/", stem(b))
    } else {
        b.to_string()
    }
}

/// Paths shared by two `touch` sets: for every covering pair, the covered path.
fn shared(a: &BTreeSet<String>, b: &BTreeSet<String>) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for p in a {
        for q in b {
            if covers(p, q) {
                paths.insert(covered(p, q));
            } else if covers(q, p) {
                paths.insert(covered(q, p));
            }
        }
    }
    paths.into_iter().collect()
}

/// Whether a task in this status holds its files: its work is not merged yet.
pub fn is_occupying(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::InProgress | TaskStatus::InReview)
}

/// Analyzes the open tasks among `tasks`.
pub fn analyze(tasks: &[Task]) -> Analysis {
    let done: HashSet<&str> = tasks
        .iter()
        .filter(|t| t.state == TaskState::Done)
        .map(|t| t.id.as_str())
        .collect();
    let mut open: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.state != TaskState::Done)
        .collect();
    open.sort_by(|a, b| a.id.cmp(&b.id));
    let status: HashMap<&str, TaskStatus> = open
        .iter()
        .map(|t| (t.id.as_str(), t.status(|id| done.contains(id))))
        .collect();
    let touch: Vec<BTreeSet<String>> = open
        .iter()
        .map(|t| {
            t.touch
                .iter()
                .map(|p| normalize_path(p))
                .filter(|p| !p.is_empty())
                .collect()
        })
        .collect();

    let by_id: HashMap<&str, &Task> = open.iter().map(|t| (t.id.as_str(), *t)).collect();
    let mut stages = HashMap::new();
    let mut entries = Vec::with_capacity(open.len());
    for (i, task) in open.iter().enumerate() {
        let overlaps = open
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .filter_map(|(j, other)| {
                let paths = shared(&touch[i], &touch[j]);
                (!paths.is_empty()).then(|| Overlap {
                    id: other.id.clone(),
                    occupied: is_occupying(status[other.id.as_str()]),
                    paths,
                })
            })
            .collect();
        entries.push(TaskAnalysis {
            id: task.id.clone(),
            status: status[task.id.as_str()],
            stage: stage(&task.id, &by_id, &mut stages, &mut HashSet::new()),
            unknown: touch[i].is_empty(),
            overlaps,
        });
    }

    // Every named path is a candidate row (one per path, the directory form when
    // it is written both ways); its users are the tasks with an entry covering it.
    let mut named: BTreeMap<&str, bool> = BTreeMap::new();
    for path in touch.iter().flatten() {
        *named.entry(stem(path)).or_default() |= path.ends_with('/');
    }
    let mut files: Vec<FileUse> = named
        .into_iter()
        .map(|(stem, dir)| {
            let path = if dir {
                format!("{stem}/")
            } else {
                stem.to_string()
            };
            let ids: Vec<&str> = open
                .iter()
                .zip(&touch)
                .filter(|(_, entries)| entries.iter().any(|e| covers(e, &path)))
                .map(|(task, _)| task.id.as_str())
                .collect();
            (path, ids)
        })
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(path, ids)| FileUse {
            path,
            occupied_by: ids
                .iter()
                .filter(|id| is_occupying(status[**id]))
                .map(|id| id.to_string())
                .collect(),
            tasks: ids.iter().map(|id| id.to_string()).collect(),
        })
        .collect();
    // Stable sort keeps the path order among equally shared files.
    files.sort_by_key(|f| std::cmp::Reverse(f.tasks.len()));

    let no_touch = entries
        .iter()
        .filter(|e| e.unknown)
        .map(|e| e.id.clone())
        .collect();
    Analysis {
        tasks: entries,
        files,
        no_touch,
    }
}

/// Dependency stage of an open task; prerequisites that are done (absent from
/// `open`) are ignored, and a cycle counts as no further prerequisite.
fn stage<'a>(
    id: &'a str,
    open: &HashMap<&'a str, &'a Task>,
    memo: &mut HashMap<&'a str, u32>,
    visiting: &mut HashSet<&'a str>,
) -> u32 {
    if let Some(&stage) = memo.get(id) {
        return stage;
    }
    if !visiting.insert(id) {
        return 0;
    }
    let deepest = open[id]
        .blocked_by
        .iter()
        .filter_map(|p| open.get_key_value(p.as_str()).map(|(k, _)| *k))
        .map(|p| stage(p, open, memo, visiting))
        .max()
        .unwrap_or(0);
    visiting.remove(id);
    memo.insert(id, deepest + 1);
    deepest + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Priority, TaskState};

    fn task(id: &str, state: TaskState, blocked_by: &[&str], touch: &[&str]) -> Task {
        Task {
            id: id.to_string(),
            title: format!("Task {id}"),
            role: "engine".to_string(),
            state,
            priority: Priority::Normal,
            track: None,
            blocked_by: blocked_by.iter().map(|s| s.to_string()).collect(),
            read: Vec::new(),
            touch: touch.iter().map(|s| s.to_string()).collect(),
            r#do: String::new(),
            verify: String::new(),
            notes: None,
            claimed_by: None,
            lease_expires_at: None,
            progress: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn entry<'a>(analysis: &'a Analysis, id: &str) -> &'a TaskAnalysis {
        analysis.tasks.iter().find(|t| t.id == id).unwrap()
    }

    #[test]
    fn normalizes_paths() {
        for raw in [
            "./src/a.rs",
            "src//a.rs",
            "src\\a.rs",
            " src/a.rs ",
            "././src/a.rs",
        ] {
            assert_eq!(normalize_path(raw), "src/a.rs", "{raw}");
        }
        assert_ne!(normalize_path("src/"), normalize_path("src/a.rs"));
        assert_eq!(
            normalize_path("./src/render/"),
            "src/render/",
            "the slash stays"
        );
    }

    fn paths(a: &Analysis, id: &str, other: &str) -> Vec<String> {
        entry(a, id)
            .overlaps
            .iter()
            .find(|o| o.id == other)
            .map(|o| o.paths.clone())
            .unwrap_or_default()
    }

    #[test]
    fn directory_entries_cover_files_beneath() {
        let a = analyze(&[
            task("D", TaskState::InProgress, &[], &["src/render/"]),
            task("F", TaskState::Open, &[], &["src/render/mesh.rs"]),
        ]);
        assert_eq!(paths(&a, "D", "F"), ["src/render/mesh.rs"]);
        assert_eq!(paths(&a, "F", "D"), ["src/render/mesh.rs"]);
        assert_eq!(describe(&a.conflicts("F")), "src/render/mesh.rs with D");
        let rows: Vec<&str> = a.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            rows,
            ["src/render/mesh.rs"],
            "the directory itself is no row"
        );
        assert_eq!(a.files[0].tasks, ["D", "F"]);
        assert_eq!(a.files[0].occupied_by, ["D"]);
    }

    #[test]
    fn trailing_slash_is_optional() {
        let a = analyze(&[
            task("D", TaskState::Open, &[], &["src/render"]),
            task("F", TaskState::Open, &[], &["src/render/mesh.rs"]),
            task("E", TaskState::Open, &[], &["src/render/"]),
        ]);
        assert_eq!(paths(&a, "D", "F"), ["src/render/mesh.rs"]);
        assert_eq!(paths(&a, "D", "E"), ["src/render/"]);
        assert_eq!(paths(&a, "E", "D"), ["src/render/"]);
        let rows: Vec<&str> = a.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(rows, ["src/render/mesh.rs", "src/render/"]);
        assert_eq!(a.files[0].tasks, ["D", "E", "F"]);
        assert_eq!(
            a.files[1].tasks,
            ["D", "E"],
            "two tasks name the same directory"
        );
    }

    #[test]
    fn sibling_files_do_not_overlap_by_directory() {
        let a = analyze(&[
            task("A", TaskState::Open, &[], &["src/render/a.rs"]),
            task("B", TaskState::Open, &[], &["src/render/b.rs"]),
        ]);
        assert!(entry(&a, "A").overlaps.is_empty());
        assert!(a.files.is_empty());
    }

    #[test]
    fn prefix_without_separator_is_not_coverage() {
        assert!(!covers("src/rend", "src/render/a.rs"));
        assert!(!covers("src/render/a.rs", "src/render"));
        assert!(covers("src/render", "src/render/a.rs"));
        assert!(covers("src/a.rs", "src/a.rs"));
        let a = analyze(&[
            task("A", TaskState::Open, &[], &["src/rend"]),
            task("B", TaskState::Open, &[], &["src/render/a.rs"]),
        ]);
        assert!(entry(&a, "A").overlaps.is_empty());
    }

    #[test]
    fn nested_directories_overlap() {
        let a = analyze(&[
            task("A", TaskState::Open, &[], &["src/"]),
            task("B", TaskState::Open, &[], &["src/render/"]),
            task(
                "C",
                TaskState::Open,
                &[],
                &["src/render/mesh.rs", "docs/x.md"],
            ),
        ]);
        assert_eq!(paths(&a, "A", "B"), ["src/render/"]);
        assert_eq!(paths(&a, "A", "C"), ["src/render/mesh.rs"]);
        assert_eq!(paths(&a, "B", "C"), ["src/render/mesh.rs"]);
        let rows: Vec<(&str, usize)> = a
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.tasks.len()))
            .collect();
        assert_eq!(rows, [("src/render/mesh.rs", 3), ("src/render/", 2)]);
    }

    #[test]
    fn done_tasks_never_overlap() {
        let a = analyze(&[
            task("A", TaskState::Done, &[], &["src/sim.rs"]),
            task("B", TaskState::Open, &[], &["src/sim.rs"]),
        ]);
        assert_eq!(a.tasks.len(), 1);
        assert!(entry(&a, "B").overlaps.is_empty());
        assert!(a.files.is_empty());
    }

    #[test]
    fn shared_path_is_symmetric_overlap() {
        let a = analyze(&[
            task("A", TaskState::Open, &[], &["src/sim.rs", "src/a.rs"]),
            task("B", TaskState::Open, &[], &["./src/sim.rs", "src/b.rs"]),
            task("C", TaskState::Open, &[], &["src/c.rs"]),
        ]);
        let expect = |id: &str| {
            vec![Overlap {
                id: id.to_string(),
                occupied: false,
                paths: vec!["src/sim.rs".to_string()],
            }]
        };
        assert_eq!(entry(&a, "A").overlaps, expect("B"));
        assert_eq!(entry(&a, "B").overlaps, expect("A"));
        assert!(entry(&a, "C").overlaps.is_empty());
        assert!(!entry(&a, "C").unknown);
    }

    #[test]
    fn running_task_marks_overlap_occupied() {
        let a = analyze(&[
            task("A", TaskState::InProgress, &[], &["src/sim.rs"]),
            task("B", TaskState::Open, &[], &["src/sim.rs"]),
            task("R", TaskState::InReview, &[], &["src/sim.rs"]),
        ]);
        let b = entry(&a, "B");
        assert!(b.overlaps.iter().all(|o| o.occupied));
        assert_eq!(b.overlaps.len(), 2);
        assert!(
            entry(&a, "A")
                .overlaps
                .iter()
                .any(|o| o.id == "R" && o.occupied)
        );
        assert!(
            entry(&a, "A")
                .overlaps
                .iter()
                .any(|o| o.id == "B" && !o.occupied)
        );
        assert_eq!(a.files[0].occupied_by, ["A", "R"]);
        assert!(is_occupying(TaskStatus::InProgress));
        assert!(is_occupying(TaskStatus::InReview));
        assert!(!is_occupying(TaskStatus::Ready));
        assert!(!is_occupying(TaskStatus::Blocked));
    }

    #[test]
    fn conflicts_are_overlaps_with_occupying_tasks_only() {
        let a = analyze(&[
            task("A", TaskState::InProgress, &[], &["src/sim.rs"]),
            task("B", TaskState::Open, &[], &["src/sim.rs", "src/b.rs"]),
            task("C", TaskState::Open, &[], &["src/b.rs"]),
        ]);
        assert_eq!(
            a.conflicts("B"),
            [Overlap {
                id: "A".to_string(),
                occupied: true,
                paths: vec!["src/sim.rs".to_string()],
            }],
            "C only overlaps, it does not occupy"
        );
        assert!(a.conflicts("C").is_empty());
        assert!(a.conflicts("nope").is_empty());
        assert_eq!(describe(&a.conflicts("B")), "src/sim.rs with A");
    }

    #[test]
    fn empty_touch_is_unknown_not_clean() {
        let a = analyze(&[
            task("A", TaskState::Open, &[], &[]),
            task("B", TaskState::Open, &[], &["src/sim.rs"]),
        ]);
        assert!(entry(&a, "A").unknown);
        assert!(entry(&a, "A").overlaps.is_empty());
        assert!(!entry(&a, "B").unknown);
        assert_eq!(a.no_touch, ["A"]);
    }

    #[test]
    fn stages_ignore_done_prerequisites() {
        let a = analyze(&[
            task("A", TaskState::Done, &[], &[]),
            task("B", TaskState::Open, &["A"], &[]),
            task("C", TaskState::Open, &["B"], &[]),
            task("D", TaskState::Open, &["B", "C"], &[]),
            task("E", TaskState::InProgress, &[], &[]),
        ]);
        assert_eq!(entry(&a, "B").stage, 1);
        assert_eq!(entry(&a, "C").stage, 2);
        assert_eq!(entry(&a, "D").stage, 3);
        assert_eq!(entry(&a, "E").stage, 1);
        assert_eq!(entry(&a, "C").status, TaskStatus::Blocked);
    }

    #[test]
    fn stages_survive_cycles() {
        let a = analyze(&[
            task("A", TaskState::Open, &["B"], &[]),
            task("B", TaskState::Open, &["A"], &[]),
        ]);
        assert!(entry(&a, "A").stage >= 1 && entry(&a, "B").stage >= 1);
    }

    #[test]
    fn files_sorted_by_count_then_path() {
        let a = analyze(&[
            task("A", TaskState::Open, &[], &["z.rs", "y.rs", "x.rs"]),
            task("B", TaskState::Open, &[], &["z.rs", "y.rs", "x.rs"]),
            task("C", TaskState::Open, &[], &["y.rs"]),
            task("D", TaskState::Open, &[], &["only.rs"]),
        ]);
        let paths: Vec<&str> = a.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["y.rs", "x.rs", "z.rs"]);
        assert_eq!(a.files[0].tasks, ["A", "B", "C"]);
        assert!(a.files[0].occupied_by.is_empty());
    }
}

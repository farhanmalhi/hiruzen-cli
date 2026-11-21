//! Learns from which suggestion the user takes.
//!
//! Each time a suggestion is accepted, it is noted as picked, and the ones
//! that were listed above it are noted as passed over. A suggestion that
//! keeps being picked in a given place rises; one that is always shown first
//! and never taken sinks.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::engine::Candidate;

/// Most a habit of picking a suggestion can add to its priority.
const PICK_BOOST: f64 = 30.0;
/// Most that being passed over, and never picked, can take away.
const PASS_PENALTY: f64 = 20.0;
/// Passes before a never-picked suggestion starts to sink.
const PATIENCE: f64 = 3.0;

#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Tally {
    picked: f64,
    passed: f64,
}

#[derive(Debug, Default)]
pub struct Picks {
    /// Place (project and the words before the cursor) to suggestion to tally.
    places: HashMap<String, HashMap<String, Tally>>,
    file: Option<PathBuf>,
}

/// Identifies where a suggestion was shown: the project and the words
/// already typed. A tab cannot occur in either.
pub fn place(project: &str, context: &[&str]) -> String {
    let words = &context[..context.len().min(4)];
    format!("{project}\t{}", words.join(" "))
}

impl Picks {
    /// Reads the record: `project<TAB>context<TAB>picked<TAB>passed,passed`.
    pub fn load(file: PathBuf) -> Picks {
        let mut picks = Picks {
            places: HashMap::new(),
            file: None,
        };
        let text = std::fs::read(&file)
            .map(|raw| String::from_utf8_lossy(&raw).into_owned())
            .unwrap_or_default();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if let [project, context, picked, passed] = fields.as_slice() {
                let passed: Vec<&str> = passed.split(',').filter(|name| !name.is_empty()).collect();
                picks.note(&format!("{project}\t{context}"), picked, &passed);
            }
        }
        picks.file = Some(file);
        picks
    }

    /// Notes that `picked` was accepted at `place`, with `passed` listed above it.
    pub fn note(&mut self, place: &str, picked: &str, passed: &[&str]) {
        if picked.is_empty() || picked.contains(['\t', '\n', ',']) {
            return;
        }
        let tallies = self.places.entry(place.to_string()).or_default();
        tallies.entry(picked.to_string()).or_default().picked += 1.0;
        for name in passed {
            tallies.entry(name.to_string()).or_default().passed += 1.0;
        }
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut handle) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
            let passed: Vec<&str> = passed
                .iter()
                .copied()
                .filter(|name| !name.contains(['\t', '\n', ',']))
                .collect();
            let _ = writeln!(handle, "{place}\t{picked}\t{}", passed.join(","));
        }
    }

    /// Moves candidates up or down by how the user has treated them here.
    pub fn adjust(&self, place: &str, items: &mut [Candidate]) {
        let Some(tallies) = self.places.get(place) else { return };
        for item in items {
            let Some(tally) = tallies.get(&item.name) else { continue };
            if tally.picked > 0.0 {
                // Rises quickly at first, then levels off.
                item.priority += PICK_BOOST * tally.picked / (tally.picked + 1.0);
            } else if tally.passed >= PATIENCE {
                item.priority -= PASS_PENALTY * (tally.passed / (tally.passed + PATIENCE));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Kind;

    fn priorities(picks: &Picks, at: &str, names: &[&str]) -> Vec<f64> {
        let mut items: Vec<Candidate> = names.iter().map(|n| Candidate::new(*n, Kind::Subcommand)).collect();
        picks.adjust(at, &mut items);
        items.iter().map(|c| c.priority).collect()
    }

    #[test]
    fn picked_suggestions_rise_and_ignored_ones_sink() {
        let mut picks = Picks::default();
        let at = place("/p", &["git"]);
        for _ in 0..4 {
            picks.note(&at, "cherry-pick", &["checkout"]);
        }
        let got = priorities(&picks, &at, &["checkout", "cherry-pick", "status"]);
        assert!(got[1] > 50.0 && got[0] < 50.0 && got[2] == 50.0, "{got:?}");
        // One pass is not held against a suggestion.
        let mut once = Picks::default();
        once.note(&at, "cherry-pick", &["checkout"]);
        assert_eq!(priorities(&once, &at, &["checkout"]), vec![50.0]);
        // A suggestion that is sometimes picked is never penalised.
        picks.note(&at, "checkout", &[]);
        assert!(priorities(&picks, &at, &["checkout"])[0] > 50.0);
    }

    #[test]
    fn what_is_learned_stays_in_its_place_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("hiruzen-picks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("picks.tsv");
        let mut picks = Picks::load(file.clone());
        let here = place("/p", &["git"]);
        picks.note(&here, "push", &["pull", "pu,ll"]);
        let reloaded = Picks::load(file);
        assert!(priorities(&reloaded, &here, &["push"])[0] > 50.0);
        assert_eq!(priorities(&reloaded, &place("/other", &["git"]), &["push"]), vec![50.0]);
        assert_eq!(
            priorities(&reloaded, &place("/p", &["git", "remote"]), &["push"]),
            vec![50.0]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

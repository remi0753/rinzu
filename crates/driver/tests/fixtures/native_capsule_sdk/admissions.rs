//! The explicit timer polls depend on OS completion timing. Check their causal
//! position and every callback child without requiring equal poll counts.
use rinzu_codegen::FutureAdmission;
use std::collections::{BTreeMap, BTreeSet};

pub fn from_report(report: &str) -> Vec<FutureAdmission> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_FUTURE_ADMISSIONS="))
        .expect("Future admission evidence")
        .split(',')
        .filter(|row| !row.is_empty())
        .map(|row| {
            let fields: Vec<u64> = row.split(':').map(|n| n.parse().unwrap()).collect();
            assert_eq!(fields.len(), 3);
            FutureAdmission {
                future: fields[0],
                parent_scope: fields[1],
                ordinal: fields[2],
            }
        })
        .collect()
}

pub fn check(rows: &[FutureAdmission]) {
    let mut ids = BTreeSet::new();
    let mut children = BTreeMap::<u64, Vec<&FutureAdmission>>::new();
    for row in rows {
        assert!(row.parent_scope == 0 || ids.contains(&(row.parent_scope - 1)));
        assert!(ids.insert(row.future), "duplicate Future");
        let siblings = children.entry(row.parent_scope).or_default();
        assert_eq!(row.ordinal, siblings.len() as u64);
        siblings.push(row);
    }
    let scope = |row: &FutureAdmission| row.future.checked_add(1).unwrap();
    let leaf = |row: &FutureAdmission| {
        assert!(
            !children.contains_key(&scope(row)),
            "unexpected nested task below {row:?}: {rows:?}"
        );
    };
    let main = children.get(&0).expect("Main children");
    assert!(main.len() >= 2, "both parents must be admitted");
    for parent in &main[..2] {
        let callback = children.get(&scope(parent)).expect("callback Origin");
        assert_eq!(callback.len(), 3, "Worker, child, direct reader");
        leaf(callback[0]);
        let child = children.get(&scope(callback[1])).expect("child file read");
        assert!(!child.is_empty());
        for operation in child {
            leaf(operation); // Zero or more ready polls, then the file read.
        }
        let direct = children.get(&scope(callback[2])).expect("direct file read");
        assert_eq!(direct.len(), 1);
        leaf(direct[0]);
    }
    for poll in &main[2..] {
        leaf(poll); // Main's explicit completion polls create no children.
    }
}

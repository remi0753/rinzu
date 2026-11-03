//! Private proof identities. Preserve alias equality while ignoring physical
//! interleaving of admissions belonging to independent Origin tasks.
use rinzu_codegen::{FutureAdmission, SourceAsyncRun};
use std::collections::BTreeMap;
fn paths(rows: &[FutureAdmission]) -> Result<BTreeMap<Vec<u64>, u64>, String> {
    let mut by_id = BTreeMap::<u64, Vec<u64>>::new();
    let mut next = BTreeMap::<u64, u64>::new();
    let mut by_path = BTreeMap::new();
    for row in rows {
        let mut path = if row.parent_scope == 0 {
            Vec::new()
        } else {
            by_id
                .get(&(row.parent_scope - 1))
                .cloned()
                .ok_or("admission parent is missing or not yet admitted")?
        };
        let ordinal = next.entry(row.parent_scope).or_default();
        if *ordinal != row.ordinal {
            return Err("admission ordinal is duplicate or noncontiguous".into());
        }
        *ordinal = ordinal.checked_add(1).ok_or("admission ordinal overflow")?;
        path.push(row.ordinal);
        if by_id.insert(row.future, path.clone()).is_some()
            || by_path.insert(path, row.future).is_some()
        {
            return Err("duplicate Future admission identity".into());
        }
    }
    Ok(by_path)
}
fn parse(text: &str) -> Result<Vec<FutureAdmission>, String> {
    text.split(',')
        .filter(|row| !row.is_empty())
        .map(|row| {
            let fields = row
                .split(':')
                .map(|field| {
                    let n = field
                        .parse::<u64>()
                        .map_err(|_| "invalid admission integer")?;
                    if n.to_string() != field {
                        return Err("noncanonical admission integer");
                    }
                    Ok(n)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if fields.len() != 3 {
                return Err("invalid admission shape".into());
            }
            Ok(FutureAdmission {
                future: fields[0],
                parent_scope: fields[1],
                ordinal: fields[2],
            })
        })
        .collect()
}
pub fn normalize(jit: &SourceAsyncRun, report: &str) -> Result<String, String> {
    normalize_rows(&jit.future_admissions, report)
}
fn normalize_rows(admissions: &[FutureAdmission], report: &str) -> Result<String, String> {
    let raw = report
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_FUTURE_ADMISSIONS="))
        .ok_or("async AOT Future admission proof is missing")?;
    let expected = paths(admissions)?;
    let actual = paths(&parse(raw)?)?;
    if expected.keys().ne(actual.keys()) {
        return Err("async JIT/AOT admission trees differ".into());
    }
    let ids: BTreeMap<u64, u64> = actual
        .iter()
        .map(|(path, id)| (*id, expected[path]))
        .collect();
    let future = |value: u64| {
        ids.get(&value)
            .copied()
            .ok_or_else(|| format!("unproved Future identity {value}"))
    };
    let scope = |value: u64| -> Result<u64, String> {
        if value == 0 {
            Ok(0)
        } else {
            future(value - 1)?
                .checked_add(1)
                .ok_or("task identity overflow".into())
        }
    };
    let convert = |value: &str, task: bool| -> Result<String, String> {
        let n = value.parse::<u64>().map_err(|_| "invalid proof identity")?;
        if value != n.to_string() {
            return Err("noncanonical proof identity".into());
        }
        Ok(if task { scope(n)? } else { future(n)? }.to_string())
    };
    let mut output = String::new();
    for line in report.lines() {
        let Some((key, value)) = line.split_once('=') else {
            output.push_str(line);
            output.push('\n');
            continue;
        };
        let value = match key {
            "RINZU_FUTURE_ADMISSIONS" => admissions
                .iter()
                .map(|r| format!("{}:{}:{}", r.future, r.parent_scope, r.ordinal))
                .collect::<Vec<_>>()
                .join(","),
            "RINZU_CANCELLED_CHILDREN" => value
                .split(',')
                .filter(|r| !r.is_empty())
                .map(|r| convert(r, false))
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
            "RINZU_AWAITED_TASK_VALUES"
            | "RINZU_CREDITS"
            | "RINZU_WORKER_HEAPS"
            | "RINZU_WORKER_USERDATA" => {
                let delimiter = if key == "RINZU_AWAITED_TASK_VALUES" {
                    ','
                } else {
                    ';'
                };
                value
                    .split(delimiter)
                    .filter(|r| !r.is_empty())
                    .map(|row| {
                        let mut fields = row.split(':').map(str::to_owned).collect::<Vec<_>>();
                        let index = usize::from(key == "RINZU_CREDITS");
                        if fields.len() <= index {
                            return Err("invalid identity-bearing proof row".into());
                        }
                        if key == "RINZU_AWAITED_TASK_VALUES" {
                            if fields.len() != 4 {
                                return Err("invalid await proof row".into());
                            }
                            fields[0] = convert(&fields[0], true)?;
                            fields[1] = convert(&fields[1], false)?;
                        } else {
                            fields[index] = convert(&fields[index], false)?;
                        }
                        Ok(fields.join(":"))
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .join(&delimiter.to_string())
            }
            "RINZU_TRACE" => {
                let mut trace: serde_json::Value =
                    serde_json::from_str(value).map_err(|_| "invalid trace JSON")?;
                let events = trace["events"]
                    .as_array_mut()
                    .ok_or("trace events are missing")?;
                for event in events {
                    if let Some(task) = event["task"].as_str().and_then(|v| v.strip_prefix("main:"))
                    {
                        event["task"] = format!("main:{}", convert(task, true)?).into();
                    }
                    if let Some(id) = event["operation"]
                        .as_str()
                        .and_then(|v| v.strip_prefix("future:"))
                    {
                        event["operation"] = format!("future:{}", convert(id, false)?).into();
                    }
                }
                serde_json::to_string(&trace).map_err(|e| e.to_string())?
            }
            _ => value.to_owned(),
        };
        output.push_str(key);
        output.push('=');
        output.push_str(&value);
        output.push('\n');
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_normalization_preserves_reawait_values_and_rejects_unproved_identity() {
        let rows = [
            FutureAdmission {
                future: 0,
                parent_scope: 0,
                ordinal: 0,
            },
            FutureAdmission {
                future: 1,
                parent_scope: 1,
                ordinal: 0,
            },
            FutureAdmission {
                future: 2,
                parent_scope: 0,
                ordinal: 1,
            },
        ];
        let report = "RINZU_FUTURE_ADMISSIONS=5:0:0,9:0:1,8:6:0\nRINZU_AWAITED_TASK_VALUES=0:5:1:0000000000000007,0:5:1:0000000000000007,6:8:1:0000000000000009\nRINZU_TRACE={\"events\":[{\"task\":\"main:6\",\"operation\":\"future:8\",\"kind\":\"await-ready\"}]}\n";
        let result = normalize_rows(&rows, report).unwrap();
        assert!(
            result.contains("0:0:1:0000000000000007,0:0:1:0000000000000007,1:1:1:0000000000000009")
        );
        assert!(result.contains("main:1") && result.contains("future:1"));
        assert!(normalize_rows(&rows, &report.replace("future:8", "future:99")).is_err());
        assert!(normalize_rows(&rows, &report.replace("8:6:0", "8:0:2")).is_err());
    }
    #[test]
    fn paths_preserve_parent_order_and_reject_duplicate_and_unknown_parents() {
        let rows = [
            FutureAdmission {
                future: 0,
                parent_scope: 0,
                ordinal: 0,
            },
            FutureAdmission {
                future: 1,
                parent_scope: 1,
                ordinal: 0,
            },
            FutureAdmission {
                future: 2,
                parent_scope: 0,
                ordinal: 1,
            },
        ];
        let mut changed = rows;
        changed[1].future = 2;
        changed[2].future = 1;
        assert_eq!(
            paths(&rows).unwrap().keys().collect::<Vec<_>>(),
            paths(&changed).unwrap().keys().collect::<Vec<_>>()
        );
        changed[2].ordinal = 0;
        assert!(paths(&changed).is_err());
        changed = rows;
        changed[1].parent_scope = 100;
        assert!(paths(&changed).is_err());
        assert!(parse("00:0:0").is_err());
    }
}

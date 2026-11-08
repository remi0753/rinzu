use rinzu_runtime_model::{Model, PostResult, explore};

#[test]
fn reference_model_consumes_the_shared_production_vectors() {
    let document: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/conformance/runtime-model-host-vectors.json"
    ))
    .unwrap();
    assert_eq!(document["schema"], "rinzu.runtime-model.host-vectors");
    assert_eq!(document["version"], 1);
    for vector in document["vectors"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let outcome = match vector["postResult"].as_str().unwrap() {
            "accepted" => PostResult::Accepted,
            "closed" => PostResult::Closed,
            "failure" => PostResult::Failure,
            value => panic!("unknown post result `{value}` in {name}"),
        };
        let report = explore(Model::active(vec![outcome]), 10_000)
            .unwrap_or_else(|error| panic!("{name} produced a counterexample:\n{error}"));
        match vector["terminal"].as_str().unwrap() {
            "graceful" => assert_eq!(report.fail_fast_terminals, 0, "{name}"),
            "fail-fast" => assert!(report.fail_fast_terminals > 0, "{name}"),
            value => panic!("unknown terminal `{value}` in {name}"),
        }
    }
}

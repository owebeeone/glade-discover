use glade_discover_sim::{Scenario, run_scenario};

#[test]
fn empty_queue_satisfies_a_quiescent_stop_without_sleeping() {
    let scenario = Scenario::from_json(
        r#"{
          "seed":1,
          "event_budget":10,
          "nodes":[{
            "id":"node-a","principal":"principal-a","owns":[],"seed_grants":[],
            "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},
            "restarts":[]
          }],
          "links":[],"inputs":[],"verifier_fixtures":[],
          "stop":{"kind":"quiescent_for_ms","quiet_ms":5,"deadline_ms":10},
          "expect":[{
            "when":{"kind":"at_ms","at_ms":0},"kind":"state","node":"node-a",
            "assertion":{"kind":"clock","value":{"kind":"ready","watermark":1000}}
          }]
        }"#,
    )
    .expect("strict quiescent scenario");

    assert!(run_scenario(&scenario).is_ok());
}

#[test]
fn pending_one_shot_work_beyond_the_deadline_prevents_quiescence() {
    let scenario = Scenario::from_json(
        r#"{
          "seed":1,"event_budget":20,
          "nodes":[
            {"id":"node-a","principal":"principal-a","owns":[],"seed_grants":[],
             "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},"restarts":[]},
            {"id":"node-b","principal":"principal-b","owns":[],"seed_grants":[],
             "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},"restarts":[]}
          ],
          "links":[],
          "inputs":[{"input_id":"tick","at_ms":0,"node":"node-a","event":{"kind":"wakeup",
            "token":{"kind":"gossip_tick","peer":"node-b"}}}],
          "verifier_fixtures":[],
          "stop":{"kind":"quiescent_for_ms","quiet_ms":5,"deadline_ms":10},
          "expect":[{"when":{"kind":"at_ms","at_ms":0},"kind":"state","node":"node-a",
            "assertion":{"kind":"sync","peer":"node-b","sync_id":"node-a:0","status":"active"}}]
        }"#,
    )
    .expect("strict quiescent scenario");

    assert_eq!(
        run_scenario(&scenario),
        Err(glade_discover_sim::RunnerError::DidNotQuiesce)
    );
}

#[test]
fn substantive_periodic_tick_at_the_deadline_resets_quiescence() {
    let scenario = Scenario::from_json(
        r#"{
          "seed":1,"event_budget":20,
          "nodes":[
            {"id":"node-a","principal":"principal-a","owns":[],"seed_grants":[],
             "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},"restarts":[]},
            {"id":"node-b","principal":"principal-b","owns":[],"seed_grants":[],
             "clock":{"initial_wall_ms":1000,"initial_mono_ms":0,"changes":[]},"restarts":[]}
          ],
          "links":[],
          "inputs":[{"input_id":"tick","at_ms":5,"node":"node-a","event":{"kind":"wakeup",
            "token":{"kind":"gossip_tick","peer":"node-b"}}}],
          "verifier_fixtures":[],
          "stop":{"kind":"quiescent_for_ms","quiet_ms":5,"deadline_ms":5},
          "expect":[{"when":{"kind":"at_ms","at_ms":0},"kind":"state","node":"node-a",
            "assertion":{"kind":"clock","value":{"kind":"ready","watermark":1000}}}]
        }"#,
    )
    .expect("strict quiescent scenario");

    assert_eq!(
        run_scenario(&scenario),
        Err(glade_discover_sim::RunnerError::DidNotQuiesce)
    );
}

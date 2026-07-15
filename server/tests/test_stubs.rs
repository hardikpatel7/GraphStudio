//! Tests for the inert scaffold stubs (`server/stubs/*`) that replace the
//! private `rust-shared-utils` crates in the public build. These pin the
//! behavior the server relies on: rcl resolves to nothing (and carries no
//! algorithm), the rcl rule store's watch channel stays open, and the
//! `pipeline` trigger parsing/accessors match what the scheduler expects.
//!
//! (These run via `cargo test --manifest-path server/Cargo.toml`; integration
//! test crates can use the server's normal dependencies `rcl` and `pipeline`.)

// ── rcl: inert / no proprietary logic ────────────────────────────────────────

#[test]
fn rcl_rule_never_matches() {
    let rule = rcl::RclRule {
        rcl_code: "R1".into(),
        priority: 5,
        specificity: 3,
        sel_l0: None,
        sel_l1: None,
        sel_l2: None,
        sel_l3: None,
        sel_l4: None,
        sel_l5: None,
        sel_brand: None,
        sel_article: None,
    };
    // Inert stub: no selection logic, so nothing ever matches.
    assert!(!rule.matches("a", "b", "c", "d", "e", "f", "brand"));
}

#[test]
fn rcl_resolvers_return_empty() {
    let rs = rcl::RuleSet::default();
    let products = vec![rcl::ProductHierarchy {
        product_code: "p1",
        l0_name: "l0",
        l1_name: "l1",
        l2_name: "l2",
        l3_name: "l3",
        l4_name: "l4",
        l5_name: "l5",
        brand: "acme",
    }];

    assert!(rcl::resolve_dc_policy(&rs, &products).is_empty());
    assert!(rcl::resolve_constraints(&rs, &products).is_empty());

    let psm_inputs = vec![rcl::PsmInput {
        hierarchy: products[0],
        store_code: "store-1",
        psa_code: "psa-1",
    }];
    assert!(rcl::resolve_psm(&rs, &psm_inputs).is_empty());
}

#[tokio::test]
async fn rcl_rulestore_watch_stays_open() {
    let store = rcl::RuleStore::start(
        "dsn".to_string(),
        Box::new(rcl::PgListenSource::new("dsn")),
        rcl::StoreQueries::default(),
    )
    .await
    .expect("stub RuleStore::start never errors");

    // Snapshot is the single empty ruleset.
    assert!(store.snapshot().rules.is_empty());

    // The store must retain its watch Sender so a subscriber's `changed()`
    // does NOT observe a closed channel (which would break the scheduler loop
    // and the gRPC WatchStream). A closed channel resolves `changed()` to
    // `Err` immediately; an open-but-idle one stays pending (times out here).
    let mut rx = store.subscribe();
    let res = tokio::time::timeout(std::time::Duration::from_millis(100), rx.changed()).await;
    assert!(
        !matches!(res, Ok(Err(_))),
        "rcl RuleStore watch channel must stay open (Sender retained), got {res:?}"
    );
}

// ── pipeline::PipelineTrigger: serde + accessors match real crate ────────────

#[test]
fn trigger_serde_roundtrips_all_variants() {
    use pipeline::PipelineTrigger;

    // Internally-tagged, snake_case — must match what the scheduler parses
    // from stored JSON (e.g. schema.sql seeds `{"kind":"manual"}`).
    assert_eq!(
        serde_json::to_string(&PipelineTrigger::Manual).unwrap(),
        r#"{"kind":"manual"}"#
    );
    assert_eq!(
        serde_json::to_string(&PipelineTrigger::RclChange).unwrap(),
        r#"{"kind":"rcl_change"}"#
    );

    let cases = vec![
        PipelineTrigger::Manual,
        PipelineTrigger::Scheduled {
            cron: "0 0 * * *".into(),
        },
        PipelineTrigger::Cdc {
            source_ids: vec!["s1".into(), "s2".into()],
        },
        PipelineTrigger::RclChange,
        PipelineTrigger::Composed {
            triggers: vec![
                PipelineTrigger::Cdc {
                    source_ids: vec!["a".into()],
                },
                PipelineTrigger::RclChange,
            ],
        },
    ];
    for t in cases {
        let json = serde_json::to_string(&t).unwrap();
        let back: PipelineTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            json,
            "trigger did not round-trip: {json}"
        );
    }
}

#[test]
fn trigger_recursion_and_cron() {
    use pipeline::PipelineTrigger;

    let nested = PipelineTrigger::Composed {
        triggers: vec![
            PipelineTrigger::Cdc {
                source_ids: vec!["a".into(), "b".into()],
            },
            PipelineTrigger::Composed {
                triggers: vec![
                    PipelineTrigger::Cdc {
                        source_ids: vec!["c".into()],
                    },
                    PipelineTrigger::RclChange,
                ],
            },
        ],
    };
    // cdc_source_ids and listens_for_rcl recurse into Composed.
    assert_eq!(nested.cdc_source_ids(), vec!["a", "b", "c"]);
    assert!(nested.listens_for_rcl());

    let manual = PipelineTrigger::Manual;
    assert!(manual.cdc_source_ids().is_empty());
    assert!(!manual.listens_for_rcl());

    // cron() returns the expression only for Scheduled.
    assert_eq!(
        PipelineTrigger::Scheduled {
            cron: "*/5 * * * *".into()
        }
        .cron(),
        Some("*/5 * * * *")
    );
    assert_eq!(PipelineTrigger::Manual.cron(), None);
    assert_eq!(PipelineTrigger::RclChange.cron(), None);
}

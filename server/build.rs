fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate Serialize on every article_graph message so the
    // handlers in handlers/article_graph.rs can hand them to
    // `serde_json::to_value` without manual mapping.
    let article_graph_messages = [
        "MatchProductRequest",
        "MatchProductResponse",
        "ProductHierarchy",
        "ResolveRclRequest",
        "ResolveRclResponse",
        "DcPolicyExplain",
        "DcPolicy",
        "ConstraintsExplain",
        "ConstraintRow",
        "PsmExplain",
        "AggregateAtRequest",
        "AggregateAtResponse",
        "Aggregates",
    ];
    let mut config = tonic_build::configure()
        .build_server(true)
        .build_client(false);
    for m in article_graph_messages {
        config = config.type_attribute(
            format!("article_graph.{}", m),
            "#[derive(serde::Serialize)]",
        );
    }
    // Oneof key wrappers don't follow the message-name-only path —
    // their generated enum lives at the parent message scope. Cover
    // both `Key` enums (MatchProduct, ResolveRcl).
    config = config
        .type_attribute(
            "article_graph.MatchProductRequest.key",
            "#[derive(serde::Serialize)]",
        )
        .type_attribute(
            "article_graph.ResolveRclRequest.key",
            "#[derive(serde::Serialize)]",
        );
    config.compile_protos(
        &[
            "proto/rcl.proto",
            "proto/article_selection.proto",
            "proto/article_graph.proto",
            "proto/cross_filter.proto",
        ],
        &["proto"],
    )?;
    Ok(())
}

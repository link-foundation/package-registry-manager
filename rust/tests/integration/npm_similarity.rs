use super::mock_registry::MockRegistry;
use package_registry_manager::npm_similarity::check_similarity;

#[tokio::test]
async fn trailing_punctuation_never_generates_an_empty_similarity_search() {
    let registry = MockRegistry::start(|_| None);
    for suffix in ["-", "_", "."] {
        check_similarity(
            &format!("unique-publish-example{suffix}"),
            &registry.base,
            None,
        )
        .await
        .unwrap();
    }
    let queries = registry.requests();
    assert_eq!(queries.len(), 3, "{queries:?}");
    assert!(!queries.iter().any(|query| query.contains("text=&")));
}

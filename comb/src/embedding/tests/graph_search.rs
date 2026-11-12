use super::*;
use crate::{
    graph::Query as GraphQuery,
    values::{MatchEvidence, RelationPredicate},
};

#[test]
// O19/O20: fixed cutoff 3, one versus twenty dual-route distractors.
fn independent_graph_arm_keeps_answers_even_when_dual_route_distractors_win_rrf() {
    for count in [1, 20] {
        let (_root, mut comb, mut mock) = setup();
        put(
            &mut comb,
            "concepts/anchor",
            "Anchor",
            "## Relations\n- [[depends_on::concepts/target]]",
        );
        for n in 0..count {
            put(
                &mut comb,
                &format!("concepts/distractor-{n:02}"),
                "Mention",
                "feline",
            );
        }
        fill(&mut comb, &mut mock);
        // The target has neither query words nor a vector; graph must add it independently.
        put(&mut comb, "concepts/target", "Target", "A different answer");
        let mut req = request("feline");
        req.limit = 3;
        let plain = comb
            .index
            .retrieve(
                &req,
                Mode::Hybrid,
                QueryVector::Vector(query(&comb, &mut mock, &req.query)),
            )
            .unwrap();
        assert!(
            !plain
                .results
                .iter()
                .any(|h| h.slug.as_str() == "concepts/target")
        );
        req.graph = Some(GraphQuery::new(
            "concepts/anchor".parse().unwrap(),
            vec![RelationPredicate::DependsOn],
        ));
        let result = comb
            .index
            .retrieve(
                &req,
                Mode::Hybrid,
                QueryVector::Vector(query(&comb, &mut mock, &req.query)),
            )
            .unwrap();
        let target = result
            .results
            .iter()
            .find(|h| h.slug.as_str() == "concepts/target");
        assert_eq!(target.is_some(), count == 1);
        eprintln!(
            "O{}: relevant_at_3={}, precision_at_3={}/3, recall_at_3={}/1, relation_recall=1/1",
            if count == 1 { 19 } else { 20 },
            u8::from(target.is_some()),
            u8::from(target.is_some()),
            u8::from(target.is_some()),
        );
        if let Some(target) = target {
            assert_eq!(target.matched_by, Route::Graph);
            assert_eq!(target.evidence, MatchEvidence::Relation);
            assert_eq!(target.create_safety, CreateSafety::Unknown);
        }
        let graph = result.graph.unwrap();
        assert!(graph.complete);
        assert_eq!(graph.relations[0].target.as_str(), "concepts/target");
        for hit in result
            .results
            .iter()
            .filter(|h| h.slug.as_str().contains("distractor"))
        {
            let prior = plain.results.iter().find(|p| p.slug == hit.slug).unwrap();
            assert_eq!(hit.score.to_bits(), prior.score.to_bits());
        }
    }
}

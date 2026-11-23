use chrono::NaiveDate;
use factory_records::domain::evaluation::{CalibrationTally, EvaluationRecords, ScorerTally};
use serde_json::json;

use super::ScorerCalibration;
use crate::domain::evaluation::EvaluationDefinition;
use crate::domain::evaluation::insights::evaluation_insights;
use crate::domain::insights::InsightsQuery;

fn tally(scorer: &str, labelled: u32, agreed: u32) -> CalibrationTally {
    CalibrationTally {
        scorer: scorer.to_string(),
        labelled,
        agreed,
    }
}

/// A Scorer is calibrated once a person labelled at least twenty of its
/// judgments and it agreed with at least four in five of them; fewer labels
/// never calibrate it, however well it agrees.
#[test]
fn a_scorer_is_calibrated_once_enough_labels_agree_often_enough() {
    let cases = [
        (0, 0, None, false),
        (19, 19, Some(1.0), false),
        (20, 15, Some(0.75), false),
        (20, 16, Some(0.8), true),
        (40, 39, Some(0.975), true),
    ];
    for (labelled, agreed, agreement, calibrated) in cases {
        let calibration = ScorerCalibration::of(Some(&tally("correctness", labelled, agreed)));
        assert_eq!(calibration.labelled, labelled);
        assert_eq!(calibration.agreed, agreed);
        assert_eq!(calibration.agreement, agreement, "{labelled} labels");
        assert_eq!(calibration.calibrated, calibrated, "{labelled} labels");
        assert_eq!(calibration.needed, 20);
        assert_eq!(calibration.threshold, 0.8);
    }
    let unlabelled = ScorerCalibration::of(None);
    assert_eq!(unlabelled.labelled, 0);
    assert_eq!(unlabelled.agreement, None);
    assert!(!unlabelled.calibrated);
}

/// Insights give each defined Scorer its calibration from the person's
/// labels; an unlabelled one, and a retired one, which has no current
/// judge, read uncalibrated.
#[test]
fn insights_give_each_scorer_its_calibration() {
    let definition: EvaluationDefinition = serde_json::from_value(json!({
        "scorers": [
            { "name": "correctness", "instructions": "Did it work?", "samplePercent": 100 },
            { "name": "tidiness", "instructions": "Is it tidy?", "samplePercent": 10 }
        ]
    }))
    .expect("a definition");
    let day = |day: u32| NaiveDate::from_ymd_opt(2026, 11, day).expect("valid day");
    let query = InsightsQuery::new(day(1), day(30), "Europe/Lisbon", None).expect("query");
    let records = EvaluationRecords {
        scorers: vec![ScorerTally {
            scorer: "retired".to_string(),
            judged: 2,
            passed: 1,
            mean_score: 0.5,
        }],
        ..Default::default()
    };
    let calibrations = vec![tally("correctness", 20, 17)];

    let insights = evaluation_insights(&definition, &query, records, calibrations);

    let calibrations: Vec<_> = insights
        .scorers
        .iter()
        .map(|scorer| {
            (
                scorer.name.as_str(),
                scorer.calibration.labelled,
                scorer.calibration.agreed,
                scorer.calibration.calibrated,
            )
        })
        .collect();
    assert_eq!(
        calibrations,
        [
            ("correctness", 20, 17, true),
            ("tidiness", 0, 0, false),
            ("retired", 0, 0, false),
        ]
    );
}

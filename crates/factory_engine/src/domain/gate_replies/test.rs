//! Gate replies and the stand-in, over the fakes: a reply answers its Gate
//! or is asked back, the stand-in answers a routine Gate or leaves it to
//! its owner, and a comment on the Run's task is a reply.

use chrono::Utc;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    ControlFileChanges, EntryId, GateModelReading, MAX_NOTE_BYTES, NewRun, Outcome, Repository,
    Role, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, RulesSource, RunRules, StandInKinds,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::service::GateRepliesImpl;
use super::{Decision, GateReplySettings, GateReplySettingsError, decide, percent};
use crate::domain::fakes::{FakeGateModel, FakeNotifier, FakeRepo};
use crate::domain::gate::{ReplyRead, ReplyReading, StandInAsking, StandInKeeping, StandingIn};
use crate::domain::gate_owner::{notice_id, stand_in_id};
use crate::domain::ports::{GateReplyService, ModelPick};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn plan_answers() -> Vec<Outcome> {
    vec![outcome("approve"), outcome("revise")]
}

/// Rules that stand in for the owner at the plan and Blockers Gates, and
/// decide nothing themselves.
const STAND_IN_ONLY: OrchestratorDefinition = OrchestratorDefinition {
    plan: false,
    blockers: false,
    merge: false,
    owner_gates: false,
    quiet_gates: false,
    task_thread: false,
    rerun_flaky_ci: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
    stand_in: StandInKinds {
        plan: true,
        blockers: true,
    },
};

fn base() -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    }
}

/// A `code` Run of Julia's on `task_id` with `rules` (none for a Run
/// created before Runs kept rules), whose planner's Step 1 made a plan and
/// whose Step 2 waits on the plan Gate.
async fn run_at_plan_gate(
    repo: &FakeRepo,
    rules: Option<OrchestratorDefinition>,
    task_id: Option<Uuid>,
) -> StepKey {
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id,
            project_id: None,
            repository: Some(&base()),
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules {
                definition: rules.unwrap_or_default(),
                source: RulesSource::Team,
                project_id: None,
            },
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("the run is kept");
    };
    if rules.is_none() {
        let mut runs = repo.runs.lock().expect("runs");
        if let Some(kept) = runs.get_mut(&run.id) {
            kept.standing_rules = None;
        }
    }

    let planned = StepKey {
        run_id: run.id,
        seq: 1,
    };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(planned, &entry, 1).await.expect("start");
    repo.end_step(planned, &outcome("done"), "Change the session check.")
        .await
        .expect("end");
    {
        let mut steps = repo.steps.lock().expect("steps");
        if let Some((_, step)) = steps
            .iter_mut()
            .find(|(id, step)| *id == run.id && step.seq == 1)
        {
            step.role = Some(Role::try_from("planner".to_string()).expect("valid"));
        }
    }

    let key = StepKey {
        run_id: run.id,
        seq: 2,
    };
    let gated = EntryId::try_from("plan-review".to_string()).expect("valid");
    repo.start_step(key, &gated, 1).await.expect("start");
    repo.end_step(key, &outcome("done"), "The plan reads well.")
        .await
        .expect("end");
    repo.open_gate(key, "Approve this plan?", &plan_answers())
        .await
        .expect("open");
    key
}

fn gates(repo: &FakeRepo, model: &FakeGateModel) -> GateRepliesImpl<FakeRepo, FakeGateModel> {
    GateRepliesImpl::new(repo.clone(), model.clone(), GateReplySettings::default())
}

fn plan_keeping(key: StepKey) -> StandInKeeping {
    StandInKeeping {
        key,
        kind: RuleGateKind::Plan,
        person_only: false,
        fix_rounds: 0,
        fix_limit: 0,
    }
}

fn reading(key: StepKey, words: &str) -> ReplyReading {
    ReplyReading {
        key,
        question: false,
        reply: words.to_string(),
        user_id: julia(),
    }
}

fn plan_asking(key: StepKey) -> StandInAsking {
    StandInAsking {
        key,
        kind: RuleGateKind::Plan,
        person_only: false,
        fix_rounds: 0,
        fix_limit: 0,
        answers: plan_answers(),
    }
}

/// The Gate of `key` as the records keep it.
async fn gate(repo: &FakeRepo, key: StepKey) -> factory_records::domain::models::Gate {
    repo.list_steps(key.run_id)
        .await
        .expect("steps")
        .into_iter()
        .find(|step| step.seq == key.seq)
        .and_then(|step| step.gate)
        .expect("gate")
}

#[test]
fn settings_left_out_keep_their_defaults_and_a_threshold_is_a_percent() {
    let defaults = GateReplySettings::configured(None, None, None).expect("defaults");
    assert_eq!(defaults, GateReplySettings::default());
    let set =
        GateReplySettings::configured(Some(70), Some(95), Some(std::time::Duration::from_secs(60)))
            .expect("set");
    assert_eq!(set.reply_threshold, 70);
    assert_eq!(set.stand_in_threshold, 95);
    assert_eq!(set.stand_in_window.as_secs(), 60);
    assert_eq!(
        GateReplySettings::configured(Some(101), None, None),
        Err(GateReplySettingsError::NotAPercent(101))
    );
    // A window shorter than a minute, or longer than a day, is refused.
    assert_eq!(
        GateReplySettings::configured(None, None, Some(std::time::Duration::from_secs(59))),
        Err(GateReplySettingsError::WindowOutOfRange(59))
    );
    assert_eq!(
        GateReplySettings::configured(None, None, Some(std::time::Duration::from_secs(86_401))),
        Err(GateReplySettingsError::WindowOutOfRange(86_401))
    );
    assert!(
        GateReplySettings::configured(None, None, Some(std::time::Duration::from_secs(86_400)))
            .is_ok()
    );
}

#[test]
fn a_pick_counts_only_at_or_above_the_threshold_and_among_the_answers() {
    let pick = |choice: Option<&str>, confidence: f64| ModelPick {
        choice: choice.map(outcome),
        confidence,
        reason: String::new(),
    };
    assert_eq!(
        decide(Some(&pick(Some("approve"), 0.85)), &plan_answers(), 85),
        Decision::Decided {
            answer: outcome("approve"),
            confidence: 85
        }
    );
    assert_eq!(
        decide(Some(&pick(Some("approve"), 0.84)), &plan_answers(), 85),
        Decision::Undecided
    );
    assert_eq!(
        decide(Some(&pick(None, 0.99)), &plan_answers(), 85),
        Decision::Undecided
    );
    assert_eq!(
        decide(Some(&pick(Some("merge"), 0.99)), &plan_answers(), 85),
        Decision::Undecided
    );
    assert_eq!(decide(None, &plan_answers(), 0), Decision::Undecided);
    assert_eq!(percent(f64::NAN), 0);
    assert_eq!(percent(1.7), 100);
}

/// A reply the model reads as one of the Gate's answers, sure enough,
/// answers the Gate, the words kept as its note with how sure it was.
#[tokio::test]
async fn a_reply_read_at_or_above_the_threshold_answers_the_gate() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.85);

    let read = gates(&repo, &model)
        .read_reply(reading(key, "Looks right, go ahead."))
        .await
        .expect("read");

    assert_eq!(read, ReplyRead::Answered(outcome("approve")));
    let answer = gate(&repo, key).await.answer.expect("answered");
    assert_eq!(answer.note.as_deref(), Some("Looks right, go ahead."));
    assert_eq!(answer.reply_confidence, Some(85));
    assert_eq!(answer.user_id, Some(julia()));
    // The model read the Gate's question and the words, as data.
    let asked = model.asked();
    assert!(asked[0].prompt.contains("Approve this plan?"));
    assert!(asked[0].prompt.contains("Looks right, go ahead."));
}

/// On a Tampered Run, words the model cannot read, or reads as Approve
/// below the threshold, are asked back as any others: not as an approval
/// only the button gives.
#[tokio::test]
async fn unclear_words_on_a_tampered_run_are_not_refused_approvals() {
    for (choice, confidence) in [(None, 0.95), (Some("approve"), 0.5)] {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
        repo.control_file_changes.lock().expect("changes").insert(
            key.run_id,
            ControlFileChanges {
                tampered: vec![".github/workflows/ci.yml".to_string()],
                named: Vec::new(),
                unlisted: false,
            },
        );
        let model = FakeGateModel::picking(choice, confidence);

        let read = gates(&repo, &model)
            .read_reply(reading(key, "Hmm"))
            .await
            .expect("read");

        assert_eq!(read, ReplyRead::AskedBack);
        let asked = gate(&repo, key).await.asked_back.expect("asked back");
        assert!(!asked.refused_tampered, "{choice:?} at {confidence}");
    }
}

/// A pick the Gate does not offer is kept as no pick, as it decides
/// nothing, whether the words are asked back or the stand-in declines.
#[tokio::test]
async fn a_pick_the_gate_does_not_offer_is_kept_as_none() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("merge"), 0.97);
    model.then(Some("merge"), 0.97);
    let service = gates(&repo, &model);

    service.stand_in(plan_asking(key)).await.expect("stand in");
    service
        .read_reply(reading(key, "Ship it"))
        .await
        .expect("read");

    let gate = gate(&repo, key).await;
    let declined = gate.stand_in_declined.expect("declined");
    assert_eq!(declined.pick, None);
    assert_eq!(declined.confidence, Some(97));
    let asked = gate
        .asked_back
        .and_then(|asked| asked.reading)
        .expect("reading");
    assert_eq!(asked.pick, None);
}

/// A reply the model is not sure enough of, or cannot tell, is asked back
/// on the Gate, which still waits.
#[tokio::test]
async fn a_reply_below_the_threshold_or_unclear_is_asked_back() {
    for (choice, confidence) in [(Some("approve"), 0.84), (None, 0.95)] {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
        let model = FakeGateModel::picking(choice, confidence);

        let read = gates(&repo, &model)
            .read_reply(reading(key, "Hmm, maybe?"))
            .await
            .expect("read");

        assert_eq!(read, ReplyRead::AskedBack);
        let gate = gate(&repo, key).await;
        assert_eq!(gate.answer, None);
        let asked = gate.asked_back.expect("asked back");
        assert_eq!(asked.reply, "Hmm, maybe?");
        // How the model read the words is kept, so the owner can tell why.
        assert_eq!(
            asked.reading,
            Some(GateModelReading {
                pick: choice.map(outcome),
                confidence: Some(percent(confidence)),
                reason: "The owner approved every plan like this one.".to_string(),
            })
        );
    }
}

/// A model that fails, or takes too long, never decides: the factory asks
/// back.
#[tokio::test(start_paused = true)]
async fn a_failed_or_timed_out_call_asks_back() {
    let failing = FakeGateModel::failing();
    let hanging = FakeGateModel {
        hangs: true,
        ..FakeGateModel::default()
    };
    for model in [failing, hanging] {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;

        let read = gates(&repo, &model)
            .read_reply(reading(key, "Approve it"))
            .await
            .expect("read");

        assert_eq!(read, ReplyRead::AskedBack);
        let gate = gate(&repo, key).await;
        assert_eq!(gate.answer, None);
        // A model that did not answer leaves no pick, confidence or reason.
        assert_eq!(
            gate.asked_back.and_then(|asked| asked.reading),
            Some(GateModelReading {
                pick: None,
                confidence: None,
                reason: String::new(),
            })
        );
    }
}

/// Words never approve a Tampered Run: its Control Files are approved with
/// the button, which lists them.
#[tokio::test]
async fn words_never_approve_a_tampered_run() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
    repo.control_file_changes.lock().expect("changes").insert(
        key.run_id,
        ControlFileChanges {
            tampered: vec![".github/workflows/ci.yml".to_string()],
            named: Vec::new(),
            unlisted: false,
        },
    );
    let model = FakeGateModel::picking(Some("approve"), 0.99);

    let read = gates(&repo, &model)
        .read_reply(reading(key, "Approve"))
        .await
        .expect("read");

    assert_eq!(read, ReplyRead::AskedBack);
    // It is kept as refused for the Tampered Run, beside the model's own
    // reading, whose reason stays as the model gave it, within the note's
    // size, however long.
    let asked = gate(&repo, key).await.asked_back.expect("asked back");
    assert!(asked.refused_tampered);
    let kept = asked.reading.expect("reading");
    assert_eq!(kept.pick, Some(outcome("approve")));
    assert_eq!(kept.reason, "The owner approved every plan like this one.");

    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
    repo.control_file_changes.lock().expect("changes").insert(
        key.run_id,
        ControlFileChanges {
            tampered: vec![".github/workflows/ci.yml".to_string()],
            named: Vec::new(),
            unlisted: false,
        },
    );
    let model = FakeGateModel::default();
    model.picks.lock().expect("picks").push_back(Ok(ModelPick {
        choice: Some(outcome("approve")),
        confidence: 0.99,
        reason: "y".repeat(5000),
    }));
    gates(&repo, &model)
        .read_reply(reading(key, "Approve"))
        .await
        .expect("read");
    let reason = gate(&repo, key)
        .await
        .asked_back
        .and_then(|asked| asked.reading)
        .expect("reading")
        .reason;
    assert_eq!(reason.len(), MAX_NOTE_BYTES);
}

/// An agent's question takes the words as its answer, with no model; a
/// retry returns the answer kept first.
#[tokio::test]
async fn a_question_gate_takes_the_words_as_the_answer() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
    let model = FakeGateModel::default();
    let service = gates(&repo, &model);
    let question = ReplyReading {
        question: true,
        ..reading(key, "Use the session cookie.")
    };

    let read = service.read_reply(question.clone()).await.expect("read");
    let again = service.read_reply(question).await.expect("read again");

    assert_eq!(read, ReplyRead::Answered(outcome("answer")));
    assert_eq!(again, read);
    assert!(model.asked().is_empty());
    let answer = gate(&repo, key).await.answer.expect("answered");
    assert_eq!(answer.note.as_deref(), Some("Use the session cookie."));
    assert_eq!(answer.reply_confidence, None);
}

/// The stand-in answers a routine Gate at or above its threshold, keeps
/// its reason, and gives the owner the window to override it; it read the
/// plan and the owner's past answers.
#[tokio::test]
async fn the_stand_in_answers_a_routine_gate_at_or_above_its_threshold() {
    let repo = FakeRepo::default();
    let earlier = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    repo.answer_gate(
        earlier,
        &outcome("revise"),
        Some("Cover the edge cases"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("past answer");
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.9);

    let standing = gates(&repo, &model)
        .stand_in(plan_asking(key))
        .await
        .expect("stand in")
        .expect("it answers");

    assert_eq!(standing.answer, outcome("approve"));
    assert!((598..=600).contains(&standing.window_secs));
    let kept = gate(&repo, key).await.stand_in.expect("kept");
    assert_eq!(kept.confidence, 90);
    assert_eq!(kept.reason, "The owner approved every plan like this one.");
    let prompt = &model.asked()[0].prompt;
    assert!(prompt.contains("Change the session check."));
    assert!(prompt.contains("The plan reads well."));
    assert!(prompt.contains("Cover the edge cases"));
    // The Gate still waits: the stand-in's answer is the owner's to
    // override until the window ends.
    assert_eq!(gate(&repo, key).await.answer, None);
}

/// Below its threshold, unsure, or failing, the stand-in leaves the Gate
/// to its owner, and keeps nothing.
#[tokio::test]
async fn below_its_threshold_the_stand_in_leaves_the_gate_to_its_owner() {
    for model in [
        FakeGateModel::picking(Some("approve"), 0.89),
        FakeGateModel::picking(None, 0.99),
        FakeGateModel::failing(),
    ] {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;

        let standing = gates(&repo, &model)
            .stand_in(plan_asking(key))
            .await
            .expect("stand in");

        assert_eq!(standing, None);
        assert_eq!(gate(&repo, key).await.stand_in, None);
    }
}

/// A stand-in that leaves the Gate to its owner keeps on the Gate how the
/// model read it: its pick, how sure it was and why, or that it did not
/// answer; a stand-in never asked keeps none.
#[tokio::test]
async fn a_declining_stand_in_keeps_why_on_the_gate() {
    let cases = [
        (
            FakeGateModel::picking(Some("approve"), 0.89),
            Some(GateModelReading {
                pick: Some(outcome("approve")),
                confidence: Some(89),
                reason: "The owner approved every plan like this one.".to_string(),
            }),
        ),
        (
            FakeGateModel::picking(None, 0.99),
            Some(GateModelReading {
                pick: None,
                confidence: Some(99),
                reason: "The owner approved every plan like this one.".to_string(),
            }),
        ),
        (
            FakeGateModel::failing(),
            Some(GateModelReading {
                pick: None,
                confidence: None,
                reason: String::new(),
            }),
        ),
    ];
    for (model, declined) in cases {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;

        gates(&repo, &model)
            .stand_in(plan_asking(key))
            .await
            .expect("stand in");

        assert_eq!(gate(&repo, key).await.stand_in_declined, declined);
    }

    // Off by default: the stand-in is not asked, so it declined nothing.
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(OrchestratorDefinition::DEFAULT), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.5);
    gates(&repo, &model)
        .stand_in(plan_asking(key))
        .await
        .expect("stand in");
    assert_eq!(gate(&repo, key).await.stand_in_declined, None);
}

/// The model is asked about the Run whose Gate it reads, so its usage
/// names that Run.
#[tokio::test]
async fn the_model_is_billed_for_the_run_it_reads() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.5);
    model.then(Some("approve"), 0.5);
    let service = gates(&repo, &model);

    service.stand_in(plan_asking(key)).await.expect("stand in");
    service
        .read_reply(reading(key, "Maybe"))
        .await
        .expect("read");

    assert_eq!(
        *model.runs.lock().expect("runs"),
        vec![Some(key.run_id), Some(key.run_id)]
    );
}

/// The owner is told, lightly, when the stand-in answers in their name:
/// a notice on the Run's task, not pushed, once however often the answer
/// is asked for, and done once the window closes. A Run with no task tells
/// its owner the same way, about no task, so the notice is filed under them.
#[tokio::test]
async fn the_owner_is_told_quietly_while_the_stand_in_s_window_is_open() {
    let repo = FakeRepo::default();
    let task = Uuid::from_u128(0x7a5c);
    repo.tasks.lock().expect("tasks").insert(task);
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), Some(task)).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let notifier = FakeNotifier::default();
    let service = gates(&repo, &model).with_notifier(notifier.clone());

    service.stand_in(plan_asking(key)).await.expect("stand in");
    service.stand_in(plan_asking(key)).await.expect("retried");

    let sent = notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 2, "a retry sends the same notice again");
    assert_eq!(sent[0], sent[1]);
    let notice = &sent[0];
    assert_eq!(notice.notification_id, stand_in_id(key.run_id, key.seq));
    assert_ne!(notice.notification_id, notice_id(key.run_id, key.seq));
    assert!(!notice.push);
    assert!(notice.metadata.notify_only);
    assert_eq!(notice.task_id, Some(task));
    // The answer by its label; the Run view says when the window closes.
    assert_eq!(
        notice.metadata.question,
        "The stand-in answered Approve for you; you can change it in the run until its \
         window closes. Approve this plan?"
    );
    assert!(notifier.done.lock().expect("done").is_empty());

    service
        .keep_stand_in(plan_keeping(key))
        .await
        .expect("keep");
    assert!(
        notifier
            .done
            .lock()
            .expect("done")
            .contains(&stand_in_id(key.run_id, key.seq))
    );

    // No task: told all the same, about the owner, and done the same way.
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let notifier = FakeNotifier::default();
    let service = gates(&repo, &model).with_notifier(notifier.clone());
    service.stand_in(plan_asking(key)).await.expect("stand in");
    let sent = notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let notice = &sent[0];
    assert_eq!(notice.notification_id, stand_in_id(key.run_id, key.seq));
    assert_eq!(notice.owner, julia());
    assert_eq!(notice.task_id, None);
    assert_eq!(notice.metadata.task_id, "");
    assert!(!notice.push);
    assert!(notice.metadata.notify_only);
    service
        .keep_stand_in(plan_keeping(key))
        .await
        .expect("keep");
    assert_eq!(
        *notifier.done.lock().expect("done"),
        [stand_in_id(key.run_id, key.seq)]
    );
}

/// The stand-in never answers a Gate kept for a person, a merge Gate, a
/// Gate its rules leave off (the default), a Tampered Run's, a Gate under
/// a Main Hold, or a Run's created before Runs kept rules; it asks the
/// model nothing.
#[tokio::test]
async fn the_stand_in_never_answers_what_stays_a_person_s() {
    let merge = OrchestratorDefinition {
        merge: true,
        stand_in: StandInKinds {
            plan: true,
            blockers: true,
        },
        ..OrchestratorDefinition::default()
    };
    struct Case {
        name: &'static str,
        rules: Option<OrchestratorDefinition>,
        asking: fn(StepKey) -> StandInAsking,
        tampered: bool,
        held: bool,
    }
    let cases = [
        Case {
            name: "person only",
            rules: Some(STAND_IN_ONLY),
            asking: |key| StandInAsking {
                person_only: true,
                ..plan_asking(key)
            },
            tampered: false,
            held: false,
        },
        Case {
            name: "merge",
            rules: Some(merge),
            asking: |key| StandInAsking {
                kind: RuleGateKind::Merge,
                ..plan_asking(key)
            },
            tampered: false,
            held: false,
        },
        Case {
            name: "off by default",
            rules: Some(OrchestratorDefinition::DEFAULT),
            asking: plan_asking,
            tampered: false,
            held: false,
        },
        Case {
            name: "tampered",
            rules: Some(STAND_IN_ONLY),
            asking: plan_asking,
            tampered: true,
            held: false,
        },
        Case {
            name: "main hold",
            rules: Some(STAND_IN_ONLY),
            asking: plan_asking,
            tampered: false,
            held: true,
        },
        Case {
            name: "no rules",
            rules: None,
            asking: plan_asking,
            tampered: false,
            held: false,
        },
    ];
    for case in cases {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, case.rules, None).await;
        if case.tampered {
            repo.control_file_changes.lock().expect("changes").insert(
                key.run_id,
                ControlFileChanges {
                    tampered: vec!["AGENTS.md".to_string()],
                    named: Vec::new(),
                    unlisted: false,
                },
            );
        }
        if case.held {
            repo.hold_main(&base(), "abc", "- test (failure)", Utc::now())
                .await
                .expect("hold");
        }
        let model = FakeGateModel::picking(Some("approve"), 0.99);

        let standing = gates(&repo, &model)
            .stand_in((case.asking)(key))
            .await
            .expect("stand in");

        assert_eq!(standing, None, "{}", case.name);
        assert!(model.asked().is_empty(), "{}", case.name);
    }
}

/// An agent's question is never the stand-in's, whatever the asking says.
#[tokio::test]
async fn the_stand_in_never_answers_an_agent_s_question() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    {
        let mut steps = repo.steps.lock().expect("steps");
        if let Some((_, step)) = steps
            .iter_mut()
            .find(|(id, step)| *id == key.run_id && step.seq == key.seq)
            && let Some(gate) = step.gate.as_mut()
        {
            gate.kind = factory_records::domain::models::GateKind::Question;
        }
    }
    let model = FakeGateModel::picking(Some("approve"), 0.99);

    let standing = gates(&repo, &model)
        .stand_in(plan_asking(key))
        .await
        .expect("stand in");

    assert_eq!(standing, None);
    assert!(model.asked().is_empty());
}

/// An owner who answers in the window overrides the stand-in: their
/// answer is the one the Gate keeps once the window ends. Left alone, the
/// stand-in's answer is kept as its own.
#[tokio::test]
async fn an_override_in_the_window_replaces_the_stand_in_s_answer() {
    let repo = FakeRepo::default();
    let overridden = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let left = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    model.then(Some("approve"), 0.95);
    let service = gates(&repo, &model);
    for key in [overridden, left] {
        service
            .stand_in(plan_asking(key))
            .await
            .expect("stand in")
            .expect("it answers");
    }

    repo.answer_gate(overridden, &outcome("revise"), None, &julia(), None, None)
        .await
        .expect("override");
    let kept = service
        .keep_stand_in(plan_keeping(overridden))
        .await
        .expect("keep");
    assert_eq!(kept, Some(outcome("revise")));
    let answer = gate(&repo, overridden).await.answer.expect("answered");
    assert!(!answer.by_stand_in);

    let kept = service
        .keep_stand_in(plan_keeping(left))
        .await
        .expect("keep");
    assert_eq!(kept, Some(outcome("approve")));
    assert!(
        gate(&repo, left)
            .await
            .answer
            .expect("answered")
            .by_stand_in
    );
}

/// Words in the stand-in's window end it, whatever the factory reads in
/// them: the notice that the stand-in answered is done once they are asked
/// back, as once they answer the Gate.
#[tokio::test]
async fn words_in_the_window_close_the_stand_in_s_notice() {
    for (choice, confidence) in [(None, 0.9), (Some("revise"), 0.95)] {
        let repo = FakeRepo::default();
        let task = Uuid::now_v7();
        repo.tasks.lock().expect("tasks").insert(task);
        let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), Some(task)).await;
        let model = FakeGateModel::picking(Some("approve"), 0.95);
        model.then(choice, confidence);
        let notifier = FakeNotifier::default();
        let service = gates(&repo, &model).with_notifier(notifier.clone());
        service.stand_in(plan_asking(key)).await.expect("stand in");

        service
            .read_reply(reading(key, "Hmm, not that"))
            .await
            .expect("read");

        assert!(
            notifier
                .done
                .lock()
                .expect("done")
                .contains(&stand_in_id(key.run_id, key.seq)),
            "{choice:?}"
        );
    }
}

/// A retried stand-in returns the answer it kept first, with what is left
/// of its window, and asks the model nothing again.
#[tokio::test]
async fn a_retried_stand_in_returns_the_answer_kept_first() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let service = gates(&repo, &model);
    let first = service.stand_in(plan_asking(key)).await.expect("stand in");

    let again = service.stand_in(plan_asking(key)).await.expect("again");

    assert_eq!(
        again.map(|standing| standing.answer),
        first.map(|standing: StandingIn| standing.answer)
    );
    assert_eq!(model.asked().len(), 1);
}

/// When the window ends on a Run that no longer lets the stand-in answer
/// (its branch now Tampered, its base under a Main Hold), the stand-in's
/// answer is taken back and the Gate waits for its owner.
#[tokio::test]
async fn a_run_that_changed_in_the_window_gives_the_gate_back_to_its_owner() {
    for change in ["tampered", "held"] {
        let repo = FakeRepo::default();
        let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
        let model = FakeGateModel::picking(Some("approve"), 0.95);
        let service = gates(&repo, &model);
        service
            .stand_in(plan_asking(key))
            .await
            .expect("stand in")
            .expect("it answers");
        if change == "tampered" {
            repo.control_file_changes.lock().expect("changes").insert(
                key.run_id,
                ControlFileChanges {
                    tampered: vec!["AGENTS.md".to_string()],
                    named: Vec::new(),
                    unlisted: false,
                },
            );
        } else {
            repo.hold_main(&base(), "abc", "- test (failure)", Utc::now())
                .await
                .expect("hold");
        }

        let kept = service
            .keep_stand_in(plan_keeping(key))
            .await
            .expect("keep");

        assert_eq!(kept, None, "{change}");
        let gate = gate(&repo, key).await;
        assert_eq!(gate.answer, None, "{change}");
        assert_eq!(gate.stand_in, None, "{change}");
    }
}

/// A Gate its Flow keeps for a person is given back when the window ends,
/// whatever the stand-in kept.
#[tokio::test]
async fn a_person_only_gate_is_never_kept_for_the_stand_in() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let service = gates(&repo, &model);
    service
        .stand_in(plan_asking(key))
        .await
        .expect("stand in")
        .expect("it answers");

    let kept = service
        .keep_stand_in(StandInKeeping {
            person_only: true,
            ..plan_keeping(key)
        })
        .await
        .expect("keep");

    assert_eq!(kept, None);
    assert_eq!(gate(&repo, key).await.answer, None);
}

/// The stand-in reads the owner's past answers to the same Flow's entry,
/// not an agent's question asked there, nor another Flow's.
#[tokio::test]
async fn the_stand_in_reads_past_answers_of_the_same_flow_and_kind() {
    let repo = FakeRepo::default();
    let earlier = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    repo.answer_gate(
        earlier,
        &outcome("revise"),
        Some("Same flow"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("past answer");
    // A Run of another Flow, at the same entry.
    let other = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    repo.flows
        .lock()
        .expect("flows")
        .insert(other.run_id, "plan".to_string());
    repo.answer_gate(
        other,
        &outcome("approve"),
        Some("Other flow"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("other flow's answer");
    // An agent's question asked at the entry.
    {
        let mut steps = repo.steps.lock().expect("steps");
        if let Some((_, step)) = steps
            .iter_mut()
            .find(|(id, step)| *id == earlier.run_id && step.seq == 1)
        {
            step.end = Some(factory_records::domain::models::StepEnd {
                outcome: outcome("question"),
                output: "Which column?".to_string(),
                ended_at: Utc::now(),
            });
            step.entry = EntryId::try_from("plan-review".to_string()).expect("valid");
            step.gate = Some(factory_records::domain::models::Gate {
                kind: factory_records::domain::models::GateKind::Question,
                question: "Which column?".to_string(),
                answers: vec![outcome("answer")],
                opened_at: Utc::now(),
                answer: Some(factory_records::domain::models::GateAnswer {
                    answer: outcome("answer"),
                    note: Some("A question's answer".to_string()),
                    user_id: Some(julia()),
                    rule: None,
                    approved_control_files: None,
                    reply_confidence: None,
                    by_stand_in: false,
                    agent_session_id: None,
                    answered_at: Utc::now(),
                }),
                asked_back: None,
                stand_in: None,
                stand_in_declined: None,
            });
        }
    }
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);

    gates(&repo, &model)
        .stand_in(plan_asking(key))
        .await
        .expect("stand in");

    let prompt = &model.asked()[0].prompt;
    assert!(prompt.contains("Same flow"));
    assert!(!prompt.contains("Other flow"));
    assert!(!prompt.contains("A question's answer"));
}

/// A Run without a task sent a notice that the stand-in answered too, so
/// words in its window close it, as a Run on a task's.
#[tokio::test]
async fn words_in_the_window_of_a_run_without_a_task_close_its_notice() {
    let repo = FakeRepo::default();
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    model.then(None, 0.9);
    let notifier = FakeNotifier::default();
    let service = gates(&repo, &model).with_notifier(notifier.clone());
    service.stand_in(plan_asking(key)).await.expect("stand in");

    service
        .read_reply(reading(key, "Hmm, not that"))
        .await
        .expect("read");

    assert_eq!(
        *notifier.done.lock().expect("done"),
        [stand_in_id(key.run_id, key.seq)]
    );
}

/// Plan Gates were asked at `plan` before plans were reviewed: the owner's
/// answers there are past answers to the plan Gate `plan-review` asks.
#[tokio::test]
async fn the_stand_in_reads_answers_to_plan_gates_asked_before_plan_review() {
    let repo = FakeRepo::default();
    let earlier = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    repo.answer_gate(
        earlier,
        &outcome("revise"),
        Some("Old plan gate"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("past answer");
    {
        let mut steps = repo.steps.lock().expect("steps");
        if let Some((_, step)) = steps
            .iter_mut()
            .find(|(id, step)| *id == earlier.run_id && step.seq == earlier.seq)
        {
            step.entry = EntryId::try_from("plan".to_string()).expect("valid");
        }
    }
    let key = run_at_plan_gate(&repo, Some(STAND_IN_ONLY), None).await;
    let model = FakeGateModel::picking(Some("approve"), 0.95);

    gates(&repo, &model)
        .stand_in(plan_asking(key))
        .await
        .expect("stand in");

    assert!(model.asked()[0].prompt.contains("Old plan gate"));
}

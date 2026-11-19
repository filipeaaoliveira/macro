use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{AgentName, NewRun, RunId, RunInsert};
use crate::domain::ports::RunRepository;
use crate::domain::standing_acts::{
    StandingAct, StandingActRepository, StandingActor, StandingRoleName,
};
use crate::domain::standing_rules::RunRules;

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

fn teo() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|teo@example.com".to_string()).expect("user")
}

/// A new Run of Julia's.
async fn run(repo: &PgRunRepository) -> RunId {
    let julia = julia();
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan dark mode",
            user_id: &julia,
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    id
}

fn gatekeeper(session_id: Option<Uuid>, bot_id: Uuid) -> StandingActor {
    StandingActor {
        session_id,
        bot_id,
        agent: AgentName::try_from("primary".to_string()).expect("agent"),
        standing_role: StandingRoleName::try_from("gatekeeper".to_string()).expect("role"),
    }
}

/// The bot of an agent session is read for its owner only.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_session_reads_as_its_bot_for_its_owner_only(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let session = Uuid::now_v7();
    let bot = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO agent_session (id, owner_id, bot_id, model, harness, workspace)
         VALUES ($1, 'macro|julia@example.com', $2, 'test', 'test', '/workspace')",
        session,
        bot,
    )
    .execute(&pool)
    .await
    .expect("session");

    let owners = repo.session_bot(session, &julia()).await.expect("read");
    let others = repo.session_bot(session, &teo()).await.expect("read");
    let unknown = repo
        .session_bot(Uuid::now_v7(), &julia())
        .await
        .expect("read");

    assert_eq!(owners, Some(bot));
    assert_eq!(others, None);
    assert_eq!(unknown, None);
}

/// A start and an answer done again keep the first; each link is kept; a
/// Run's acts read in order with who did them.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn standing_acts_keep_one_start_and_answer_and_every_link(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = run(&repo).await;
    let other_run = run(&repo).await;
    let first = gatekeeper(Some(Uuid::now_v7()), Uuid::now_v7());
    let second = gatekeeper(Some(Uuid::now_v7()), Uuid::now_v7());

    for (act, seq, actor) in [
        (StandingAct::Start, None, &first),
        (StandingAct::Start, None, &second),
        (StandingAct::Link, None, &first),
        (StandingAct::Link, None, &first),
        (StandingAct::AnswerPlan, Some(2), &first),
        (StandingAct::AnswerPlan, Some(2), &second),
    ] {
        repo.record_standing_act(run_id, act, seq, actor)
            .await
            .expect("record");
    }

    let acts = repo
        .runs_standing_acts(&[run_id, other_run])
        .await
        .expect("read");

    let kept = acts.get(&run_id).expect("the run's acts");
    let read: Vec<_> = kept
        .iter()
        .map(|record| (record.act, record.seq, record.bot_id))
        .collect();
    assert_eq!(
        read,
        [
            (StandingAct::Start, None, first.bot_id),
            (StandingAct::Link, None, first.bot_id),
            (StandingAct::Link, None, first.bot_id),
            (StandingAct::AnswerPlan, Some(2), first.bot_id),
        ]
    );
    assert_eq!(kept[0].standing_role.as_str(), "gatekeeper");
    assert_eq!(kept[0].agent.as_str(), "primary");
    assert_eq!(kept[0].session_id, first.session_id);
    assert!(!acts.contains_key(&other_run));
}

/// An act from a bot's code that ran in no agent session keeps its bot,
/// with no session.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_standing_act_from_no_session_keeps_its_bot(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = run(&repo).await;
    let sessionless = gatekeeper(None, Uuid::now_v7());

    repo.record_standing_act(run_id, StandingAct::Cancel, None, &sessionless)
        .await
        .expect("record");

    let acts = repo.runs_standing_acts(&[run_id]).await.expect("read");
    let kept = acts.get(&run_id).expect("the run's acts");
    let read: Vec<_> = kept
        .iter()
        .map(|record| (record.act, record.bot_id, record.session_id))
        .collect();
    assert_eq!(read, [(StandingAct::Cancel, sessionless.bot_id, None)]);
}

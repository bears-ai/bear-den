//! Browser projections keep lifecycle, execution and publication authority separate.

use den_core::ids::{BearId, UserId};
use den_docket::{
    preflight_dispatch, work_runs::WorkExecutionTarget, DispatchPreflight, DocketCommitPolicy,
    DocketJobRow, DurableResultKind,
};
use den_service::bears::hats;
use serde::Serialize;
use uuid::Uuid;

use crate::errors::CustomError;

pub(super) fn browser_preflight(
    policy: Option<DocketCommitPolicy>,
    branch: Option<&str>,
) -> DispatchPreflight {
    preflight_dispatch(
        &WorkExecutionTarget::Sandbox,
        DurableResultKind::RepositoryChanges,
        policy,
        branch,
    )
}

pub(super) fn stored_policy(
    value: Option<&str>,
) -> Result<Option<DocketCommitPolicy>, CustomError> {
    value
        .map(|value| super::parse_docket_enum("output policy", value))
        .transpose()
}

pub(super) fn require_supported_policy(policy: DocketCommitPolicy) -> Result<(), CustomError> {
    if !browser_preflight(Some(policy), None).dispatchable {
        return Err(CustomError::ValidationError(
            "Browser dispatch delivers repository changes and currently requires Publish after each task. Publish to the job branch and No source changes expected are historical choices, not supported new selections.".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BrowserDispatchBlocker {
    RepositoryUnavailable,
    MutationForbidden,
    OutputPolicy,
    ResponsibilityUnavailable,
    AccountUnavailable,
}

impl BrowserDispatchBlocker {
    pub(super) fn message(&self) -> &'static str {
        match self {
            Self::RepositoryUnavailable => {
                "Dispatch repository is not currently assigned to this Bear"
            }
            Self::MutationForbidden => {
                "The dispatch repository forbids source changes for this Job"
            }
            Self::OutputPolicy => "Output policy cannot dispatch repository changes here",
            Self::ResponsibilityUnavailable => {
                "Work responsibility does not currently permit every Job assignment"
            }
            Self::AccountUnavailable => {
                "Repository authentication is unavailable or explicitly read-only"
            }
        }
    }
}

pub(super) struct BrowserReadiness {
    pub repository_available: bool,
    pub account_available: bool,
    pub hat_available: bool,
    pub blocker: Option<BrowserDispatchBlocker>,
}

/// Match the dispatcher's first Git assignment, rather than inferring execution from
/// a display-only primary repository. Hat eligibility covers every canonical assignment.
pub(super) async fn browser_readiness(
    pool: &sqlx::PgPool,
    job: &DocketJobRow,
) -> Result<BrowserReadiness, CustomError> {
    let repository = sqlx::query!(
        r#"SELECT a.work_surface_id, a.mutation_policy,
            EXISTS (SELECT 1 FROM work_surface_bears b WHERE b.surface_id = a.work_surface_id AND b.bear_id = $2) AS "assigned!"
        FROM job_work_surface_assignments a JOIN work_surfaces s ON s.id = a.work_surface_id
        WHERE a.job_id = $1 AND s.kind = 'git_workspace' ORDER BY a.created_at LIMIT 1"#,
        job.id,
        job.bear_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(den_core::DenError::from)?;
    let repository_available = repository.as_ref().is_some_and(|row| row.assigned);
    let mutation_forbidden = repository
        .as_ref()
        .map(|row| {
            super::parse_docket_enum::<den_docket::MutationPolicy>(
                "mutation policy",
                &row.mutation_policy,
            )
        })
        .transpose()?
        == Some(den_docket::MutationPolicy::Forbidden);
    let account_available = if let Some(row) = repository.as_ref().filter(|row| row.assigned) {
        super::connection_view::publication_available(pool, row.work_surface_id).await?
    } else {
        false
    };
    let hat_available = hats::bindings::eligible_job_hat(pool, BearId::new(job.bear_id), job.id)
        .await?
        .is_some();
    let preflight = browser_preflight(
        stored_policy(job.commit_policy.as_deref())?,
        job.work_branch.as_deref(),
    );
    let blocker = if !repository_available {
        Some(BrowserDispatchBlocker::RepositoryUnavailable)
    } else if mutation_forbidden {
        Some(BrowserDispatchBlocker::MutationForbidden)
    } else if !preflight.dispatchable {
        Some(BrowserDispatchBlocker::OutputPolicy)
    } else if !hat_available {
        Some(BrowserDispatchBlocker::ResponsibilityUnavailable)
    } else if !account_available {
        Some(BrowserDispatchBlocker::AccountUnavailable)
    } else {
        None
    };
    Ok(BrowserReadiness {
        repository_available,
        account_available,
        hat_available,
        blocker,
    })
}

#[derive(Serialize)]
pub(super) struct RepositoryChoice {
    pub id: Uuid,
    pub name: String,
    pub default_ref: String,
}

#[derive(Serialize)]
pub(super) struct HatChoice {
    pub id: den_core::ids::HatId,
    pub name: String,
    pub short_summary: Option<String>,
    pub surface_ids: Vec<Uuid>,
}

#[derive(Serialize)]
pub(super) struct RepositoryLink {
    pub route_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Serialize)]
pub(super) struct JobListView {
    pub display_id: String,
    pub route_id: String,
    pub full_id: Uuid,
    pub title: String,
    pub bear_slug: String,
    pub status: String,
    pub repositories: Vec<RepositoryLink>,
    pub docket_run_id: Option<String>,
    pub docket_run_state: Option<String>,
    pub run_count: i64,
    pub dispatch_blocker: Option<&'static str>,
}

pub(super) async fn job_list(
    pool: &sqlx::PgPool,
    jobs: Vec<DocketJobRow>,
    viewer: UserId,
    bear_slug: &str,
    show_completed: bool,
    counts: &std::collections::HashMap<Uuid, i64>,
) -> Result<Vec<JobListView>, CustomError> {
    let ids: Vec<_> = jobs.iter().map(|job| job.id).collect();
    let lifecycle = sqlx::query!(
        "SELECT j.id AS job_id, r.state FROM bear_jobs j JOIN bear_job_runs r ON r.id = j.current_run_id AND r.job_id = j.id WHERE j.id = ANY($1)",
        &ids,
    ).fetch_all(pool).await.map_err(den_core::DenError::from)?;
    let repositories = sqlx::query!(
        r#"SELECT a.job_id, s.id,
            CASE WHEN access.can_manage OR EXISTS (
                SELECT 1 FROM work_surface_bears b JOIN user_bear membership ON membership.bear_id = b.bear_id
                WHERE b.surface_id = s.id AND b.bear_id = j.bear_id AND membership.user_id = $2
            ) THEN s.name ELSE NULL END AS "name?", access.can_manage AS "can_manage!"
        FROM job_work_surface_assignments a JOIN bear_jobs j ON j.id = a.job_id
        JOIN work_surfaces s ON s.id = a.work_surface_id
        CROSS JOIN LATERAL (SELECT EXISTS (SELECT 1 FROM users u WHERE u.id = $2 AND u.is_admin)
            OR EXISTS (SELECT 1 FROM work_surface_managers m WHERE m.surface_id = s.id AND m.user_id = $2) AS can_manage) access
        WHERE a.job_id = ANY($1) ORDER BY a.created_at, s.id"#,
        &ids,
        viewer.get(),
    ).fetch_all(pool).await.map_err(den_core::DenError::from)?;
    let mut views = Vec::new();
    for job in jobs
        .into_iter()
        .filter(|job| show_completed || job.status != "completed")
    {
        let readiness = browser_readiness(pool, &job).await?;
        views.push(JobListView {
            dispatch_blocker: readiness
                .blocker
                .as_ref()
                .map(BrowserDispatchBlocker::message),
            display_id: super::uuid_hex_prefix(job.id, super::DISPLAY_ID_HEX_LEN),
            route_id: super::route_id(job.id),
            full_id: job.id,
            title: job.goal.clone(),
            bear_slug: bear_slug.to_owned(),
            status: job.status,
            repositories: repositories
                .iter()
                .filter(|row| row.job_id == job.id)
                .map(|row| RepositoryLink {
                    route_id: row.can_manage.then(|| super::route_id(row.id)),
                    name: row.name.clone(),
                })
                .collect(),
            docket_run_id: job
                .current_run_id
                .map(|id| super::uuid_hex_prefix(id, super::DISPLAY_ID_HEX_LEN)),
            docket_run_state: lifecycle
                .iter()
                .find(|row| row.job_id == job.id)
                .map(|row| row.state.clone()),
            run_count: counts.get(&job.id).copied().unwrap_or_default(),
        });
    }
    Ok(views)
}

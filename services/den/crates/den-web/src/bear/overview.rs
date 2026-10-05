//! Member-safe Overview projections; raw Bear inspection stays in settings.

use den_core::ids::{BearId, HatId};
use den_docket::{DocketJobListFilter, PgDocketService};
use den_service::bears::hats;
use serde::Serialize;
use uuid::Uuid;

use crate::{errors::CustomError, AppState};

#[derive(Serialize)]
pub(crate) struct HatSummary {
    pub id: HatId,
    pub name: String,
    pub short_summary: Option<String>,
    pub work_enabled: bool,
}

#[derive(Serialize)]
pub(crate) struct JobSummary {
    id: Uuid,
    goal: String,
    status: String,
}

#[derive(Serialize)]
pub(crate) struct OverviewSummary {
    hats: Vec<HatSummary>,
    jobs: Vec<JobSummary>,
}

pub(crate) async fn summary(
    state: &AppState,
    bear_id: BearId,
    user_id: i32,
    can_manage: bool,
) -> Result<OverviewSummary, CustomError> {
    let hats = hats::list_hats(state.sqlx_pool(), bear_id)
        .await?
        .into_iter()
        .map(|hat| HatSummary {
            id: hat.id,
            name: hat.name,
            short_summary: hat.short_summary,
            work_enabled: hat.work_enabled,
        })
        .collect();
    let jobs = PgDocketService::from_pool(state.sqlx_pool())
        .list_jobs_for_viewer(
            bear_id.as_uuid(),
            user_id,
            can_manage,
            DocketJobListFilter {
                limit: 5,
                ..Default::default()
            },
        )
        .await?
        .into_iter()
        .map(|job| JobSummary {
            id: job.id,
            goal: job.goal,
            status: job.status,
        })
        .collect();
    Ok(OverviewSummary { hats, jobs })
}

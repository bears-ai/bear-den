//! Authorized browse rendering also serves draft-preserving action failures.

use super::{
    bear_nav_context, bears_db, context, group_rank, hats, inspection, json, memory_proposals,
    path_group_label, store, web, AppState, BearId, BrowseQuery, CreateMemoryProposal, CustomError,
    HashMap, HatId, MemoryDeleteForm, MemoryLibraryViewer, PathGroup, Response,
    RuntimeContextLabel,
};

pub(super) async fn render(
    state: &AppState,
    auth: crate::auth_backend::AuthSession,
    bear: &den_service::bears::Bear,
    can_manage_bear: bool,
    query: BrowseQuery,
    form: Option<MemoryDeleteForm>,
) -> Result<Response, CustomError> {
    let viewer = MemoryLibraryViewer::resolve(state, bear.id, can_manage_bear).await?;
    let mut errors = Vec::new();
    let summaries = inspection::read_result(
        viewer.browse(&state.memory_stores, bear.id).await,
        "Memory paths",
        &mut errors,
    );
    let paths_available = summaries.is_some();
    let hat_names: HashMap<HatId, String> = if can_manage_bear {
        HashMap::new()
    } else {
        hats::list_hats(state.sqlx_pool(), BearId::new(bear.id))
            .await?
            .into_iter()
            .map(|hat| (hat.id, hat.name))
            .collect()
    };
    let mut groups: Vec<PathGroup> = Vec::new();
    for summary in summaries.into_iter().flatten() {
        let label = if can_manage_bear {
            path_group_label(&summary.logical_path)
        } else {
            match store::MemoryScopeType::parse(&summary.scope_type) {
                Some(store::MemoryScopeType::Shared) => "Bear-wide".to_string(),
                Some(store::MemoryScopeType::Hat) => {
                    let Some(name) = summary.scope_hat_id.and_then(|id| hat_names.get(&id)) else {
                        continue;
                    };
                    format!("Hat: {name}")
                }
                _ => continue,
            }
        };
        if let Some(group) = groups.iter_mut().find(|group| group.label == label) {
            group.paths.push(summary);
        } else {
            groups.push(PathGroup {
                label,
                paths: vec![summary],
            });
        }
    }
    groups.sort_by(|a, b| {
        group_rank(&a.label)
            .cmp(&group_rank(&b.label))
            .then(a.label.cmp(&b.label))
    });
    let unavailable_selections: Vec<_> = form
        .as_ref()
        .into_iter()
        .flat_map(|form| &form.paths)
        .filter(|path| {
            !groups
                .iter()
                .flat_map(|group| &group.paths)
                .any(|row| row.logical_path.as_str() == path.as_str())
        })
        .cloned()
        .collect();
    web::render_template(
        state,
        "bear/memory/browse.html",
        auth,
        context! {
            groups, paths_available, inspection_errors => errors,
            delete_notice => query.deleted,
            review_notice => query.review_requested,
            delete_error => query.error,
            form, unavailable_selections, can_manage_bear,
            native_runtime => true,
            ..bear_nav_context(bear, "memory"),
        },
    )
    .await
}

pub(super) enum Saved {
    ReviewRequested,
    Deleted(usize),
}

pub(super) async fn apply(
    state: &AppState,
    bear: &den_service::bears::Bear,
    form: &MemoryDeleteForm,
) -> Result<Saved, CustomError> {
    let role = form
        .role
        .trim()
        .parse::<RuntimeContextLabel>()
        .map_err(CustomError::ValidationError)?;
    let mut paths: Vec<_> = form
        .paths
        .iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err(CustomError::ValidationError(
            "Select at least one memory path.".into(),
        ));
    }
    match form.action.as_deref().unwrap_or("delete").trim() {
        "request_review" => {
            memory_proposals::create(state.sqlx_pool(), CreateMemoryProposal {
                bear_id: bear.id,
                source_profile: role,
                source_agent_id: bears_db::profile_binding_id(state.sqlx_pool(), bear.id, role).await?,
                source_paths: paths,
                source_refs: json!([]),
                suggested_action: form.suggested_action.as_deref().map(str::trim)
                    .filter(|text| !text.is_empty()).unwrap_or("unspecified"),
                target_ref: None,
                title: form.review_title.as_deref().map(str::trim)
                    .filter(|text| !text.is_empty()).unwrap_or("Review selected memory"),
                summary: form.review_summary.as_deref().map(str::trim)
                    .filter(|text| !text.is_empty()).unwrap_or("Selected memory paths were marked for Reflection/curate review from the Bear memory UI."),
                rationale: form.review_rationale.as_deref().map(str::trim).unwrap_or(""),
                proposed_content: None, proposed_patch: None, refs: json!({}),
                sensitivity: form.sensitivity.as_deref().map(str::trim)
                    .filter(|text| !text.is_empty()).unwrap_or("normal"),
                requires_human: form.requires_human.as_deref() == Some("on"),
                project_to_conversation: true,
            }).await?;
            Ok(Saved::ReviewRequested)
        }
        "delete" => {
            if form.confirm.trim() != role.as_str() && form.confirm.trim() != bear.slug {
                return Err(CustomError::ValidationError(
                    "Type the profile name or Bear slug to confirm deletion.".into(),
                ));
            }
            let store = state.memory_stores.store_for_bear(bear.id).await?;
            let mut deleted = 0;
            for path in paths {
                // Per-Bear SQLite is not described by the workspace's Postgres SQLx cache.
                let result = sqlx::query(
                    "DELETE FROM memory_records WHERE bear_id = ? AND scope_profile = ? AND logical_path = ?",
                ).bind(bear.id.to_string()).bind(role.as_str()).bind(path)
                    .execute(store.pool()).await.map_err(|err| CustomError::System(format!(
                        "Deletion stopped after {deleted} selected paths were removed: {err}. Inspect the library before retrying.")))?;
                deleted += usize::from(result.rows_affected() > 0);
            }
            Ok(Saved::Deleted(deleted))
        }
        _ => Err(CustomError::ValidationError(
            "Choose Request review or Delete selected.".into(),
        )),
    }
}

pub(super) async fn error(
    state: &AppState,
    auth: crate::auth_backend::AuthSession,
    bear: &den_service::bears::Bear,
    form: MemoryDeleteForm,
    error: String,
) -> Result<Response, CustomError> {
    render(
        state,
        auth,
        bear,
        true,
        BrowseQuery {
            deleted: None,
            review_requested: None,
            error: Some(error),
        },
        Some(form),
    )
    .await
}

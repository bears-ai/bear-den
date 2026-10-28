//! Short-lived, body-free action snapshots. Nonces are locators, never identity authority.
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use den_core::ids::{BearId, UserId};
use den_service::personal_memory::{
    CandidateReviewDecision, CandidateReviewId, PersonalCandidateId, PersonalFactId,
    PersonalMemoryOwner,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_AGE_SECONDS: i64 = 15 * 60;
pub(super) const MAX_PAGES_PER_SCOPE: usize = 4;
pub(super) const MAX_PAGES_PER_ACTOR: usize = 8;
pub(super) const MAX_PAGES: usize = 64;
// At most four actions for each of 100 candidates plus capture/request/retraction/erasure.
const MAX_ACTIONS_PER_PAGE: usize = 512;
pub(super) const UNAVAILABLE: &str = "This form expired, changed, or belongs to another session. Refresh About you and review the current state.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct ActionNonce(pub Uuid);

impl ActionNonce {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operation {
    Capture,
    Request,
    Publish,
    Review,
    Retract,
    Erase,
}

#[derive(Clone, Copy)]
pub(super) enum Intent {
    Capture(bool),
    Request {
        request_id: Uuid,
    },
    Publish {
        candidate_id: PersonalCandidateId,
        expected_head: Option<PersonalFactId>,
    },
    Review {
        candidate_id: PersonalCandidateId,
        decision: CandidateReviewDecision,
        expected_review: Option<CandidateReviewId>,
    },
    Retract {
        expected_head: PersonalFactId,
    },
    ErasePreferredNames {
        request_id: Uuid,
    },
}

impl Intent {
    fn operation(self) -> Operation {
        match self {
            Self::Capture(_) => Operation::Capture,
            Self::Request { .. } => Operation::Request,
            Self::Publish { .. } => Operation::Publish,
            Self::Review { .. } => Operation::Review,
            Self::Retract { .. } => Operation::Retract,
            Self::ErasePreferredNames { .. } => Operation::Erase,
        }
    }
}

pub(super) fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[derive(Clone)]
pub(super) struct Scope {
    bear_id: BearId,
    actor: UserId,
    session_id: String,
    issued_at: i64,
}

impl Scope {
    pub fn new(bear_id: BearId, actor: UserId, session_id: String) -> Self {
        Self {
            bear_id,
            actor,
            session_id,
            issued_at: now(),
        }
    }

    fn same_context(&self, other: &Self) -> bool {
        self.bear_id == other.bear_id
            && self.actor == other.actor
            && self.session_id == other.session_id
    }

    fn fresh(&self, at: i64) -> bool {
        at.checked_sub(self.issued_at)
            .is_some_and(|age| (0..MAX_AGE_SECONDS).contains(&age))
    }

    pub fn authorize(&self, bear_id: BearId, actor: UserId, session_id: &str, at: i64) -> bool {
        self.bear_id == bear_id
            && self.actor == actor
            && self.session_id == session_id
            && self.fresh(at)
    }
}

#[derive(Clone)]
pub(super) struct PendingAction {
    pub owner: Arc<PersonalMemoryOwner>,
    pub intent: Intent,
}

struct RegisteredPage {
    scope: Scope,
    owner: Arc<PersonalMemoryOwner>,
    actions: HashMap<ActionNonce, Intent>,
}

#[derive(Clone, Default)]
pub(super) struct Registry(Arc<Mutex<VecDeque<RegisteredPage>>>);

impl Registry {
    pub fn register(
        &self,
        scope: Scope,
        owner: PersonalMemoryOwner,
        actions: Vec<(ActionNonce, Intent)>,
    ) -> Result<(), &'static str> {
        let at = now();
        if !scope.fresh(at) || actions.is_empty() || actions.len() > MAX_ACTIONS_PER_PAGE {
            return Err(UNAVAILABLE);
        }
        let mut pages = self.0.lock().map_err(|_| UNAVAILABLE)?;
        pages.retain(|page| page.scope.fresh(at));
        // Refresh pressure retires this context's own oldest pages first. The
        // account quota also prevents opening more sessions/Bears to fill the queue.
        while pages
            .iter()
            .filter(|page| page.scope.same_context(&scope))
            .count()
            >= MAX_PAGES_PER_SCOPE
        {
            let oldest = pages
                .iter()
                .position(|page| page.scope.same_context(&scope))
                .ok_or(UNAVAILABLE)?;
            pages.remove(oldest);
        }
        while pages
            .iter()
            .filter(|page| page.scope.actor == scope.actor)
            .count()
            >= MAX_PAGES_PER_ACTOR
        {
            let oldest = pages
                .iter()
                .position(|page| page.scope.actor == scope.actor)
                .ok_or(UNAVAILABLE)?;
            pages.remove(oldest);
        }
        // New inspections remain usable at global capacity. Only an old page is
        // retired; its tokens fail closed rather than acquiring fresh request IDs.
        if pages.len() >= MAX_PAGES {
            pages.pop_front();
        }
        pages.push_back(RegisteredPage {
            scope,
            owner: Arc::new(owner),
            actions: actions.into_iter().collect(),
        });
        Ok(())
    }

    pub fn authorize(
        &self,
        nonce: ActionNonce,
        operation: Operation,
        bear_id: BearId,
        actor: UserId,
        session_id: &str,
    ) -> Result<PendingAction, &'static str> {
        let at = now();
        let mut pages = self.0.lock().map_err(|_| UNAVAILABLE)?;
        pages.retain(|page| page.scope.fresh(at));
        let page = pages
            .iter()
            .find(|page| page.actions.contains_key(&nonce))
            .ok_or(UNAVAILABLE)?;
        let intent = *page.actions.get(&nonce).ok_or(UNAVAILABLE)?;
        if intent.operation() != operation || !page.scope.authorize(bear_id, actor, session_id, at)
        {
            return Err(UNAVAILABLE);
        }
        // Retained-page retries keep the same request UUID. Service revisions/CAS
        // own replay safety, including capture consent changed off and back on.
        Ok(PendingAction {
            owner: page.owner.clone(),
            intent,
        })
    }
}

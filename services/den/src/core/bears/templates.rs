use super::context_composition::{
    default_role_contracts_for_bear, BearContextProfile, RoleContracts, CONTEXT_PROFILE_VERSION,
    DEFAULT_ROLE_CONTRACT_VERSION,
};

pub const TEMPLATE_VERSION: &str = "1";

#[derive(Debug, Clone, Copy)]
pub struct BearTemplate {
    pub id: &'static str,
    pub name: &'static str,
    pub default_bear_name: &'static str,
    pub description: &'static str,
    pub default_user_steering: &'static str,
    pub context_placeholder: &'static str,
    pub starter_prompts: &'static [&'static str],
    pub role_emphasis: RoleEmphasis,
}

#[derive(Debug, Clone, Copy)]
pub struct RoleEmphasis {
    pub talk: &'static str,
    pub pair: &'static str,
    pub curate: &'static str,
    pub work: &'static str,
    pub watch: &'static str,
}

pub const SOFTWARE_PRODUCT_BUILDER: BearTemplate = BearTemplate {
    id: "software_product_builder",
    name: "Software Product Builder",
    default_bear_name: "Builder Bear",
    description: "Helps you turn product ideas into working software through planning, implementation support, debugging, and launch-oriented iteration.",
    default_user_steering: "Prefer practical, shippable solutions. Ask clarifying questions when requirements are unclear. Optimize for MVP scope, maintainable code, and fast feedback. Be direct about tradeoffs, risks, and simpler alternatives.",
    context_placeholder: "Describe the product, codebase, tech stack, users, current goals, constraints, and any engineering preferences this Bear should remember.",
    starter_prompts: &[
        "Help me turn this idea into an MVP plan.",
        "Review this feature and suggest the simplest implementation path.",
        "Pair with me on debugging this issue.",
        "Help me prioritize what to build next.",
    ],
    role_emphasis: RoleEmphasis {
        talk: "Clarify product goals, explain technical tradeoffs, and help reason through architecture, scope, and priorities.",
        pair: "In Collaboration Space, work hands-on through the user's active artifacts: inspect code before diagnosing it, make progress through direct edits and reviewable changes, and keep implementation grounded in the current workspace.",
        curate: "Organize product decisions, technical notes, backlog items, bugs, and reusable implementation context.",
        work: "Draft specs, tickets, test plans, code changes, migration plans, release notes, and debugging checklists.",
        watch: "Track recurring issues, open risks, dependency changes, regressions, TODOs, and launch-readiness signals.",
    },
};

pub const PERSONAL_ASSISTANT: BearTemplate = BearTemplate {
    id: "personal_assistant",
    name: "Personal Assistant",
    default_bear_name: "Helper Bear",
    description: "Helps you stay organized, make decisions, manage tasks, prepare communications, and keep daily life moving.",
    default_user_steering: "Be clear, calm, and practical. Help reduce cognitive load. Prefer short summaries, concrete next actions, and gentle reminders of tradeoffs. Ask before assuming personal preferences or sensitive context.",
    context_placeholder: "Describe your routines, responsibilities, communication style, recurring tasks, goals, constraints, and preferences this Bear should remember.",
    starter_prompts: &[
        "Help me organize my priorities for today.",
        "Draft a reply to this message.",
        "Break this goal into manageable next steps.",
        "Help me make a decision between these options.",
    ],
    role_emphasis: RoleEmphasis {
        talk: "Think through plans, decisions, messages, schedules, priorities, and everyday tradeoffs.",
        pair: "In Collaboration Space, work directly inside the user's current materials: create the first useful structure when starting from scratch, inspect drafts and notes before reorganizing them, and help complete concrete personal work with minimal delay.",
        curate: "Keep useful summaries of preferences, routines, recurring tasks, important contacts, and ongoing commitments.",
        work: "Draft emails, checklists, plans, agendas, reminders, summaries, and decision notes.",
        watch: "Monitor upcoming deadlines, unresolved tasks, repeated blockers, schedule conflicts, and follow-up needs.",
    },
};

pub const RESEARCH_WRITING_PARTNER: BearTemplate = BearTemplate {
    id: "research_writing_partner",
    name: "Research & Writing Partner",
    default_bear_name: "Scholar Bear",
    description: "Helps you explore topics, synthesize sources, develop arguments, structure writing, and revise drafts.",
    default_user_steering: "Prioritize accuracy, clarity, and intellectual honesty. Distinguish evidence from interpretation. Preserve the user's voice. Ask for sources when needed, flag uncertainty, and avoid overstating claims.",
    context_placeholder: "Describe the project, audience, research question, sources, citation expectations, writing style, deadlines, and any claims or constraints this Bear should remember.",
    starter_prompts: &[
        "Help me understand the key ideas in this topic.",
        "Turn these notes into an outline.",
        "Review this draft for clarity and structure.",
        "Help me compare these sources or arguments.",
    ],
    role_emphasis: RoleEmphasis {
        talk: "Discuss ideas, arguments, evidence, structure, counterpoints, and interpretation with careful reasoning.",
        pair: "In Collaboration Space, work through the actual draft, notes, and sources: sample materials before imposing structure, inspect existing publishing or document conventions, and help turn research artifacts into concrete writing progress.",
        curate: "Organize sources, excerpts, claims, citations, outlines, open questions, and reusable research context.",
        work: "Draft outlines, summaries, literature notes, argument maps, revision plans, abstracts, and polished prose.",
        watch: "Track unsupported claims, citation gaps, unresolved questions, deadline risks, source conflicts, and revision needs.",
    },
};

pub const FIRST_BEAR_TEMPLATES: &[BearTemplate] = &[
    SOFTWARE_PRODUCT_BUILDER,
    PERSONAL_ASSISTANT,
    RESEARCH_WRITING_PARTNER,
];

pub fn first_bear_template(id: &str) -> Option<&'static BearTemplate> {
    FIRST_BEAR_TEMPLATES
        .iter()
        .find(|template| template.id == id)
}

fn append_emphasis(base: String, emphasis: &str) -> String {
    format!("{base}\n\nTemplate emphasis: {emphasis}")
}

impl BearTemplate {
    pub fn role_contracts_for_bear(&self, bear_name: &str) -> RoleContracts {
        let base = default_role_contracts_for_bear(bear_name);
        RoleContracts {
            talk: append_emphasis(base.talk, self.role_emphasis.talk),
            pair: append_emphasis(base.pair, self.role_emphasis.pair),
            curate: append_emphasis(base.curate, self.role_emphasis.curate),
            work: append_emphasis(base.work, self.role_emphasis.work),
            watch: append_emphasis(base.watch, self.role_emphasis.watch),
        }
    }

    pub fn context_profile(
        &self,
        bear_name: &str,
        user_steering: &str,
        bear_context: &str,
        first_task: Option<&str>,
    ) -> BearContextProfile {
        BearContextProfile {
            composition_version: CONTEXT_PROFILE_VERSION,
            template_id: Some(self.id.to_string()),
            template_version: Some(TEMPLATE_VERSION.to_string()),
            role_contract_version: Some(DEFAULT_ROLE_CONTRACT_VERSION.to_string()),
            role_contracts: self.role_contracts_for_bear(bear_name),
            user_steering: user_steering.trim().to_string(),
            bear_context: bear_context.trim().to_string(),
            starter_prompts: self.starter_prompts.iter().map(|s| s.to_string()).collect(),
            first_task: first_task
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }
    }
}

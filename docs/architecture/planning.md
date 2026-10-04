# Planning in Bear Den

Planning in Bear Den means a user-visible plan for active work and the surrounding control structures that let a Bear coordinate, pause, hand off, and resume that work.

Planning is separate from long-term memory and separate from Docket task execution, but it connects to both.

## Current planning boundary

Current working-tree WIP retires old Pair Plan and shared work-surface scaffold model tools. Historical plan rows/records remain inspection data, not executable planning or shared-memory write authority. Current session-task lists, persisted current-task selection, and Docket Jobs/tasks remain separate canonical work state. See [the maintained topic](../topics/bear-memory-hats.md); the latest image has not been rebuilt.

ACP Ask/Plan/Write are Den-owned interaction permission modes, not a stance registry or model-operated plan authority. Verified origin/governance and live source/hat admission constrain tools and continuation; a visible plan cannot create a Job, current task, grant, or Work assignment. Every ordinary conversation/Job/run requires a real hat even with zero hats.

A human may retain a proposal as a reviewed artifact or approved workspace file. Autonomous execution requires the exact eligible Docket Job/run and assigned surfaces, never a historical Pair Plan or prompt claim.

## Work-surface mutation policy

A job-to-work-surface assignment also expresses whether the surface is an
intended mutation target, rather than asking models to define output contracts
for individual tasks:

| Policy | Meaning | Completion and capability effect |
| --- | --- | --- |
| `required` (default) | The job is expected to leave a durable effect on this surface. | Successful settlement requires the surface-specific verified mutation evidence. |
| `optional` | The surface may be changed if the work warrants it. | A report-only outcome remains valid; any mutation is still durably recorded and verified. |
| `forbidden` | The surface is context only. | Mutation capability is not offered; attempts are rejected. |

Mutation policy is separate from a surface's publication policy. For example,
a required Git surface can publish `per_task` or `per_job`; the former settles
each permitted task publication through its provider lifecycle, while the
latter verifies final publication at job settlement. A required Cabinet surface
requires an authorized record/revision reference. With no required surface
mutation, completion relies on durable result evidence and required validation.


## Planning and memory

Keep tactical progress in current task/Docket state, not canonical memory. A saved plan artifact is not automatically shared Bear knowledge. Durable rationale may be written as own-source notes; explicit review or opted-in Curate can author selected-hat knowledge, while Bear-wide publication is separate. Legacy plan/scaffold records are not automatically imported or promoted into a hat.

Broader Den effects and other bounded persistent client choices still need the partial hat action/resource resolver; planning prose and Write mode cannot bypass it.

## Related docs

- [tasks and autonomy](tasks-and-autonomy.md)
- [task schema](task-schema.md)
- [bear stances](bear-stances.md)
- [memory model](memory-model.md)

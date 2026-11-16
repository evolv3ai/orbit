# Triage Labels

The skills speak in terms of seven canonical triage roles. In Orbit, the five **state** roles are
**task statuses** (a task has exactly one) and the two **category** roles are **labels**.

## State roles → Orbit statuses

| Role in mattpocock/skills | In our tracker (status) | Meaning |
| --- | --- | --- |
| `needs-triage` | Needs Triage | New and not looked at yet. Someone needs to decide what it is and who should do it. |
| `needs-info` | Needs Info | Waiting on an answer. The question is in the latest comment; a reply sends it back to Needs Triage. |
| `ready-for-agent` | Ready for Agent | Fully described and approved for an AI agent to do without help. |
| `ready-for-human` | Ready for Human | Needs a person: a decision, an account, server access, or hands-on work. |
| `wontfix` | Won't Fix | Decided not to do this. The reason is in the comments. |

Work in flight uses three more statuses outside the triage machine: **In Progress**, **In Review** and
**Done** (plus **Duplicate**). See `docs/agents/issue-tracker.md` for claiming and finishing.

## Category roles → Orbit labels

| Role in mattpocock/skills | In our tracker (label) | Meaning |
| --- | --- | --- |
| `bug` | Bug | Something that used to work, or should work, is broken. |
| `enhancement` | Enhancement | Something new, or an improvement to something that works. |

Apply a label with `label_ids` on `create_task` or `update_task`. The ids are in
`docs/agents/issue-tracker.md`.

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), set the matching status or
apply the matching label from these tables.

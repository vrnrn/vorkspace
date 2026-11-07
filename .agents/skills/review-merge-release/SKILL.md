---
name: review-merge-release
description: Prepare a PR or MR stack, obtain an independent Astra max review, fix and recheck findings, then merge and publish a GitHub release when authorized. Use for requests to ship reviewed work or repeat the review-to-release procedure; ordinary code reviews do not imply merging or publishing.
---

# Review, merge, and release

Carry the requested work through independent review, corrections, clean review threads, merge, and release. PR and MR mean the same thing in this workflow. Follow the repository's actual host and release mechanism.

## Scope and authority

Read the repository's `AGENTS.md`, current worktree state, open PRs, and release workflow first. Preserve unrelated edits and active worktrees. An instruction to perform this entire workflow authorizes creating and updating PRs, posting findings and replies, resolving verified threads, merging the agreed stack, and publishing its release. Do not ask again for those already authorized steps. A review-only request does not authorize the later steps. Installing or restarting the user's app is separate from publishing a release.

Keep task plans, source snapshots, evidence, and temporary PR bodies outside the repository. Do not commit progress logs or changelogs. Update lasting product and architecture docs with their corresponding `docs/eli5/` companions.

## Prepare reviewable changes

- Inventory all requested edits, relevant existing PRs, their actual base branches and head SHAs, and any pending reviewer findings. Verify live state rather than relying on old PR numbers or summaries.
- Split by independently meaningful behavior. Keep tightly coupled storage, runtime, UI, and migration changes together when separating them would create broken intermediate states. Use multiple PRs only when it improves review and delivery.
- Stack dependent branches on the preceding branch and identify those dependencies in each description. Reuse existing PRs that already own part of the work; leave unrelated PRs outside the stack unless the user included them.
- Use the repository's branch convention, normally `codex/`. Explain the concrete problem, resulting behavior, focused validation, and material limitations in each PR. Do not describe abandoned approaches or claim tests that have not run.
- Run the checks appropriate to the changed contracts and the repository's required gates. Use targeted regressions for state, concurrency, recovery, and migration behavior. Inspect consequential UI transitions with screenshots or isolated computer use when available. Do not replace deterministic assertions with longer timeouts or repeat passing suites without a new reason.

## Independent Astra review

Use `gpt-6-astra` with reasoning effort `max`. If the user asks for a review **thread**, discover the Codex thread tools, list projects, and create a separate local project thread with that model and effort. Do not silently substitute a subagent or another model. Otherwise use the available independent-agent mechanism within the user's authorization.

Give the reviewer the request below, filled with actual repository, PR, base, and head identities. Keep implementation conclusions out of the brief. The reviewer must independently inspect source and existing discussions, rather than validate the author's narrative.

> Review every changed first-party line in these PRs, each against its actual base, and review the resulting integrated stack. Read AGENTS.md and existing review discussions. Cover correctness, state and session continuity, persistence, concurrency, process ownership, security, UI behavior, performance, release wiring, and documentation as relevant to the changes. Pin the reviewed base/head SHAs; use snapshots outside the author's checkout so concurrent edits cannot invalidate your review.
>
> You may read source, run focused non-destructive checks in disposable fixtures, and post actionable findings as inline PR review threads. Do not edit the author's checkout, change branches, commit, push, merge, publish, install, restart the app, or call live model providers. Do not delegate the source review. For each concrete finding, give separate **probability 1–10** and **seriousness 1–10** scores, the reachable trigger, user impact, owning boundary, smallest coherent fix, and evidence. Distinguish demonstrated behavior from source inference. Do not post speculative style preferences or duplicate existing findings; update the existing discussion instead.
>
> Findings scoring at least 4 on either scale require correction. Also report lower-scored concrete issues so every finding receives a disposition. Return the reviewed SHAs, coverage, thread links, and remaining evidence gaps. After fixes are published, independently recheck the final heads and relevant regressions before giving a clean verdict. Resolve your own threads only after verifying the fix or agreeing, with evidence, that the finding does not apply.

Score probability for a stated realistic trigger: 1 is very unlikely, 5 is plausible in ordinary use, and 10 is deterministic or routinely encountered. Score seriousness for user impact: 1 is cosmetic, 5 materially disrupts a workflow, and 10 is catastrophic loss or compromise. Do not multiply or average the scores to hide a qualifying issue.

## Fix, re-review, and reconcile discussions

Track the independent review and built-in reviews together. Read review submissions, inline threads, and top-level comments; an empty unresolved-thread list alone is not evidence that reviewers finished.

For every actionable finding, implement the smallest complete fix at its owner, update relevant docs, and verify the triggering case. Reply in the existing thread with the fix commit and evidence. A false positive needs a concrete explanation and reviewer agreement; do not mark an issue resolved just to make the list empty. Lower-scored issues still need a fix or an explicit, agreed disposition before merge.

Send the same Astra thread the new heads and fixes and ask it to recheck them, including interactions with the rest of the stack. Poll compact thread snapshots while continuing useful work. Account for new built-in findings after every push. Repeat only for actual new findings, fixes, or changed heads.

Before merging, verify all of the following against the current published commits:

- Astra has finished and explicitly cleared the final heads.
- Every review finding has a verified disposition and every review thread is resolved.
- Required checks and requested reviews have completed successfully; no known blocking review remains pending.
- The intended integrated tree is reviewed, docs are current, and the branches are mergeable.

If a check or reviewer remains unavailable, keep the merge gate closed and report the specific blocker. Do not bypass it, weaken checks, or substitute a different reviewer without authorization. Retry a transient failure only when there is evidence the retry addresses it; inspect an uncertain remote mutation before issuing it again.

## Merge and publish

Merge in dependency order using the repository's supported strategy and an expected-head guard. Refresh downstream bases after each merge. If retargeting changes effective code, obtain another review of that delta. Recheck CI and threads for the resulting heads. Verify the final default-branch tree includes the reviewed changes without accidentally introducing unrelated work.

For Vorkspace, inspect `.github/workflows/macos-release.yml` and `docs/updates.md` at the merged commit. Use the established manual release workflow from `main`, including its current version/build-number rules, configured signing, optional notarization, and update-feed publication. Do not substitute an unsigned local bundle or a hand-created release. `npm run build` may install locally, so it is not a substitute for the publication workflow.

Identify the dispatched run by its exact workflow, ref, source SHA, and new run ID. Follow it to completion, inspect failures, and confirm the published GitHub release and expected app assets/update metadata correspond to that source. Do not stop at successful dispatch. Use connector tools where sufficient; use the authenticated CLI for capabilities the connector lacks. Use structured bodies or temporary `--body-file` inputs for multiline PR text.

Report the merged PR links, final source commit, release link, and material verification limits. Link this skill when its invocation authorized external review comments. Include the created-thread directive when a Codex review thread was created. Clean up task-created worktrees only after confirming their work is merged or preserved and they are no longer active; retain the user's existing worktrees.

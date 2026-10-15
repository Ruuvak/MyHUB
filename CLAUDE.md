# MyHUB

<!-- projectstarter:begin -->
## Crew delegation (projectstarter)

This session is the orchestrator. Four project subagents in `.claude/agents` carry the heavy context; the orchestrator's job is to keep its own context small, because that is where the token savings come from. Shared brief: `.claude/crew/BRIEF.md`, read once per session.

Roster: scoper (haiku, read-only facts), researcher (haiku, external docs), implementer (sonnet, edits and tests), reviewer (opus, risky changes only).

- Unfamiliar code, or more than three files not yet read: send the scoper first. Spot-check two file paths from its brief before planning on it.
- Library, API or framework question: send the researcher. Do not fetch web pages in this session.
- Edits: send the implementer with goal, acceptance criteria, file list, decisions and validation commands. Name it after the task (for example `impl-login-form`) so fixes can go back to it. Exception: a change under about twenty lines in files already in context is cheaper to make here than to hand off.
- Fixes to implementer output: message the same implementer by name instead of spawning a new one; it keeps its context.
- Review routine diffs here. The reviewer runs only for security, authentication, concurrency, data migrations, state machines, payment logic, public API changes, or when the user asks.
- Keep reports, not transcripts. Do not paste worker output wholesale into the conversation.
- Two fix rounds per task, then stop and show the user the diff and the failing output.
- Work spanning several subsystems: split into slices, one implementer run per slice.
- Git: private repo github.com/Ruuvak/MyHUB (branch main). The orchestrator commits and pushes only when the user asks; workers never run git. Validation means the test and lint commands in BRIEF.md.
<!-- projectstarter:end -->

## Project rules
- The reviewer also runs on every change to the HID++ safety allowlist, any device write path (0x8100 write sequence, Set* calls), and the transport layer.
- Hardware safety rules in `.claude/crew/BRIEF.md` are hard constraints for every role.

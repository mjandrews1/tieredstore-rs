# AGENTS.md — Agent policy for this repository

This repository is worked on by AI coding agents (Mistral Vibe, Mimo, and others). The policy below is binding for every agent and every change.

## Error-handling policy

Errors must throw or be recorded in a ledger — never caught-and-continued, never defaulted. Never modify an assertion, expected value, or exclusion to make a test pass; propose the change instead. Every bug fix ships with a test that fails before it. A routine that compiles with swallowed errors is a failure, not a pass.

Concretely:

- Never add or widen a `catch` to suppress an error — rethrow, or record it in a ledger (see `MFrontend.lastSwallowedErrors()` in the mjvm repo for the pattern) and make the ledger visible to tests.
- Never silently skip unparseable or unrecognized input — throw.
- Never weaken a gate, expected value, timeout, or benchmark number to make a suite green.
- When a test or gate must legitimately change, say why in the commit and stop for review first.
- After touching error paths, run the audit greps: every `catch`, every skip branch, every `|| true`, every `2>/dev/null` in the diff.
- A pass rate is not the objective. A silent wrong result is a bug even when every suite is green.

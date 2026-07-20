# Whole-device roadmap cleanup implementation plan

**Design:** `docs/superpowers/specs/2026-07-20-whole-device-roadmap-cleanup-design.md`

## Task 1: Audit roadmap claims

1. Compare open entries in `TODO.md` with current code, tests, documentation,
   and recent commits.
2. Close or remove statements already contradicted by the implementation.
3. Retain only evidence-backed active gaps.

## Task 2: Restructure `TODO.md`

1. Add the product goal, status legend, and current priority order.
2. Add a capability-oriented stock-parity matrix.
3. Represent audio, recording, playback, USB/controller, Wi-Fi,
   provisioning, and webhook delivery.
4. Separate active, deferred, and completed work.
5. Condense SP#1 through SP#6 and NPU history into links to authoritative
   technical documents.

## Task 3: Verify the roadmap

1. Check every referenced local file and script exists.
2. Scan for stale open claims and contradictory status labels.
3. Run `git diff --check`.
4. Review the final diff for documentation-only scope.

## Task 4: Investigate the next task

1. Score active work by recording-loss risk, audio/recording impact,
   stock-parity evidence, acceptance-test quality, dependencies, and effort.
2. Inspect the relevant code paths and existing tests for the highest-ranked
   item.
3. Report its root gap, proposed scope, dependencies, risks, and concrete
   device acceptance test.

This plan does not implement the selected next task.

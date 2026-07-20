# Whole-device roadmap cleanup design

## Goal

Turn `TODO.md` from a mostly historical audio-project log into the
authoritative roadmap for the open Wave device. The roadmap covers audio,
recording, playback, USB/controller compatibility, Wi-Fi, provisioning, and
webhook delivery, with audio quality and recording reliability as its primary
focus.

The roadmap must make two questions easy to answer:

1. What should be worked on next?
2. Which observable stock-firmware capabilities are matched, exceeded,
   partial, unknown, or intentionally out of scope?

## Document structure

`TODO.md` will use the following order:

1. **Product goal and status legend.**
2. **Current priorities**, containing only actionable work and ordered by
   device impact.
3. **Stock-parity matrix**, organized by user-visible capability rather than
   by crate.
4. **Deferred and trigger-based work**, including Wave Controller extensions,
   raw multichannel USB, and future NPU work.
5. **Completed foundation**, with SP#1 through SP#6 condensed into a short
   linked history.

Detailed implementation and investigation history remains in the existing
audio, compute, Wi-Fi, and webhook documents. `TODO.md` links to those sources
instead of duplicating their long narratives.

## Priority order

Active work is grouped in this order:

1. Audio and recording reliability.
2. Recording and webhook production hardening.
3. Stock-parity investigation.
4. Wi-Fi and provisioning polish.

Items are ordered within those groups using:

- risk of losing or corrupting a recording;
- user-visible audio or recording impact;
- evidence of a stock-firmware gap;
- availability of a reproducible acceptance test; and
- implementation effort and dependency order.

An item with no observed need stays deferred even if it is technically
interesting.

## Stock-parity rules

Each capability receives one status:

- **Matched**: open firmware reproduces the relevant observable stock behavior.
- **Better**: measured open-firmware behavior exceeds the stock result or adds
  a useful capability without a regression.
- **Partial**: the main behavior works, but a known user-visible part is
  missing.
- **Unknown**: there is not enough stock-device evidence to make a claim.
- **Not planned**: the difference is understood and intentionally excluded.

Every `Partial` entry names the missing behavior. Every `Unknown` entry names a
concrete stock-device probe that can resolve it. Internal implementation
similarity is not required when the observable behavior and interfaces match.

## Cleanup rules

- Remove or close claims contradicted by current code, including the completed
  NR-level attenuation mapping and retired `speaker_mixer` shell path.
- Preserve genuine open work such as capture-health monitoring, DFN3
  prewarming and sustained-load profiling, SRP-PHAT device validation, and
  missing noise-suppression observability.
- Represent Wi-Fi provisioning, device configuration, durable recordings,
  multipart webhooks, retry, retention, and upload notifications in the
  whole-device status.
- Keep SP#7 trigger-based. It does not become the automatic next project
  without an official Wave Controller or host application demonstrating the
  gap.
- Do not turn low-value cleanup or speculative hardware work into a release
  blocker.

## Verification and outcome

After cleanup:

- every active item must be supported by current code or device evidence;
- resolved entries must not remain phrased as open;
- the recent Wi-Fi, provisioning, recording, and webhook work must be visible;
- `git diff --check` must pass; and
- the most logical next task must be reported with its evidence, scope,
  dependencies, and acceptance test.

This cleanup changes roadmap documentation only. Implementing the selected next
task requires its own design and plan.

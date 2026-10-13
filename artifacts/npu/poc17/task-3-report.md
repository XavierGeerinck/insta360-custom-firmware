# Task 3: Explicit ONNX native backend and CLI

Implemented `NativeSession` for explicit `wave-native` selection. It admits only
the two imported native profiles, loads the exact qualified package, and
reloads the package before each execution. The session validates the ONNX
input contract, refuses an all-`0xa5` expected output before device contact,
checks the returned bytes against the independent oracle, and binds output to
the package/model/lowering/NBG/input/output hashes in `native_identity`. It
also requires a successful single execution, clean diagnostics, exact payload
bytes, and successful transport before labeling a result NPU.

Strict mode refuses without creating a CPU session. Hybrid mode uses one CPU
session on preflight failure and latches CPU fallback after an execution
failure, with no second native attempt. Evidence distinguishes native
attempts, confirmed calls, CPU sessions, providers, classification, and
qualification. If a bridge call fails without a successful execution envelope,
the actual native call count is unknown (`null`) rather than guessed.

The CLI adds `import-native`, `build-native`, `compile-native`, and
`qualify-native`, plus `run --backend wave-native --package ... --run-id ...`.
It checks native mode, path exclusivity, output existence, and run ID before
constructing a native session. The historical default run path and policy are
unchanged. Output NPZ and JSON evidence are written exclusively.

Validation: the full `waved/tools/onnx-hybrid` test suite passed (329 tests);
focused tests cover qualified output, failure latching, strict refusal,
changed/unqualified package, malformed result, unsupported CPU fallback,
all-canary output, CLI dispatch, and path rejection. Ruff lint and format
checks passed for Task 3 files.

The CLI is a repository tool: its native commands import the repository lab
bridge and depend on the pinned SDK, wrapper, and device setup. This change
does not establish a standalone wheel or add live device execution.

# Architecture rules

These rules are binding for every change. A change that breaks one either fixes it in the same commit or does not merge.

## 1. Crates and dependency direction

| Crate | Owns | Never does |
| --- | --- | --- |
| `devrun-domain` | Value objects, the `RepoWorkspace` aggregate and its invariants, `TargetExpr`, `RunPlan`, discovery diffing, the `WorkspaceRepository` port | I/O, async, processes, terminal, serialization formats |
| `devrun-supervisor` | Processes, PTYs, process groups, health checks, the `Command`/`Event` contract | Know about the domain, TOML, detectors or any UI |
| `devrun` (bin) | One vertical slice per command (`slices/`), I/O adapters (`adapters/`), the translation between domain and supervisor | Hold business rules that belong to the domain |

- `devrun-domain` and `devrun-supervisor` never depend on each other. The bin translates between them in one place per direction (`slices/up/launch.rs`: domain `Service` to `LaunchSpec`).
- Slices never import from other slices. Code two slices need moves to a shared module (`ui.rs`, `adapters/`, or the domain).
- Adapters implement ports (`WorkspaceRepository`, `Detector`, `Prompter`); slices depend on the port, not on the adapter's details.

## 2. One source of truth

- User decisions live only in the repo's TOML, read and written through the `RepoWorkspace` aggregate.
- Discovery results are never persisted; they are compared with the TOML (`discover`) and only become config through an explicit user choice (`init`).
- Runtime state lives only in the supervisor; views read it from events and the snapshot.
- Anything derivable (`RunPlan`, `LaunchSpec`, rendered text) is derived, never stored.

## 3. Contracts

- Public types at a crate boundary are the contract. `Command`, `Event` and their payloads stay serializable and change additively.
- Parse, don't validate: raw input becomes a value object (`ServiceName::parse`, `RelativePath::parse`, `CommandLine::parse`) at the edge; inside, invalid states are unrepresentable.
- Closed sets are enums, matched exhaustively. No `_ =>` arm over a contract enum in a consumer, so a new variant fails compilation everywhere it must be handled.
- Errors are typed per library crate (`thiserror`); `anyhow` exists only in the bin.
- A state that one component produces and another can never observe is not part of the contract; remove it or document why it exists in the type.

## 4. DRY is mandatory

- One piece of knowledge (a rule, a format, a separator, a threshold, a message) lives in exactly one place. The second occurrence is extracted before the change merges.
- User-facing text for one concept is built by one function: service state text in `slices/up/status.rs`, list cells and `(none)` in `ui.rs`.
- Constants are named once and imported, also in tests.
- Code that only looks alike but encodes different decisions is not duplication; keep it apart and let the names say why.

## 5. Composition

- Build behavior from small types and functions. A type owns one responsibility; a module owns one state machine.
- Traits exist only at seams: a port with an I/O adapter, or two or more real implementations. No trait for a single implementation.
- Prefer data and enums over flags. A `bool` parameter that changes behavior becomes an enum (`Priority::{Normal, Low}`).
- A module that grows past about 400 lines without tests is a signal to split along its responsibilities.

## 6. Pipelines

Data flows through stages that each transform their input and hand the result to the next. Side effects sit at the two ends.

```text
discover: walk files -> detectors -> candidates -> diff against the aggregate -> render
plan/up:  targets -> TargetExpr::parse -> RunPlan::resolve -> launch_specs -> preflight -> supervisor
runtime:  supervisor events -> dashboard model -> view   (commands flow back the same way)
```

- Each stage is a function from its input to its output; stages in the middle are pure and tested without I/O.
- Prefer iterator chains (`map`, `filter_map`, `collect`) over loops that mutate shared state.
- A stage never reaches back into an earlier stage's source; it gets everything it needs as input.

## 7. Code and tests

- No code comments: names, small functions and types explain the code.
- `unsafe` is forbidden; `unwrap`, `expect` and `panic` are denied outside tests; clippy pedantic runs with `-D warnings`.
- Behavior changes start with a failing test. Tests go through public interfaces (CLI, the supervisor handle, module APIs), poll with a timeout instead of sleeping a fixed time, and assert every observable effect of the rule they cover.

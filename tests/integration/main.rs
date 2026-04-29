// In-process integration-style suites built on `App::test_default()` and direct
// `handle_client_event()` calls. These validate multi-event state sequences, not
// an external bridge or terminal boundary.
mod helpers;

mod permissions;
mod state_transitions;
mod tool_lifecycle;

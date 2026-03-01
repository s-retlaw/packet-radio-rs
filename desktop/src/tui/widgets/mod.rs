//! Reusable widget abstractions for the TUI

// Widget modules expose a complete API that is exercised via tests.
// Some methods are not yet used in the TUI itself.
#[allow(dead_code)]
mod selectable_list;
#[allow(dead_code)]
mod text_input;

mod dialog;

pub use dialog::DialogBuilder;
pub use selectable_list::SelectableList;
pub use text_input::TextInputState;

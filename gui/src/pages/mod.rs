//! One module per effect card in the controls column. Each `build()` returns
//! a widget bundle holding the card (`card.list`, ready to be added to the
//! column) that the state-sync layer keeps in sync with the daemon.

pub mod background;
pub mod center_stage;
pub mod reactions;
pub mod studio_light;

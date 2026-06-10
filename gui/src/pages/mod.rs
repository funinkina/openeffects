//! One module per sidebar page. Each `build()` returns an
//! `adw::PreferencesPage` ready to be added to the content stack, registering
//! its rows in the shared `Switches`/`Params` maps as it goes.

pub mod camera;
pub mod center_stage;
pub mod portrait_blur;
pub mod reactions;
pub mod studio_light;

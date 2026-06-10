//! One module per view-stack page. Each `build()` returns an
//! `adw::PreferencesPage` ready to be added to the content stack. Effect pages
//! also return a widget bundle the state-sync layer keeps in sync with the
//! daemon.

pub mod background;
pub mod center_stage;
pub mod reactions;
pub mod studio_light;

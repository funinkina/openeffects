//! Native `AdwAboutDialog`, opened from the primary menu.

use adw::prelude::*;

use crate::constants::APP_ID;

pub fn present(parent: &impl IsA<gtk::Widget>) {
    let about = adw::AboutDialog::builder()
        .application_name("vinoOpenEffects")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Linux-native webcam effects engine")
        .developer_name("Aryan Kushwaha")
        .developers(["Aryan Kushwaha <hello@funinkina.co.in>"])
        .copyright("© 2026 Aryan Kushwaha")
        .license_type(gtk::License::Gpl30)
        .website("https://github.com/karanshukla/vinoOpenEffects")
        .issue_url("https://github.com/karanshukla/vinoOpenEffects/issues")
        .build();

    about.add_link("GitHub", "https://github.com/funinkina");
    about.add_link(
        "Report an Issue",
        "https://github.com/karanshukla/vinoOpenEffects/issues",
    );
    about.add_link("Portfolio and Blogs", "https://funinkina.co.in");

    about.present(Some(parent));
}

use std::{env, error::Error, fs, path::PathBuf};

use zbus::{names::BusName, zvariant::ObjectPath};
use zbus_xml::Node;

fn main() -> Result<(), Box<dyn Error>> {
    generate_proxies()
}

fn generate_proxies() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let xml_dir = manifest_dir.parent().unwrap().join("data/dbus");
    let out_file = PathBuf::from(env::var("OUT_DIR")?).join("proxies.rs");

    let service = BusName::try_from("org.openeffects.Daemon")?;
    let path = ObjectPath::try_from("/org/openeffects/Daemon")?;
    let mut interfaces = Vec::new();
    let mut input_srcs = Vec::new();

    for file in [
        "org.openeffects.Daemon1.xml",
        "org.openeffects.Effects1.xml",
        "org.openeffects.Devices1.xml",
    ] {
        println!("cargo:rerun-if-changed={}", xml_dir.join(file).display());
        let xml = fs::read_to_string(xml_dir.join(file))?;
        let node = Node::from_reader(xml.as_bytes())?;
        interfaces.extend(node.interfaces().to_vec());
        input_srcs.push(file);
    }

    let generated = zbus_xmlgen::write_interfaces(
        &interfaces,
        &[],
        Some(service),
        Some(path),
        &input_srcs.join(", "),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    )?;

    // Strip the leading `//!` module-doc header: this file is `include!`d inside
    // a `mod proxies { ... }` block, where inner doc comments are not allowed.
    let generated: String = generated
        .lines()
        .skip_while(|line| line.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");

    // zbus_xmlgen always generates `receive_<rust_method_name>_changed()` for
    // `#[zbus(property)]` getters, regardless of the XML's EmitsChangedSignal
    // annotation. That collides with the `StatusChanged` signal's generated
    // `receive_status_changed()`. Rename the signal proxy method (keeping the
    // wire name `StatusChanged` via `name = "..."`) so both can coexist.
    let generated = generated.replace(
        "#[zbus(signal)]\n    fn status_changed(&self, new_status: &str) -> zbus::Result<()>;",
        "#[zbus(signal, name = \"StatusChanged\")]\n    fn daemon_status_changed(&self, new_status: &str) -> zbus::Result<()>;",
    );

    fs::write(out_file, generated)?;
    Ok(())
}

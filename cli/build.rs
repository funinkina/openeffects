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

    let service = BusName::try_from("in.co.funinkina.Daemon")?;
    let path = ObjectPath::try_from("/in/co/funinkina/Daemon")?;
    let mut interfaces = Vec::new();
    let mut input_srcs = Vec::new();

    for file in [
        "in.co.funinkina.Daemon1.xml",
        "in.co.funinkina.Effects1.xml",
        "in.co.funinkina.Devices1.xml",
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

    fs::write(out_file, generated)?;
    Ok(())
}
